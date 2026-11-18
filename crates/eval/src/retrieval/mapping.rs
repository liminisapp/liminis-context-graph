//! Result → note / evidence mapping (FR-018). One definition, applied to every configuration.
//!
//! * **Passages** map by `source_description`, which `knowledge_process_chunk` sets to
//!   `"{note}:{chunk_id}"`; the note is everything before the first `:`. Evidence = episode text.
//! * **Entities** map through an ingest-time `entity uuid → notes` table (no query-time lookups).
//!   One ranked entity expands to all of its notes, in note-id order. Evidence = `name: summary`.
//! * **Facts** map through the edge's *producing* episodes (`episodes`, ADR-0705 provenance) and an
//!   ingest-time `episode uuid → note` table. ADR-0012 either-endpoint inference is never used.
//!   Evidence = the fact string. A fact with no producing episode maps to no note.
//!
//! All mappers return notes in rank order **with duplicates**; `metrics::dedupe_truncate` keeps the
//! first occurrence, so a note's rank is that of its best-ranked result.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

/// Rough characters-per-token used by the chunker (no tokenizer dependency).
pub const CHARS_PER_TOKEN: usize = 4;

pub fn note_of_source_description(sd: &str) -> Option<&str> {
    let n = sd.split(':').next()?;
    (!n.is_empty() && n != sd).then_some(n)
}

/// Passages (`knowledge_search_passages` → `passages`) to (notes, evidence texts).
pub fn map_passages(passages: &[Value]) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut evidence = Vec::new();
    for p in passages {
        evidence.push(p["content"].as_str().unwrap_or("").to_string());
        if let Some(n) = p["source_description"]
            .as_str()
            .and_then(note_of_source_description)
        {
            notes.push(n.to_string());
        }
    }
    (notes, evidence)
}

/// Entities (`knowledge_find_entities` → `nodes`).
pub fn map_entities(
    nodes: &[Value],
    entity_notes: &HashMap<String, BTreeSet<String>>,
) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut evidence = Vec::new();
    for n in nodes {
        let name = n["name"].as_str().unwrap_or("");
        let summary = n["summary"].as_str().unwrap_or("");
        evidence.push(if summary.is_empty() {
            name.to_string()
        } else {
            format!("{name}: {summary}")
        });
        if let Some(set) = n["uuid"].as_str().and_then(|u| entity_notes.get(u)) {
            notes.extend(set.iter().cloned());
        }
    }
    (notes, evidence)
}

/// Facts (`knowledge_find_relationships` → `edges`). `unmapped` lists the uuids of edges that
/// carried no `episodes` field, so the caller can run its counted fallback lookup.
pub struct MappedEdges {
    pub notes: Vec<String>,
    pub evidence: Vec<String>,
    pub unmapped: Vec<String>,
}

/// Episodes that produced an edge. `knowledge_find_relationships` and
/// `knowledge_get_edges_by_uuids` do not return an `episodes` field (their `episode_uuids` is the
/// either-endpoint inference of ADR-0012, empty on these reads); the real provenance is
/// `belief.producing_episode_uuids`, present whenever the read carries a belief parameter
/// (ADR-0705; `believed_only: false` annotates without filtering). `episodes` is still honoured
/// when present. Issue #787: reading only `episodes` mapped every fact to no note at all.
pub fn edge_producers(e: &Value) -> Vec<&str> {
    for v in [&e["episodes"], &e["belief"]["producing_episode_uuids"]] {
        if let Some(a) = v.as_array() {
            let eps: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            if !eps.is_empty() {
                return eps;
            }
        }
    }
    Vec::new()
}

pub fn map_edges(edges: &[Value], episode_note: &HashMap<String, String>) -> MappedEdges {
    let mut out = MappedEdges {
        notes: Vec::new(),
        evidence: Vec::new(),
        unmapped: Vec::new(),
    };
    for e in edges {
        out.evidence
            .push(e["fact"].as_str().unwrap_or("").to_string());
        let eps = edge_producers(e);
        if eps.is_empty() {
            if let Some(u) = e["uuid"].as_str() {
                out.unmapped.push(u.to_string());
            }
            continue;
        }
        let mut set = BTreeSet::new();
        for ep in eps {
            if let Some(n) = episode_note.get(ep) {
                set.insert(n.clone());
            }
        }
        out.notes.extend(set);
    }
    out
}

/// Per-result note sets for entities, in rank order (empty set when unmapped).
pub fn entity_groups(
    nodes: &[Value],
    entity_notes: &HashMap<String, BTreeSet<String>>,
) -> Vec<Vec<String>> {
    nodes
        .iter()
        .map(|n| {
            n["uuid"]
                .as_str()
                .and_then(|u| entity_notes.get(u))
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default()
        })
        .collect()
}

/// Per-result note sets for facts (producing episodes only), in rank order.
pub fn edge_groups(edges: &[Value], episode_note: &HashMap<String, String>) -> Vec<Vec<String>> {
    edges
        .iter()
        .map(|e| {
            let set: BTreeSet<String> = edge_producers(e)
                .into_iter()
                .filter_map(|ep| episode_note.get(ep).cloned())
                .collect();
            set.into_iter().collect()
        })
        .collect()
}

