// Tests for the native ART secondary indexes on `Episodic.name` and `Episodic.doc_id`
// (issue #291, ADR-0291) that replace the O(episodes-in-group) scans behind
// `knowledge_process_chunk`'s chunk_id lookup and the `doc_id` existence/uniqueness checks.
//
// Covers:
//   Spike (Task 1): EXPLAIN shows the ART index for every real lookup query shape.
//   FR-004..FR-008 (satisfied by DB maintenance): create, supersede, each remove path, group
//     purge, reopen and WAL replay all leave the lookups equal to a direct, unindexed query.
//   FR-003/FR-010: group isolation and multiple episodes per name.

use lcg_core::{Conn, Db, EpisodicRow, WalReplayer};
use tempfile::TempDir;

const DIM: usize = 4;
const GROUP: &str = "g1";
const OTHER: &str = "g2";
const TS: &str = "2026-01-01 00:00:00";
const AS_OF: &str = "2099-01-01 00:00:00";

fn open_db(dir: &TempDir) -> Db {
    let db = Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap();
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_episodic_lookup_indexes().unwrap();
    }
    db
}

fn ep(uuid: &str, name: &str, group: &str, doc_id: Option<&str>) -> EpisodicRow {
    EpisodicRow {
        uuid: uuid.to_string(),
        name: name.to_string(),
        group_id: group.to_string(),
        created_at: TS.to_string(),
        source: "text".to_string(),
        source_description: "t".to_string(),
        content: format!("content of {uuid}"),
        content_embedding: vec![0.1; DIM],
        valid_at: TS.to_string(),
        entity_edges: vec![],
        attributes: "{}".to_string(),
        doc_id: doc_id.map(str::to_string),
        ..Default::default()
    }
}

fn plan(conn: &Conn, q: &str) -> String {
    conn.cypher_query(&format!("EXPLAIN {q}"))
        .unwrap()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// Direct, plan-independent oracle: every uuid in `group` whose name is `name`, via a
/// full-table scan that ignores the index (a `lower()`-wrapped predicate cannot use ART).
fn oracle_by_name(conn: &Conn, group: &str, name: &str) -> Vec<String> {
    let rows = conn
        .cypher_query(&format!(
            "MATCH (ep:Episodic) WHERE lower(ep.name) = lower('{name}') \
             AND ep.group_id = '{group}' RETURN ep.uuid"
        ))
        .unwrap();
    sorted(rows.into_iter().map(|r| r[0].clone()).collect())
}

fn oracle_by_doc(conn: &Conn, group: &str, doc: &str) -> Vec<String> {
    let rows = conn
        .cypher_query(&format!(
            "MATCH (ep:Episodic) WHERE lower(ep.doc_id) = lower('{doc}') \
             AND ep.group_id = '{group}' RETURN ep.uuid"
        ))
        .unwrap();
    sorted(rows.into_iter().map(|r| r[0].clone()).collect())
}

// ── Spike: EXPLAIN shows the ART path for every real query shape ───────────────────────

#[test]
fn explain_shows_art_for_name_lookup_shapes() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();
    conn.insert_episodic(&ep("u1", "chunk-1", GROUP, Some("doc-1")))
        .unwrap();

    // live_episode_uuids_for_chunk
    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.name = 'chunk-1' AND ep.group_id = 'g1' \
         AND (ep.superseded_at IS NULL \
              OR (ep.superseded_by_kind = 'document' AND ep.superseded_at > timestamp('2099-01-01 00:00:00'))) \
         RETURN ep.uuid",
    );
    assert!(p.contains("ART"), "live_episode_uuids_for_chunk plan: {p}");

    // remove_episodes_by_chunk_id
    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.name = 'chunk-1' AND ep.group_id IN ['g1'] RETURN ep.uuid",
    );
    assert!(p.contains("ART"), "remove_episodes_by_chunk_id plan: {p}");

    // get_episode_rows_by_name
    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.name = 'chunk-1' AND ep.group_id = 'g1' RETURN ep.uuid",
    );
    assert!(p.contains("ART"), "get_episode_rows_by_name plan: {p}");
}

#[test]
fn explain_shows_art_for_doc_id_lookup_shapes() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();
    conn.insert_episodic(&ep("u1", "chunk-1", GROUP, Some("doc-1")))
        .unwrap();

    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.group_id = 'g1' AND ep.doc_id = 'doc-1' \
         RETURN ep.uuid LIMIT 1",
    );
    assert!(p.contains("ART"), "doc_is_known plan: {p}");

    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.group_id = 'g1' AND ep.doc_id = 'doc-1' \
         AND ep.superseded_at IS NULL RETURN ep.uuid ORDER BY ep.uuid",
    );
    assert!(p.contains("ART"), "live_episode_uuids_for_doc plan: {p}");
}

