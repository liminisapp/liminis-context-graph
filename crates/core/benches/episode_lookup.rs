//! Benchmark for issue #291 / ADR-0291: the `Episodic.name` (chunk_id) and `Episodic.doc_id`
//! lookups behind `knowledge_process_chunk` and document supersession.
//!
//! The "scan baseline" issues a `lower(...)`-wrapped predicate, which lbug cannot route through
//! an ART index, so it measures the pre-#291 full `Episodic` scan regardless of what the indexed
//! implementation does. The "indexed" benches call the real lookup functions. Group sizes of 1k,
//! 4k and 10k show that the indexed cost stays flat while the scan grows with the group
//! (FR-015, SC-001..SC-003). Run on demand: `gh workflow run bench.yml`.

use criterion::{criterion_group, criterion_main, Criterion};
use lcg_core::{Db, EpisodicRow};
use std::sync::Arc;

const GROUP: &str = "bench";
const AS_OF: &str = "2099-01-01 00:00:00";

fn setup(n: usize) -> (Arc<Db>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("bench.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(8).unwrap();
        conn.create_episodic_lookup_indexes().unwrap();
        for i in 0..n {
            conn.insert_episodic(&EpisodicRow {
                uuid: format!("ep{i:06}"),
                name: format!("chunk-{i}"),
                group_id: GROUP.to_string(),
                created_at: "2026-01-01 00:00:00".to_string(),
                source: "text".to_string(),
                source_description: "bench".to_string(),
                content: format!("content {i}"),
                content_embedding: vec![0.1; 8],
                valid_at: "2026-01-01 00:00:00".to_string(),
                attributes: "{}".to_string(),
                doc_id: Some(format!("doc-{i}")),
                ..Default::default()
            })
            .unwrap();
        }
    }
    (db, dir)
}

fn bench_episode_lookup(c: &mut Criterion) {
    for n in [1_000usize, 4_000, 10_000] {
        let (db, _dir) = setup(n);
        let last = n - 1;

        c.bench_function(&format!("episode_chunk_scan_baseline_{n}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                let rows = conn
                    .cypher_query(&format!(
                        "MATCH (ep:Episodic) WHERE lower(ep.name) = 'chunk-{last}' \
                         AND ep.group_id = '{GROUP}' AND ep.superseded_at IS NULL RETURN ep.uuid"
                    ))
                    .unwrap();
                assert_eq!(rows.len(), 1);
            });
        });

        c.bench_function(&format!("episode_chunk_indexed_hit_{n}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                let r = conn
                    .live_episode_uuids_for_chunk(&format!("chunk-{last}"), GROUP, &[], AS_OF)
                    .unwrap();
                assert_eq!(r.len(), 1);
            });
        });

        // The common first-time-ingest case: a never-seen chunk_id.
        c.bench_function(&format!("episode_chunk_indexed_miss_{n}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                let r = conn
                    .live_episode_uuids_for_chunk("never-seen", GROUP, &[], AS_OF)
                    .unwrap();
                assert!(r.is_empty());
            });
        });

        c.bench_function(&format!("episode_doc_scan_baseline_{n}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                let rows = conn
                    .cypher_query(&format!(
                        "MATCH (ep:Episodic) WHERE lower(ep.doc_id) = 'doc-{last}' \
                         AND ep.group_id = '{GROUP}' RETURN ep.uuid LIMIT 1"
                    ))
                    .unwrap();
                assert_eq!(rows.len(), 1);
            });
        });

        c.bench_function(&format!("episode_doc_indexed_hit_{n}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                assert!(conn.doc_is_known(GROUP, &format!("doc-{last}")).unwrap());
            });
        });

        c.bench_function(&format!("episode_doc_indexed_miss_{n}"), |b| {
            b.iter(|| {
                let conn = db.connect().unwrap();
                assert!(!conn.doc_is_known(GROUP, "no-such-doc").unwrap());
            });
        });
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench_episode_lookup
}
criterion_main!(benches);
