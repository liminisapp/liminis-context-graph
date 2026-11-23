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

/// One returned passage, pre-dedupe, as written to `ranked.jsonl` (`--record-ranked`). Carries
/// the per-hit fields the retriever boundary otherwise drops. Every optional field is absent
/// (never `null`) when the service did not supply it: `similarity` is hybrid-mode only, `legs`
/// needs a service at or after #871. No evidence text is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedHit {
    /// 1-based position in the service's response.
    pub rank: usize,
    pub note: Option<String>,
    pub uuid: Option<String>,
    pub score: Option<f64>,
    pub similarity: Option<f64>,
    pub legs: Option<String>,
}

impl RankedHit {
    pub fn to_json(&self) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("rank".into(), Value::from(self.rank));
        if let Some(n) = &self.note {
            o.insert("note".into(), Value::from(n.clone()));
        }
        if let Some(u) = &self.uuid {
            o.insert("uuid".into(), Value::from(u.clone()));
        }
        if let Some(s) = self.score {
            o.insert("score".into(), Value::from(s));
        }
        if let Some(s) = self.similarity {
            o.insert("similarity".into(), Value::from(s));
        }
        if let Some(l) = &self.legs {
            o.insert("legs".into(), Value::from(l.clone()));
        }
        Value::Object(o)
    }
}

/// Passages to per-hit records, in response order (passage level, pre-dedupe).
pub fn map_hits(passages: &[Value]) -> Vec<RankedHit> {
    passages
        .iter()
        .enumerate()
        .map(|(i, p)| RankedHit {
            rank: i + 1,
            note: p["source_description"]
                .as_str()
                .and_then(note_of_source_description)
                .map(str::to_string),
            uuid: p["uuid"].as_str().map(str::to_string),
            score: p["score"].as_f64(),
            similarity: p["similarity"].as_f64(),
            legs: p["legs"].as_str().map(str::to_string),
        })
        .collect()
}

/// Lexical-only tally over a hybrid passage response (issue #871): hits whose `legs` is `lexical`
/// against all hits returned. A zero-hit response is a zero tally, so the question still counts.
pub fn tally_legs(passages: &[Value]) -> super::LegsTally {
    super::LegsTally {
        lexical_only: passages
            .iter()
            .filter(|p| p["legs"].as_str() == Some("lexical"))
            .count() as u64,
        total: passages.len() as u64,
    }
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
    #[test]
    fn tally_legs_counts_lexical_only_hits_and_keeps_empty_responses_in_the_denominator() {
        use serde_json::json;
        let ps = vec![
            json!({"uuid": "a", "legs": "lexical"}),
            json!({"uuid": "b", "legs": "dense"}),
            json!({"uuid": "c", "legs": "both"}),
            json!({"uuid": "d", "legs": "lexical"}),
        ];
        let t = tally_legs(&ps);
        assert_eq!((t.lexical_only, t.total), (2, 4));
        assert_eq!(t.share(), Some(0.5));
        let empty = tally_legs(&[]);
        assert_eq!((empty.lexical_only, empty.total), (0, 0));
        assert_eq!(empty.share(), None);
        // A response without `legs` (older service) reads as no lexical-only hits.
        assert_eq!(tally_legs(&[json!({"uuid": "x"})]).lexical_only, 0);
    }

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

    #[test]
    fn map_hits_carries_score_similarity_and_legs_when_present() {
        let ps = vec![
            json!({"uuid": "u1", "source_description": "n1:0", "score": 0.5, "similarity": 0.8, "legs": "both"}),
            json!({"uuid": "u2", "source_description": "n2:1", "score": 0.25, "similarity": null, "legs": "lexical"}),
        ];
        let h = map_hits(&ps);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].rank, 1);
        assert_eq!(h[0].note.as_deref(), Some("n1"));
        assert_eq!(h[0].score, Some(0.5));
        assert_eq!(h[0].similarity, Some(0.8));
        assert_eq!(h[0].legs.as_deref(), Some("both"));
        assert_eq!(h[1].similarity, None, "null similarity reads as absent");
        let j = h[1].to_json();
        assert!(j.get("similarity").is_none(), "absent, never null: {j}");
        assert_eq!(j["legs"], "lexical");
    }

    #[test]
    fn map_hits_tolerates_a_service_without_legs_or_scores() {
        let h = map_hits(&[
            json!({"source_description": "n1:0"}),
            json!({"content": "x"}),
        ]);
        assert_eq!(h[0].legs, None);
        assert_eq!(h[0].score, None);
        assert_eq!(h[1].note, None);
        let j = h[0].to_json();
        assert_eq!(j, json!({"rank": 1, "note": "n1"}));
        assert!(map_hits(&[]).is_empty());
    }
}