/// Negative control: without the indexes the same shapes show no ART access path, so the
/// positive assertions above are not trivially satisfied by an unrelated "ART" substring.
#[test]
fn explain_shows_no_art_without_the_indexes() {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.init_schema(DIM).unwrap();
    conn.insert_episodic(&ep("u1", "chunk-1", GROUP, Some("doc-1")))
        .unwrap();

    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.name = 'chunk-1' AND ep.group_id = 'g1' RETURN ep.uuid",
    );
    assert!(
        !p.contains("ART"),
        "unindexed plan unexpectedly has ART: {p}"
    );
    let p = plan(
        &conn,
        "MATCH (ep:Episodic) WHERE ep.group_id = 'g1' AND ep.doc_id = 'doc-1' RETURN ep.uuid",
    );
    assert!(
        !p.contains("ART"),
        "unindexed plan unexpectedly has ART: {p}"
    );
}

#[test]
fn null_doc_id_and_long_name_are_tolerated() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();

    let long = "n".repeat(4000);
    conn.insert_episodic(&ep("u-null", "short", GROUP, None))
        .unwrap();
    conn.insert_episodic(&ep("u-long", &long, GROUP, Some("d")))
        .unwrap();

    assert_eq!(
        conn.live_episode_uuids_for_chunk(&long, GROUP, &[], AS_OF)
            .unwrap(),
        vec!["u-long".to_string()]
    );
    assert!(conn
        .live_episode_uuids_for_chunk("short", GROUP, &[], AS_OF)
        .unwrap()
        .contains(&"u-null".to_string()));
    assert!(!conn.doc_is_known(GROUP, "nope").unwrap());
}

// ── Correctness through the public lookups ─────────────────────────────────────────────

#[test]
fn lookups_match_oracle_with_group_isolation_and_multiple_per_name() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();

    conn.insert_episodic(&ep("a1", "chunk-1", GROUP, Some("doc-1")))
        .unwrap();
    conn.insert_episodic(&ep("a2", "chunk-1", GROUP, None))
        .unwrap();
    conn.insert_episodic(&ep("b1", "chunk-1", OTHER, Some("doc-1")))
        .unwrap();

    let got = sorted(
        conn.live_episode_uuids_for_chunk("chunk-1", GROUP, &[], AS_OF)
            .unwrap(),
    );
    assert_eq!(got, vec!["a1".to_string(), "a2".to_string()]);
    assert_eq!(got, oracle_by_name(&conn, GROUP, "chunk-1"));
    assert_eq!(
        conn.live_episode_uuids_for_chunk("chunk-1", OTHER, &[], AS_OF)
            .unwrap(),
        vec!["b1".to_string()]
    );
    assert!(conn
        .live_episode_uuids_for_chunk("missing", GROUP, &[], AS_OF)
        .unwrap()
        .is_empty());

    assert!(conn.doc_is_known(GROUP, "doc-1").unwrap());
    assert!(conn.doc_is_known(OTHER, "doc-1").unwrap());
    assert!(!conn.doc_is_known(GROUP, "doc-2").unwrap());
    assert_eq!(
        conn.live_episode_uuids_for_doc(GROUP, "doc-1").unwrap(),
        oracle_by_doc(&conn, GROUP, "doc-1")
    );
    assert_eq!(
        conn.live_episode_uuids_for_doc(OTHER, "doc-1").unwrap(),
        vec!["b1".to_string()]
    );

    let rows = conn.get_episode_rows_by_name(GROUP, "chunk-1").unwrap();
    assert_eq!(rows.len(), 2);
}

#[test]
fn removal_paths_leave_no_stale_lookup_results() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();

    for (u, n, d) in [
        ("r1", "c-remove-chunk", "d-remove-chunk"),
        ("r2", "c-remove-one", "d-remove-one"),
        ("r3", "c-remove-uuid", "d-remove-uuid"),
    ] {
        let mut row = ep(u, n, GROUP, Some(d));
        row.source_description = format!("src-{u}");
        conn.insert_episodic(&row).unwrap();
    }

    conn.remove_episodes_by_chunk_id("c-remove-chunk", &[GROUP])
        .unwrap();
    assert!(conn
        .live_episode_uuids_for_chunk("c-remove-chunk", GROUP, &[], AS_OF)
        .unwrap()
        .is_empty());
    assert!(!conn.doc_is_known(GROUP, "d-remove-chunk").unwrap());

    conn.remove_episode("r2").unwrap();
    assert!(conn
        .live_episode_uuids_for_chunk("c-remove-one", GROUP, &[], AS_OF)
        .unwrap()
        .is_empty());
    assert!(!conn.doc_is_known(GROUP, "d-remove-one").unwrap());

    conn.remove_episodes_by_source("src-r3", &[GROUP]).unwrap();
    assert!(conn
        .live_episode_uuids_for_chunk("c-remove-uuid", GROUP, &[], AS_OF)
        .unwrap()
        .is_empty());
    assert!(!conn.doc_is_known(GROUP, "d-remove-uuid").unwrap());
}

