//! `knowledge_lint` (issue #755, ADR-0755): a deterministic, LLM-free, read-only notebook health
//! report.
//!
//! Three phases. **Snapshot** ([`snapshot::LintSnapshot::load`]) runs chunked keyset scans for the
//! tables the selected categories need. **Analyse** is one pure function per category from the
//! snapshot and the evaluated instant to a sorted `Vec<Finding>`; there is no DB access and no
//! clock inside. **Page** concatenates the categories in a fixed order and slices them with a
//! keyset cursor `(category index, finding id)`. Counts are exact on every page, so every call
//! does a full pass over the selected categories; only the returned page is hydrated with entity
//! names and stored embeddings.
//!
//! Lint reports. It never merges, edits, deletes, queues or suggests anything (FR-017), and it
//! reads origin as a caller claim that is not authenticated (ADR-0752).

pub mod classes;
pub mod contradiction;
pub mod entities;
pub mod ids;
pub mod links;
pub mod snapshot;
pub mod stale;
pub mod weak;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::db::Conn;
use crate::error::Error;
use crate::read_page as rp;

use classes::Ctx;
use snapshot::{LintSnapshot, Needs, DEFAULT_CHUNK};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;
/// Default similarity at or above which two entities are a candidate merge.
pub const DEFAULT_NEAR_DUPLICATE_THRESHOLD: f64 = 0.90;
/// Default number of distinct documents that must mention an entity before it lacks a page.
pub const DEFAULT_MIN_MENTIONS: usize = 3;
/// Citation lists are sorted and capped so a report page stays bounded and deterministic.
pub const CITATION_CAP: usize = 25;

/// The finding categories, in the fixed order the report lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Contradiction,
    StaleDerivedPage,
    WeakSupport,
    OrphanEntity,
    NearDuplicateEntity,
    UnlinkedMention,
}

pub const ALL_CATEGORIES: [Category; 6] = [
    Category::Contradiction,
    Category::StaleDerivedPage,
    Category::WeakSupport,
    Category::OrphanEntity,
    Category::NearDuplicateEntity,
    Category::UnlinkedMention,
];

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Contradiction => "contradiction",
            Category::StaleDerivedPage => "stale_derived_page",
            Category::WeakSupport => "weak_support",
            Category::OrphanEntity => "orphan_entity",
            Category::NearDuplicateEntity => "near_duplicate_entity",
            Category::UnlinkedMention => "unlinked_mention",
        }
    }

    pub fn index(self) -> usize {
        ALL_CATEGORIES.iter().position(|c| *c == self).unwrap_or(0)
    }

    pub fn parse(s: &str) -> Option<Category> {
        ALL_CATEGORIES.iter().copied().find(|c| c.as_str() == s)
    }
}

fn category_vocabulary() -> String {
    ALL_CATEGORIES
        .iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// One fact a finding cites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactCite {
    pub uuid: String,
    pub source: String,
    pub target: String,
    pub relation: String,
}

/// What a finding cites, before entity names are hydrated.
#[derive(Debug, Clone, Default)]
pub struct Citations {
    pub episodes: BTreeSet<String>,
    /// `doc_id`s (an episode without one is cited by episode only).
    pub documents: BTreeSet<String>,
    pub entities: BTreeSet<String>,
    pub facts: BTreeMap<String, FactCite>,
}

impl Citations {
    pub fn fact(&mut self, e: &snapshot::LintEdge) {
        self.facts.insert(
            e.uuid().to_string(),
            FactCite {
                uuid: e.uuid().to_string(),
                source: e.source.clone(),
                target: e.target.clone(),
                relation: e.name.clone(),
            },
        );
    }

    /// Cites an episode and, when it has one, its document.
    pub fn episode(&mut self, e: &snapshot::LintEpisode) {
        self.episodes.insert(e.uuid.clone());
        if let Some(d) = &e.doc_id {
            self.documents.insert(d.clone());
        }
    }
}

/// A single reported problem. Never persisted.
#[derive(Debug, Clone)]
pub struct Finding {
    pub id: String,
    pub category: Category,
    pub group_id: String,
    pub reason: &'static str,
    pub details: Value,
    pub citations: Citations,
}

/// Whether a category could be evaluated, with its findings sorted by id.
#[derive(Debug, Clone, Default)]
pub struct CategoryResult {
    /// `Some(reason)` ⇒ the category is `unavailable` and `findings` is empty.
    pub unavailable: Option<&'static str>,
    pub findings: Vec<Finding>,
    /// Extra per-category summary keys (e.g. `skipped_blocks`).
    pub extra: Option<Value>,
}