/// Splits a markdown note into chunks no larger than `max_tokens` (approximate), breaking at
/// headings first and then at blank lines. Never returns an empty chunk. A note with no content
/// yields no chunks.
pub fn chunk_markdown(body: &str, max_tokens: usize) -> Vec<String> {
    let max_chars = max_tokens.max(1) * CHARS_PER_TOKEN;
    // 1. sections at headings
    let mut sections: Vec<String> = Vec::new();
    let mut cur = String::new();
    for line in body.lines() {
        if line.starts_with('#') && !cur.trim().is_empty() {
            sections.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        sections.push(cur);
    }
    // 2. split oversize sections on paragraphs, then hard-split any oversize paragraph
    let mut out = Vec::new();
    for s in sections {
        if s.len() <= max_chars {
            out.push(s.trim().to_string());
            continue;
        }
        let mut acc = String::new();
        for para in s.split("\n\n") {
            for piece in hard_split(para, max_chars) {
                if !acc.is_empty() && acc.len() + piece.len() + 2 > max_chars {
                    out.push(std::mem::take(&mut acc).trim().to_string());
                }
                if !acc.is_empty() {
                    acc.push_str("\n\n");
                }
                acc.push_str(&piece);
            }
        }
        if !acc.trim().is_empty() {
            out.push(acc.trim().to_string());
        }
    }
    out.retain(|c| !c.is_empty());
    out
}

fn hard_split(s: &str, max_chars: usize) -> Vec<String> {
    if s.len() <= max_chars {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if cur.len() + ch.len_utf8() > max_chars {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edge_producers_prefer_episodes_then_belief_provenance() {
        let both = json!({"episodes":["ep1"],"belief":{"producing_episode_uuids":["ep2"]}});
        assert_eq!(edge_producers(&both), vec!["ep1"]);
        let belief_only = json!({"episodes":null,"episode_uuids":["ep9"],
            "belief":{"producing_episode_uuids":["ep2","ep3"]}});
        assert_eq!(edge_producers(&belief_only), vec!["ep2", "ep3"]);
        assert!(edge_producers(&json!({"episode_uuids":["ep9"]})).is_empty());
    }

    #[test]
    fn source_description_maps_to_note() {
        assert_eq!(
            note_of_source_description("my-note:my-note#0"),
            Some("my-note")
        );
        assert_eq!(note_of_source_description("nocolon"), None);
        assert_eq!(note_of_source_description(":x"), None);
    }

    #[test]
    fn passages_map_in_rank_order_with_duplicates() {
        let p = vec![
            json!({"content":"c1","source_description":"a:a#0"}),
            json!({"content":"c2","source_description":"b:b#0"}),
            json!({"content":"c3","source_description":"a:a#1"}),
        ];
        let (notes, ev) = map_passages(&p);
        assert_eq!(notes, vec!["a", "b", "a"]);
        assert_eq!(ev, vec!["c1", "c2", "c3"]);
    }

    #[test]
    fn entities_expand_to_all_their_notes() {
        let mut m = HashMap::new();
        m.insert(
            "e1".to_string(),
            ["b", "a"].iter().map(|s| s.to_string()).collect(),
        );
        let nodes = vec![
            json!({"uuid":"e1","name":"Acme","summary":"a firm"}),
            json!({"uuid":"e2","name":"Zed","summary":""}),
        ];
        let (notes, ev) = map_entities(&nodes, &m);
        assert_eq!(notes, vec!["a", "b"]);
        assert_eq!(ev, vec!["Acme: a firm", "Zed"]);
    }

    #[test]
    fn edges_map_through_producing_episodes_only() {
        let mut m = HashMap::new();
        m.insert("ep1".to_string(), "a".to_string());
        m.insert("ep2".to_string(), "b".to_string());
        let edges = vec![
            json!({"uuid":"f1","fact":"X owns Y","episodes":["ep1"],"episode_uuids":["ep2"]}),
            json!({"uuid":"f2","fact":"no provenance"}),
        ];
        let r = map_edges(&edges, &m);
        assert_eq!(
            r.notes,
            vec!["a"],
            "episode_uuids (either-endpoint) must be ignored"
        );
        assert_eq!(r.evidence, vec!["X owns Y", "no provenance"]);
        assert_eq!(r.unmapped, vec!["f2"]);
    }

    #[test]
    fn chunker_splits_on_headings_and_respects_cap() {
        let body = "# One\nalpha\n\n## Two\nbeta\n";
        let c = chunk_markdown(body, 900);
        assert_eq!(c.len(), 2);
        assert!(c[0].starts_with("# One") && c[1].starts_with("## Two"));

        let long = format!("# T\n{}", "word ".repeat(2000));
        let c = chunk_markdown(&long, 100);
        assert!(c.len() > 1);
        assert!(
            c.iter().all(|x| x.len() <= 100 * CHARS_PER_TOKEN + 8),
            "{:?}",
            c.iter().map(|x| x.len()).collect::<Vec<_>>()
        );
        assert!(c.iter().all(|x| !x.is_empty()));
        assert!(chunk_markdown("   \n\n", 100).is_empty());
    }
}
