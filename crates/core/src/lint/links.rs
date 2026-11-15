//! Unlinked mentions and missing pages (FR-015).
//!
//! LCG does not model notebook pages or links, so the category reads a convention carried in the
//! free-form episode `attributes` JSON (ADR-0528) — parsed only when this category is selected, and
//! never by text matching:
//!
//! * `page_for: "<entity uuid or entity name>"` on any live episode of a document makes that
//!   document the entity's page. A uuid is exact; otherwise a unique case-insensitive name match
//!   within the group is used.
//! * `links: ["<doc_id>", …]` on any live episode of a document opts it into link tracking (an
//!   empty array counts); the union over its live episodes is its outgoing link set.
//!
//! The category is `unavailable` unless the scope carries both keys somewhere. It then reports
//! `unlinked_mention` (a link-tracked document mentions an entity that has a page it does not link
//! to) and `missing_page` (an entity mentioned by at least `min_mentions` distinct live documents
//! with no page).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};

use super::classes::Ctx;
use super::ids::finding_id;
use super::{Category, CategoryResult, Citations, Finding};

type DocKey = (String, String); // (group, doc_key)

fn doc_cite(cites: &mut Citations, doc_key: &str) {
    if let Some(d) = doc_key.strip_prefix("doc:") {
        cites.documents.insert(d.to_string());
    } else if let Some(e) = doc_key.strip_prefix("ep:") {
        cites.episodes.insert(e.to_string());
    }
}

pub fn analyse(ctx: &Ctx<'_>, min_mentions: usize) -> CategoryResult {
    // Entity lookup per group.
    let mut uuids: BTreeMap<&str, &str> = BTreeMap::new(); // uuid → group
    let mut by_name: HashMap<(&str, String), Vec<&str>> = HashMap::new();
    for e in &ctx.snap.entities {
        uuids.insert(e.uuid.as_str(), e.group_id.as_str());
        by_name
            .entry((e.group_id.as_str(), e.name.trim().to_lowercase()))
            .or_default()
            .push(e.uuid.as_str());
    }

    let mut page_decls: Vec<(String, String, String)> = Vec::new(); // (group, doc_key, raw ref)
    let mut link_sets: BTreeMap<DocKey, BTreeSet<String>> = BTreeMap::new();
    let mut doc_eps: BTreeMap<DocKey, Vec<&str>> = BTreeMap::new();
    let mut doc_of_ep: HashMap<&str, DocKey> = HashMap::new();
    let mut any_links = false;

    for ep in ctx.snap.episodes.iter().filter(|e| e.is_live(ctx.as_of_us)) {
        let key: DocKey = (ep.group_id.clone(), ep.doc_key());
        doc_eps
            .entry(key.clone())
            .or_default()
            .push(ep.uuid.as_str());
        doc_of_ep.insert(ep.uuid.as_str(), key.clone());
        let Some(attrs) = ep.attributes.as_deref().filter(|a| !a.is_empty()) else {
            continue;
        };
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(attrs) else {
            continue;
        };
        if let Some(Value::String(r)) = obj.get("page_for") {
            if !r.trim().is_empty() {
                page_decls.push((key.0.clone(), key.1.clone(), r.trim().to_string()));
            }
        }
        if let Some(Value::Array(links)) = obj.get("links") {
            any_links = true;
            let set = link_sets.entry(key.clone()).or_default();
            for l in links.iter().filter_map(Value::as_str) {
                if !l.is_empty() {
                    set.insert(format!("doc:{l}"));
                }
            }
        }
    }
    if page_decls.is_empty() {
        return CategoryResult::unavailable("no_page_data");
    }
    if !any_links {
        return CategoryResult::unavailable("no_link_data");
    }

    // Entity → page document (the smallest doc_key if several declare one).
    let mut pages: BTreeMap<(String, String), String> = BTreeMap::new(); // (group, entity) → doc_key
    for (group, doc_key, raw) in &page_decls {
        let entity = if uuids.get(raw.as_str()) == Some(&group.as_str()) {
            Some(raw.clone())
        } else {
            match by_name.get(&(group.as_str(), raw.to_lowercase())) {
                Some(v) if v.len() == 1 => Some(v[0].to_string()),
                _ => None,
            }
        };
        if let Some(e) = entity {
            let slot = pages
                .entry((group.clone(), e))
                .or_insert_with(|| doc_key.clone());
            if doc_key < slot {
                *slot = doc_key.clone();
            }
        }
    }

    // Document → entities it mentions (live episodes only).
    let mut doc_entities: BTreeMap<DocKey, BTreeMap<&str, BTreeSet<&str>>> = BTreeMap::new();
    for (ep, ent) in &ctx.snap.mentions {
        if let Some(key) = doc_of_ep.get(ep.as_str()) {
            doc_entities
                .entry(key.clone())
                .or_default()
                .entry(ent.as_str())
                .or_default()
                .insert(ep.as_str());
        }
    }

    let mut out = Vec::new();

    // unlinked_mention: link-tracked document → mentioned entity with a page it does not link to.
    for (doc, links) in &link_sets {
        let Some(ents) = doc_entities.get(doc) else {
            continue;
        };
        for (ent, eps) in ents {
            let Some(page) = pages.get(&(doc.0.clone(), (*ent).to_string())) else {
                continue;
            };
            if page == &doc.1 || links.contains(page) {
                continue;
            }
            let mut cites = Citations::default();
            doc_cite(&mut cites, &doc.1);
            doc_cite(&mut cites, page);
            cites.entities.insert((*ent).to_string());
            cites.episodes.extend(eps.iter().map(|e| (*e).to_string()));
            out.push(Finding {
                id: finding_id(
                    Category::UnlinkedMention.as_str(),
                    &doc.0,
                    &[&doc.1, ent, "unlinked_mention"],
                ),
                category: Category::UnlinkedMention,
                group_id: doc.0.clone(),
                reason: "unlinked_mention",
                details: json!({"document": doc.1, "page": page}),
                citations: cites,
            });
        }
    }

    // missing_page: entity mentioned by ≥ min_mentions distinct live documents, no page.
    let mut mentioned_in: BTreeMap<(String, &str), BTreeSet<&str>> = BTreeMap::new();
    for (doc, ents) in &doc_entities {
        for ent in ents.keys() {
            mentioned_in
                .entry((doc.0.clone(), *ent))
                .or_default()
                .insert(doc.1.as_str());
        }
    }
    for ((group, ent), docs) in &mentioned_in {
        if docs.len() < min_mentions || pages.contains_key(&(group.clone(), (*ent).to_string())) {
            continue;
        }
        let mut cites = Citations::default();
        cites.entities.insert((*ent).to_string());
        for d in docs {
            doc_cite(&mut cites, d);
        }
        out.push(Finding {
            id: finding_id(
                Category::UnlinkedMention.as_str(),
                group,
                &[ent, "missing_page"],
            ),
            category: Category::UnlinkedMention,
            group_id: group.clone(),
            reason: "missing_page",
            details: json!({"mention_documents": docs.len(), "min_mentions": min_mentions}),
            citations: cites,
        });
    }
    CategoryResult::ok(out)
}