impl CategoryResult {
    pub fn unavailable(reason: &'static str) -> Self {
        CategoryResult {
            unavailable: Some(reason),
            ..Default::default()
        }
    }

    pub fn ok(mut findings: Vec<Finding>) -> Self {
        findings.sort_by(|a, b| a.id.cmp(&b.id));
        findings.dedup_by(|a, b| a.id == b.id);
        CategoryResult {
            unavailable: None,
            findings,
            extra: None,
        }
    }
}

/// A validated `knowledge_lint` request.
#[derive(Debug, Clone)]
pub struct LintRequest {
    pub groups: Option<Vec<String>>,
    /// Selected categories in report order.
    pub categories: Vec<Category>,
    pub limit: usize,
    /// Normalised (trimmed, ASCII-lowercased), sorted, deduplicated.
    pub single_valued: Vec<String>,
    pub near_duplicate_threshold: f64,
    pub min_mentions: usize,
    /// Decoded cursor position: `(category index, last finding id)`.
    pub after: Option<(usize, String)>,
}

impl LintRequest {
    /// Validates `p` and decodes the cursor, before any work is done (FR-018). `groups` is the
    /// already-resolved group scope (`None` ⇒ every group).
    pub fn parse(p: &Value, groups: Option<Vec<String>>) -> Result<LintRequest, Error> {
        let categories = match &p["categories"] {
            Value::Null => ALL_CATEGORIES.to_vec(),
            Value::Array(arr) => {
                if arr.is_empty() {
                    return Err(Error::Ipc(format!(
                        "categories must not be empty (allowed: {})",
                        category_vocabulary()
                    )));
                }
                let mut set = BTreeSet::new();
                for v in arr {
                    let s = v.as_str().ok_or_else(|| {
                        Error::Ipc("categories must be an array of strings".to_string())
                    })?;
                    let c = Category::parse(s).ok_or_else(|| {
                        Error::Ipc(format!(
                            "unknown category '{s}' (allowed: {})",
                            category_vocabulary()
                        ))
                    })?;
                    set.insert(c);
                }
                set.into_iter().collect()
            }
            _ => {
                return Err(Error::Ipc(
                    "categories must be an array of strings".to_string(),
                ))
            }
        };

        let limit = match &p["limit"] {
            Value::Null => DEFAULT_LIMIT,
            v => {
                let n = v.as_u64().ok_or_else(|| {
                    Error::Ipc(format!("limit must be an integer in 1..={MAX_LIMIT}"))
                })?;
                if n == 0 || n > MAX_LIMIT as u64 {
                    return Err(Error::Ipc(format!("limit must be in 1..={MAX_LIMIT}")));
                }
                n as usize
            }
        };

        let near_duplicate_threshold = match &p["near_duplicate_threshold"] {
            Value::Null => DEFAULT_NEAR_DUPLICATE_THRESHOLD,
            v => {
                let t = v.as_f64().ok_or_else(|| {
                    Error::Ipc("near_duplicate_threshold must be a number in (0, 1]".to_string())
                })?;
                if !(t > 0.0 && t <= 1.0) {
                    return Err(Error::Ipc(
                        "near_duplicate_threshold must be in (0, 1]".to_string(),
                    ));
                }
                t
            }
        };

        let min_mentions = match &p["min_mentions"] {
            Value::Null => DEFAULT_MIN_MENTIONS,
            v => {
                let n = v.as_u64().filter(|n| *n >= 1).ok_or_else(|| {
                    Error::Ipc("min_mentions must be an integer >= 1".to_string())
                })?;
                n as usize
            }
        };

        let single_valued = match &p["single_valued_relations"] {
            Value::Null => Vec::new(),
            Value::Array(arr) => {
                let mut set = BTreeSet::new();
                for v in arr {
                    let s = v.as_str().map(str::trim).filter(|s| !s.is_empty());
                    let s = s.ok_or_else(|| {
                        Error::Ipc(
                            "single_valued_relations must be an array of non-empty strings"
                                .to_string(),
                        )
                    })?;
                    set.insert(s.to_ascii_lowercase());
                }
                set.into_iter().collect()
            }
            _ => {
                return Err(Error::Ipc(
                    "single_valued_relations must be an array of non-empty strings".to_string(),
                ))
            }
        };

        let mut req = LintRequest {
            groups,
            categories,
            limit,
            single_valued,
            near_duplicate_threshold,
            min_mentions,
            after: None,
        };

        let cursor = match &p["cursor"] {
            Value::Null => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s.as_str()),
            _ => return Err(Error::Ipc("cursor must be a string".to_string())),
        };
        if let Some(c) = cursor {
            let pos = rp::decode_cursor(c, rp::LINT_TAG, &req.shape())?;
            match (pos["c"].as_u64(), pos["id"].as_str()) {
                (Some(c), Some(id)) if (c as usize) < ALL_CATEGORIES.len() => {
                    req.after = Some((c as usize, id.to_string()));
                }
                _ => return Err(Error::Ipc("invalid cursor: malformed".to_string())),
            }
        }
        Ok(req)
    }

    /// The cursor shape: every input that changes which findings exist or their order.
    pub fn shape(&self) -> String {
        let cats: Vec<&str> = self.categories.iter().map(|c| c.as_str()).collect();
        rp::lint_shape_hash(
            self.groups.as_deref(),
            &cats,
            self.near_duplicate_threshold,
            self.min_mentions,
            &self.single_valued,
        )
    }

    pub fn needs(&self) -> Needs {
        Needs::for_categories(&self.categories)
    }

    /// The effective parameters, echoed in every response.
    pub fn effective(&self) -> Value {
        json!({
            "categories": self.categories.iter().map(|c| c.as_str()).collect::<Vec<_>>(),
            "limit": self.limit,
            "near_duplicate_threshold": self.near_duplicate_threshold,
            "min_mentions": self.min_mentions,
            "single_valued_relations": self.single_valued,
        })
    }
}

