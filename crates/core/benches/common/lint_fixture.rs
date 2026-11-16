//! Shared `knowledge_lint` scale fixture (issue #755): `n` episodes, each primary with its own
//! `doc_id`, producing one fact between two entities and mentioning both; every tenth is a derived
//! page restating the previous episode's fact. Used by `benches/lint.rs` and the `#[ignore]`d
//! scaling probe in `tests/knowledge_lint.rs`.

use lcg_core::{Db, EntityRow, EpisodicRow, MentionsEdge, RelatesToEdge};
use std::sync::Arc;

pub const DIM: usize = 4;
pub const GROUP: &str = "bench";
const TS: &str = "2026-01-01 00:00:00";

/// A pseudo-random six-letter name per index: unique, and spread over the blocking keys so the
/// near-duplicate pass sees realistic (small) blocks rather than one huge numeric family.
fn scrambled_name(i: usize) -> String {
    let mut x = (i as u64).wrapping_mul(2_654_435_761) % 308_915_776; // 26^6
    let mut s = String::with_capacity(6);
    for _ in 0..6 {
        s.push((b'a' + (x % 26) as u8) as char);
        x /= 26;
    }
    s
}

#[allow(dead_code)]
pub fn seed(n: usize) -> (Arc<Db>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("lint.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        let emb = vec![0.5f32; DIM];
        for i in 0..=n {
            conn.insert_entity(&EntityRow {
                uuid: format!("n{i:06}"),
                name: scrambled_name(i / 2),
                group_id: GROUP.to_string(),
                labels: vec!["Entity".to_string()],
                created_at: TS.to_string(),
                name_embedding: emb.clone(),
                summary: String::new(),
                attributes: "{}".to_string(),
                ..Default::default()
            })
            .unwrap();
        }
        for i in 0..n {
            let derived = i % 10 == 9;
            let ep = format!("p{i:06}");
            conn.insert_episodic(&EpisodicRow {
                uuid: ep.clone(),
                name: format!("chunk-{i}"),
                group_id: GROUP.to_string(),
                created_at: TS.to_string(),
                source: "text".to_string(),
                source_description: "bench".to_string(),
                content: String::new(),
                content_embedding: emb.clone(),
                valid_at: TS.to_string(),
                entity_edges: vec![],
                doc_id: Some(format!("doc{i}")),
                origin: Some(if derived { "derived" } else { "primary" }.to_string()),
                ..Default::default()
            })
            .unwrap();
            // A derived episode restates the previous episode's fact.
            let (src, dst) = if derived { (i - 1, i) } else { (i, i + 1) };
            conn.insert_relates_to_edge(&RelatesToEdge {
                uuid: format!("r{i:06}"),
                name: format!("link {src}"),
                source_node_uuid: format!("n{src:06}"),
                target_node_uuid: format!("n{dst:06}"),
                group_id: GROUP.to_string(),
                fact: format!("fact {i}"),
                fact_embedding: emb.clone(),
                created_at: TS.to_string(),
                attributes: "{}".to_string(),
                relation_type: Some("RELATED_TO".to_string()),
                episodes: vec![ep.clone()],
                ..Default::default()
            })
            .unwrap();
            for e in [src, dst] {
                conn.insert_mentions_edge(&MentionsEdge {
                    episodic_uuid: ep.clone(),
                    entity_uuid: format!("n{e:06}"),
                    group_id: GROUP.to_string(),
                })
                .unwrap();
            }
        }
    }
    (db, dir)
}