#[test]
fn group_purge_then_recreate_leaves_no_stale_entries() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();

    conn.insert_episodic(&ep("p1", "chunk-p", GROUP, Some("doc-p")))
        .unwrap();
    conn.delete_episodics_by_group_ids(&[GROUP]).unwrap();
    assert!(conn
        .live_episode_uuids_for_chunk("chunk-p", GROUP, &[], AS_OF)
        .unwrap()
        .is_empty());
    assert!(!conn.doc_is_known(GROUP, "doc-p").unwrap());

    conn.insert_episodic(&ep("p2", "chunk-p", GROUP, Some("doc-p")))
        .unwrap();
    assert_eq!(
        conn.live_episode_uuids_for_chunk("chunk-p", GROUP, &[], AS_OF)
            .unwrap(),
        vec!["p2".to_string()]
    );
}

#[test]
fn supersession_is_reflected_and_survives_reopen() {
    let dir = TempDir::new().unwrap();
    {
        let db = open_db(&dir);
        let conn = db.connect().unwrap();
        conn.insert_episodic(&ep("s1", "chunk-s", GROUP, Some("doc-s")))
            .unwrap();
        conn.insert_episodic(&ep("s2", "chunk-s", GROUP, Some("doc-s")))
            .unwrap();
        assert_eq!(
            sorted(
                conn.live_episode_uuids_for_chunk("chunk-s", GROUP, &[], AS_OF)
                    .unwrap()
            ),
            vec!["s1".to_string(), "s2".to_string()]
        );
        conn.cypher_query(&format!(
            "MATCH (ep:Episodic {{uuid: 's1'}}) SET ep.superseded_at = timestamp('{TS}')"
        ))
        .unwrap();
        assert_eq!(
            conn.live_episode_uuids_for_chunk("chunk-s", GROUP, &[], AS_OF)
                .unwrap(),
            vec!["s2".to_string()]
        );
    }

    // Reopen: the index is database-persisted, lookups stay correct, and re-creating is a no-op.
    let db = Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.create_episodic_lookup_indexes().unwrap();
    assert_eq!(
        conn.live_episode_uuids_for_chunk("chunk-s", GROUP, &[], AS_OF)
            .unwrap(),
        vec!["s2".to_string()]
    );
    assert_eq!(
        conn.get_episode_rows_by_name(GROUP, "chunk-s")
            .unwrap()
            .len(),
        2
    );
    assert!(conn.doc_is_known(GROUP, "doc-s").unwrap());
}

#[test]
fn index_creation_is_idempotent_and_backfills_existing_rows() {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("test.db").to_str().unwrap()).unwrap();
    let conn = db.connect().unwrap();
    conn.init_schema(DIM).unwrap();

    // Rows written before any index exists (an existing workspace).
    conn.insert_episodic(&ep("e1", "old-chunk", GROUP, Some("old-doc")))
        .unwrap();

    conn.create_episodic_lookup_indexes().unwrap();
    conn.create_episodic_lookup_indexes().unwrap();

    assert_eq!(
        conn.live_episode_uuids_for_chunk("old-chunk", GROUP, &[], AS_OF)
            .unwrap(),
        vec!["e1".to_string()]
    );
    assert!(conn.doc_is_known(GROUP, "old-doc").unwrap());
}

/// WAL replay executes raw Cypher against the same table (bypassing `insert_episodic`), so
/// replayed rows are covered by the DB-maintained indexes with no backfill (FR-006).
#[test]
fn wal_replayed_episodes_are_found_by_indexed_lookups() {
    let dir = TempDir::new().unwrap();
    let db = open_db(&dir);
    let conn = db.connect().unwrap();

    let wal_dir = TempDir::new().unwrap();
    let line = r#"{"seq":0,"ts":"2026-05-19T03:00:00.000000+00:00","db":"","cypher":"CREATE (:Episodic {uuid: 'replayed-1', name: 'replayed-chunk', group_id: 'g1', created_at: timestamp('2026-05-19 00:00:00'), source: 'text', source_description: 's', content: 'c', content_embedding: [0.1, 0.1, 0.1, 0.1], valid_at: timestamp('2026-05-19 00:00:00'), entity_edges: [], attributes: '{}', doc_id: 'replayed-doc'})","params":{}}"#;
    std::fs::write(
        wal_dir.path().join("20260519_030000_aaa111_0000.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();

    WalReplayer::new(wal_dir.path())
        .replay(&conn, lcg_core::zero_vector_embed_fn(DIM), DIM)
        .unwrap();
    assert_eq!(conn.count_nodes("Episodic").unwrap(), 1);

    assert_eq!(
        conn.live_episode_uuids_for_chunk("replayed-chunk", GROUP, &[], AS_OF)
            .unwrap(),
        vec!["replayed-1".to_string()]
    );
    assert!(conn.doc_is_known(GROUP, "replayed-doc").unwrap());
    assert_eq!(
        conn.live_episode_uuids_for_doc(GROUP, "replayed-doc")
            .unwrap(),
        oracle_by_doc(&conn, GROUP, "replayed-doc")
    );
}