/// The analysed report: summary over the whole scope plus the requested page.
#[derive(Debug)]
pub struct Analysis {
    pub summary: Value,
    pub page: Vec<Finding>,
    pub next_cursor: Option<String>,
}

/// Runs every selected category's analyser over `snap`, builds the summary and slices the page.
/// Pure: no DB access, no clock.
pub fn analyse(snap: &LintSnapshot, req: &LintRequest, as_of_us: i64) -> Analysis {
    let ctx = Ctx::new(snap, as_of_us);
    let mut results: Vec<(Category, CategoryResult)> = Vec::new();
    for c in &req.categories {
        let r = match c {
            Category::Contradiction => contradiction::analyse(&ctx, &req.single_valued),
            Category::StaleDerivedPage => stale::analyse(&ctx),
            Category::WeakSupport => weak::analyse(&ctx),
            Category::OrphanEntity => entities::analyse_orphans(&ctx),
            Category::NearDuplicateEntity => {
                entities::analyse_near_duplicates(&ctx, req.near_duplicate_threshold)
            }
            Category::UnlinkedMention => links::analyse(&ctx, req.min_mentions),
        };
        results.push((*c, r));
    }

    let mut cats = Map::new();
    let mut total = 0usize;
    for (c, r) in &results {
        let mut entry = match r.unavailable {
            Some(reason) => json!({"status": "unavailable", "count": 0, "reason": reason}),
            None => {
                total += r.findings.len();
                json!({"status": "ok", "count": r.findings.len()})
            }
        };
        if let (Some(Value::Object(extra)), Some(obj)) = (&r.extra, entry.as_object_mut()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        cats.insert(c.as_str().to_string(), entry);
    }
    let summary = json!({"categories": Value::Object(cats), "total": total});

    // Page: fixed category order, then finding id. The cursor is the last emitted `(c, id)`.
    let mut page: Vec<Finding> = Vec::new();
    let mut more = false;
    'outer: for (c, r) in results {
        if let Some((ci, _)) = &req.after {
            if c.index() < *ci {
                continue;
            }
        }
        for f in r.findings {
            if let Some((ci, last)) = &req.after {
                if c.index() == *ci && f.id.as_str() <= last.as_str() {
                    continue;
                }
            }
            if page.len() == req.limit {
                more = true;
                break 'outer;
            }
            page.push(f);
        }
    }
    let next_cursor = if more {
        page.last().map(|f| {
            rp::encode_cursor(
                rp::LINT_TAG,
                &req.shape(),
                json!({"c": f.category.index(), "id": f.id}),
            )
        })
    } else {
        None
    };
    Analysis {
        summary,
        page,
        next_cursor,
    }
}

fn capped<T: Clone>(items: impl Iterator<Item = T>) -> (Vec<T>, usize) {
    let all: Vec<T> = items.collect();
    let total = all.len();
    (all.into_iter().take(CITATION_CAP).collect(), total)
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(((dot / (na.sqrt() * nb.sqrt())) * 1e6).round() / 1e6)
}

/// Renders the page, resolving entity names (and the stored-embedding cosine of near-duplicate
/// pairs) with two batched reads. Citation lists are sorted and capped, with `*_total` alongside.
pub fn hydrate(conn: &Conn<'_>, page: Vec<Finding>) -> Result<Vec<Value>, Error> {
    let mut uuids: BTreeSet<String> = BTreeSet::new();
    let mut pair_uuids: BTreeSet<String> = BTreeSet::new();
    for f in &page {
        let (ents, _) = capped(f.citations.entities.iter().cloned());
        uuids.extend(ents);
        let (facts, _) = capped(f.citations.facts.values().cloned());
        for fc in facts {
            uuids.insert(fc.source);
            uuids.insert(fc.target);
        }
        if f.category == Category::NearDuplicateEntity {
            pair_uuids.extend(f.citations.entities.iter().cloned());
        }
    }
    let names = conn.lint_entity_names(&uuids.into_iter().collect::<Vec<_>>())?;
    let embeddings = conn.lint_entity_embeddings(&pair_uuids.into_iter().collect::<Vec<_>>())?;
    let name_of = |u: &str| names.get(u).cloned().unwrap_or_default();

    Ok(page
        .into_iter()
        .map(|f| {
            let (episodes, episodes_total) = capped(f.citations.episodes.iter().cloned());
            let (documents, documents_total) = capped(f.citations.documents.iter().cloned());
            let (ents, entities_total) = capped(f.citations.entities.iter().cloned());
            let (facts, facts_total) = capped(f.citations.facts.values().cloned());
            let mut details = f.details.clone();
            if f.category == Category::NearDuplicateEntity {
                let ids: Vec<&String> = f.citations.entities.iter().collect();
                let cos = match (ids.first(), ids.get(1)) {
                    (Some(a), Some(b)) => match (embeddings.get(*a), embeddings.get(*b)) {
                        (Some(x), Some(y)) => cosine(x, y),
                        _ => None,
                    },
                    _ => None,
                };
                details["embedding_cosine"] = json!(cos);
            }
            json!({
                "id": f.id,
                "category": f.category.as_str(),
                "group_id": f.group_id,
                "reason": f.reason,
                "details": details,
                "citations": {
                    "episodes": episodes,
                    "episodes_total": episodes_total,
                    "documents": documents,
                    "documents_total": documents_total,
                    "entities": ents.iter()
                        .map(|u| json!({"uuid": u, "name": name_of(u)}))
                        .collect::<Vec<_>>(),
                    "entities_total": entities_total,
                    "facts": facts.iter().map(|fc| json!({
                        "uuid": fc.uuid,
                        "source": {"uuid": fc.source, "name": name_of(&fc.source)},
                        "target": {"uuid": fc.target, "name": name_of(&fc.target)},
                        "relation": fc.relation,
                    })).collect::<Vec<_>>(),
                    "facts_total": facts_total,
                },
            })
        })
        .collect())
}

/// Assembles the response object. No timestamp is included, so identical calls on an unchanged
/// graph return byte-identical output.
pub fn response(
    req: &LintRequest,
    summary: Value,
    findings: Vec<Value>,
    next_cursor: Option<String>,
) -> Value {
    let count = findings.len();
    json!({
        "findings": findings,
        "count": count,
        "next_cursor": next_cursor,
        "summary": summary,
        "effective": req.effective(),
    })
}

/// Loads, analyses and hydrates in one call (tests and non-handler callers). The IPC handler
/// composes the same phases so it can release the read lock before the CPU-bound analysis.
pub fn run_lint(conn: &Conn<'_>, req: &LintRequest, as_of_us: i64) -> Result<Value, Error> {
    run_lint_with_chunk(conn, req, as_of_us, DEFAULT_CHUNK)
}

pub fn run_lint_with_chunk(
    conn: &Conn<'_>,
    req: &LintRequest,
    as_of_us: i64,
    chunk: usize,
) -> Result<Value, Error> {
    let snap = LintSnapshot::load(conn, req.groups.as_deref(), req.needs(), chunk)?;
    let a = analyse(&snap, req, as_of_us);
    let findings = hydrate(conn, a.page)?;
    Ok(response(req, a.summary, findings, a.next_cursor))
}

#[cfg(test)]
mod tests;
