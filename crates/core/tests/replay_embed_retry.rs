//! End-to-end tests for issue #808: WAL replay embedding recompute retries transient embedder
//! failures with backoff, isolates a poison text to its own row, and durably records the rows
//! that still end up with a fallback zero vector (ADR-0808).

use lcg_core::replay_embed_retry::EmbedRetryPolicy;
use lcg_core::zero_vector_record::ZeroKind;
use lcg_core::{Db, Error, RecomputeEmbedFn, ReplayOptions, ReplayStats, WalReplayer};
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

const GROUP: &str = "g1";
const DIM: usize = 4;

/// Millisecond-scale policy so retried tests run fast.
fn fast_policy(retries: u32) -> EmbedRetryPolicy {
    EmbedRetryPolicy {
        retries,
        base_ms: 1,
        cap_ms: 2,
    }
}

fn timeout_err() -> Error {
    Error::Ipc("UDS send request: timed out after 30000ms".to_string())
}

fn status_err(code: &str) -> Error {
    Error::Ipc(format!("UDS embedder returned status {code}"))
}

fn real_vec(text: &str) -> Vec<f32> {
    vec![text.len() as f32 + 1.0, 1.0, 1.0, 1.0]
}

/// `MERGE ... SET` template carrying one name embedding — SET form so a replay onto an existing
/// node rewrites its embedding.
const NAME_TEMPLATE: &str = "MERGE (n:Entity {uuid: $uuid}) SET n.name = $name, \
     n.group_id = $group_id, n.name_embedding = $name_embedding";

const TWO_KIND_TEMPLATE: &str = "MERGE (n:Entity {uuid: $uuid}) SET n.name = $name, \
     n.group_id = $group_id, n.summary = $summary, n.name_embedding = $name_embedding, \
     n.summary_embedding = $summary_embedding";

fn wal_line(seq: u64, cypher: &str, params: serde_json::Value) -> String {
    serde_json::json!({
        "seq": seq,
        "ts": "2026-05-19T03:00:00.000000+00:00",
        "db": "",
        "cypher": cypher,
        "params": params,
    })
    .to_string()
}

fn entity_params(i: usize, name: &str) -> serde_json::Value {
    serde_json::json!({ "uuid": format!("u{i}"), "name": name, "group_id": GROUP })
}

fn write_wal(wal_dir: &std::path::Path, lines: &[String]) {
    fs::create_dir_all(wal_dir).unwrap();
    fs::write(
        wal_dir.join("20260519_030000_aaa111_0000.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
}

struct Fixture {
    _root: TempDir,
    db_path: String,
    wal_dir: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let db_path = root.path().join("graph.db").to_str().unwrap().to_string();
        let wal_dir = root.path().join("wal");
        Fixture {
            _root: root,
            db_path,
            wal_dir,
        }
    }

    fn open(&self) -> Db {
        let db = Db::open(&self.db_path).unwrap();
        db.connect().unwrap().init_schema(DIM).unwrap();
        db
    }
}

fn replay(
    fx: &Fixture,
    db: &Db,
    embed: RecomputeEmbedFn,
    policy: EmbedRetryPolicy,
    window: usize,
    from_seq: u64,
) -> ReplayStats {
    let conn = db.connect().unwrap();
    WalReplayer::new(&fx.wal_dir)
        .replay_opts(
            &conn,
            ReplayOptions {
                from_seq,
                embed_window_size: Some(window),
                embed_retry: Some(policy),
                group_id: Some(GROUP.to_string()),
                ..ReplayOptions::new(embed, DIM)
            },
        )
        .expect("replay")
}

fn stored_vec(db: &Db, uuid: &str) -> Vec<f64> {
    let conn = db.connect().unwrap();
    let rows = conn
        .cypher_query(&format!(
            "MATCH (n:Entity {{uuid: '{uuid}'}}) RETURN n.name_embedding"
        ))
        .unwrap();
    rows[0][0]
        .trim_matches(|c: char| c == '[' || c == ']')
        .split(',')
        .map(|s| s.trim().parse::<f64>().unwrap())
        .collect()
}

fn n_rows(n: usize, name_of: impl Fn(usize) -> String) -> Vec<String> {
    (0..n)
        .map(|i| wal_line(i as u64, NAME_TEMPLATE, entity_params(i, &name_of(i))))
        .collect()
}

// AC1: intermittent timeouts / 503s that succeed within the retry budget leave nothing failed.
#[test]
fn transient_failures_within_budget_leave_no_zero_vectors() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(64, |i| format!("entity-{i}")));
    let db = fx.open();

    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let embed: RecomputeEmbedFn =
        Box::new(
            move |texts: &[&str]| match c.fetch_add(1, Ordering::SeqCst) {
                0 => Err(timeout_err()),
                1 => Err(status_err("503 Service Unavailable")),
                2 => Err(status_err("429 Too Many Requests")),
                _ => Ok(texts.iter().map(|t| real_vec(t)).collect()),
            },
        );
    let stats = replay(&fx, &db, embed, fast_policy(4), 64, 0);

    assert_eq!(stats.embeddings_recompute_failed, 0);
    assert_eq!(stats.embeddings_recomputed, 64);
    assert_eq!(stats.lines_replayed, 64);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "3 transient failures + 1 success"
    );
    let conn = db.connect().unwrap();
    assert_eq!(
        conn.count_nodes("Entity").unwrap(),
        64,
        "each row stored exactly once"
    );
    assert_eq!(conn.zero_vector_counts(GROUP).unwrap().total(), 0);
    assert_eq!(stored_vec(&db, "u7"), vec![9.0, 1.0, 1.0, 1.0]); // "entity-7" len 8 + 1
}

// AC2: one text that always fails, in a 64-row window, fails exactly its own row.
#[test]
fn poison_text_in_64_row_window_fails_only_its_own_row() {
    for poison_err in [
        status_err("503 Service Unavailable"),
        status_err("400 Bad Request"),
    ] {
        let fx = Fixture::new();
        write_wal(
            &fx.wal_dir,
            &n_rows(64, |i| {
                if i == 40 {
                    "POISON".to_string()
                } else {
                    format!("entity-{i}")
                }
            }),
        );
        let db = fx.open();
        let msg = poison_err.to_string();
        let embed: RecomputeEmbedFn = Box::new(move |texts: &[&str]| {
            if texts.contains(&"POISON") {
                Err(Error::Ipc(
                    msg.trim_start_matches("IPC error: ").to_string(),
                ))
            } else {
                Ok(texts.iter().map(|t| real_vec(t)).collect())
            }
        });
        let stats = replay(&fx, &db, embed, fast_policy(4), 64, 0);

        assert_eq!(stats.embeddings_recompute_failed, 1, "{poison_err}");
        assert_eq!(stats.embeddings_recomputed, 63);
        let conn = db.connect().unwrap();
        assert_eq!(conn.count_nodes("Entity").unwrap(), 64);
        assert_eq!(
            conn.list_zero_vector_rows(GROUP, None).unwrap(),
            vec![(ZeroKind::Name, "u40".to_string())]
        );
        assert_eq!(stored_vec(&db, "u40"), vec![0.0; DIM]);
        assert_eq!(stored_vec(&db, "u3"), vec![9.0, 1.0, 1.0, 1.0]);
    }
}

// AC4: a non-transient error is attempted exactly once per request — never retried.
#[test]
fn non_transient_error_is_not_retried() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(1, |_| "only".to_string()));
    let db = fx.open();
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let embed: RecomputeEmbedFn = Box::new(move |_t: &[&str]| {
        c.fetch_add(1, Ordering::SeqCst);
        Err(status_err("400 Bad Request"))
    });
    let stats = replay(&fx, &db, embed, fast_policy(4), 64, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(stats.embeddings_recompute_failed, 1);
}

// A single request that exhausts its retries is not retried further or split.
#[test]
fn single_request_exhausting_retries_fails_after_budgeted_attempts() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(1, |_| "only".to_string()));
    let db = fx.open();
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let embed: RecomputeEmbedFn = Box::new(move |_t: &[&str]| {
        c.fetch_add(1, Ordering::SeqCst);
        Err(timeout_err())
    });
    let stats = replay(&fx, &db, embed, fast_policy(4), 64, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    assert_eq!(stats.embeddings_recompute_failed, 1);
}

// AC3: after retry exhaustion on more than 10 requests the exact count and per-kind breakdown are
// durable — past the 10-row log cap, across a restart.
#[test]
fn failures_beyond_log_cap_are_counted_exactly_and_survive_restart() {
    let fx = Fixture::new();
    let lines: Vec<String> = (0..25)
        .map(|i| {
            wal_line(
                i as u64,
                TWO_KIND_TEMPLATE,
                serde_json::json!({
                    "uuid": format!("u{i}"), "name": format!("bad-{i}"),
                    "summary": format!("bad-summary-{i}"), "group_id": GROUP,
                }),
            )
        })
        .collect();
    write_wal(&fx.wal_dir, &lines);
    {
        let db = fx.open();
        let embed: RecomputeEmbedFn = Box::new(|t: &[&str]| {
            if t.iter().any(|x| x.starts_with("bad")) {
                Err(status_err("500 Internal Server Error"))
            } else {
                Ok(t.iter().map(|x| real_vec(x)).collect())
            }
        });
        let stats = replay(&fx, &db, embed, fast_policy(1), 8, 0);
        assert_eq!(stats.embeddings_recompute_failed, 50);
        let counts = db.connect().unwrap().zero_vector_counts(GROUP).unwrap();
        assert_eq!(counts.total(), 50);
        assert_eq!((counts.name, counts.summary), (25, 25));
        assert_eq!((counts.fact, counts.content), (0, 0));
    }
    // Restart: a fresh Db handle over the same files reports the same values.
    let db = fx.open();
    let conn = db.connect().unwrap();
    let counts = conn.zero_vector_counts(GROUP).unwrap();
    assert_eq!((counts.total(), counts.name, counts.summary), (50, 25, 25));
    assert_eq!(counts.to_json()["by_kind"]["summary"], 25);
    // Other groups are untouched.
    assert_eq!(conn.zero_vector_counts("other").unwrap().total(), 0);
}

// Retry exhaustion while the embedder is simply down: the failure is recorded, bounded work.
#[test]
fn dead_embedder_replay_completes_and_records_every_failed_row() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(20, |i| format!("e{i}")));
    let db = fx.open();
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let embed: RecomputeEmbedFn = Box::new(move |_t: &[&str]| {
        c.fetch_add(1, Ordering::SeqCst);
        Err(timeout_err())
    });
    let stats = replay(&fx, &db, embed, fast_policy(4), 8, 0);
    assert_eq!(stats.embeddings_recompute_failed, 20);
    assert_eq!(
        db.connect()
            .unwrap()
            .zero_vector_counts(GROUP)
            .unwrap()
            .name,
        20
    );
    // 3 windows (8+8+4). The first is bounded by its call budget; once everything failed
    // transiently the breaker limits each later window to a single attempt.
    let budget = fast_policy(4).call_budget(8);
    assert!(
        calls.load(Ordering::SeqCst) <= budget + 2,
        "calls={} budget={budget}",
        calls.load(Ordering::SeqCst)
    );
}

// FR-011: a later successful embed (incremental replay) clears a stale entry.
#[test]
fn later_successful_recompute_clears_the_stale_entry() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(3, |i| format!("entity-{i}")));
    let db = fx.open();
    let failing: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    replay(&fx, &db, failing, fast_policy(0), 64, 0);
    assert_eq!(
        db.connect()
            .unwrap()
            .zero_vector_counts(GROUP)
            .unwrap()
            .total(),
        3
    );

    let healthy: RecomputeEmbedFn =
        Box::new(|t: &[&str]| Ok(t.iter().map(|x| real_vec(x)).collect()));
    // Re-replay only seq >= 1: rows 1 and 2 are repaired, row 0 keeps its entry.
    let stats = replay(&fx, &db, healthy, fast_policy(4), 64, 1);
    assert_eq!(stats.embeddings_recompute_failed, 0);
    let conn = db.connect().unwrap();
    assert_eq!(
        conn.list_zero_vector_rows(GROUP, None).unwrap(),
        vec![(ZeroKind::Name, "u0".to_string())]
    );
}

// A row deleted later in the WAL leaves no stale entry — even within the same window.
#[test]
fn deleting_a_recorded_row_clears_its_entry() {
    let fx = Fixture::new();
    let mut lines = n_rows(2, |i| format!("entity-{i}"));
    lines.push(wal_line(
        2,
        "MATCH (n:Entity {uuid: $uuid}) DETACH DELETE n",
        serde_json::json!({ "uuid": "u0" }),
    ));
    write_wal(&fx.wal_dir, &lines);
    let db = fx.open();
    let failing: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    replay(&fx, &db, failing, fast_policy(0), 64, 0);
    let conn = db.connect().unwrap();
    assert_eq!(conn.count_nodes("Entity").unwrap(), 1);
    assert_eq!(
        conn.list_zero_vector_rows(GROUP, None).unwrap(),
        vec![(ZeroKind::Name, "u1".to_string())]
    );
}

// The record is derived, best-effort state: with its table absent, an embedder failure must still
// store the (zero-vector) graph rows rather than roll the batch back over a failed record write.
#[test]
fn missing_record_table_does_not_cost_graph_rows() {
    let fx = Fixture::new();
    let lines = n_rows(3, |i| format!("entity-{i}"));
    write_wal(&fx.wal_dir, &lines);
    let db = fx.open();
    db.connect()
        .unwrap()
        .run_cypher("DROP TABLE EmbeddingZeroRow")
        .unwrap();
    let failing: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    let stats = replay(&fx, &db, failing, fast_policy(0), 64, 0);
    assert_eq!(stats.embeddings_recompute_failed, 3);
    assert_eq!(stats.transactions_rolled_back, 0);
    assert_eq!(db.connect().unwrap().count_nodes("Entity").unwrap(), 3);
}

// A bulk delete carrying a `uuids` list (`WHERE n.uuid IN $uuids`) clears every listed entry.
#[test]
fn bulk_delete_with_uuids_list_clears_entries() {
    let fx = Fixture::new();
    let mut lines = n_rows(3, |i| format!("entity-{i}"));
    lines.push(wal_line(
        3,
        "MATCH (n:Entity) WHERE n.uuid IN $uuids DETACH DELETE n",
        serde_json::json!({ "uuids": ["u0", "u2"] }),
    ));
    write_wal(&fx.wal_dir, &lines);
    let db = fx.open();
    let failing: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    replay(&fx, &db, failing, fast_policy(0), 64, 0);
    let conn = db.connect().unwrap();
    assert_eq!(conn.count_nodes("Entity").unwrap(), 1);
    assert_eq!(
        conn.list_zero_vector_rows(GROUP, None).unwrap(),
        vec![(ZeroKind::Name, "u1".to_string())]
    );
}

// FR-009 / #526: rows with no source text, and skipped vector-only SETs, are never recorded.
#[test]
fn no_text_and_skipped_rows_are_not_recorded() {
    let fx = Fixture::new();
    let lines = vec![
        // CREATE-type row with no `name` text: zero-filled, legitimately.
        wal_line(
            0,
            "MERGE (n:Entity {uuid: $uuid}) SET n.group_id = $group_id, \
             n.name_embedding = $name_embedding",
            serde_json::json!({ "uuid": "no-text", "group_id": GROUP }),
        ),
        // Vector-only SET with no text: skipped entirely.
        wal_line(
            1,
            "MATCH (n:Entity {uuid: $uuid}) SET n.name_embedding = $name_embedding",
            serde_json::json!({ "uuid": "no-text" }),
        ),
    ];
    write_wal(&fx.wal_dir, &lines);
    let db = fx.open();
    let embed: RecomputeEmbedFn = Box::new(|_t: &[&str]| panic!("no text → no embedder call"));
    let stats = replay(&fx, &db, embed, fast_policy(4), 64, 0);
    assert_eq!(stats.embeddings_recompute_skipped_no_text, 2);
    assert_eq!(stats.embeddings_skip_rows, 1);
    assert_eq!(stats.embeddings_recompute_failed, 0);
    assert_eq!(
        db.connect()
            .unwrap()
            .zero_vector_counts(GROUP)
            .unwrap()
            .total(),
        0
    );
}

// A vector-only SET *with* text whose embed fails is skipped (never written) → not recorded.
#[test]
fn failed_vector_only_set_is_skipped_and_not_recorded() {
    let fx = Fixture::new();
    let lines = vec![
        wal_line(0, NAME_TEMPLATE, entity_params(0, "alpha")),
        wal_line(
            1,
            "MATCH (n:Entity {uuid: $uuid}) SET n.name_embedding = $name_embedding",
            serde_json::json!({ "uuid": "u0", "name": "alpha" }),
        ),
    ];
    write_wal(&fx.wal_dir, &lines);
    let db = fx.open();
    // Fails everything: row 0 is zero-filled+recorded; row 1 is skipped, so it adds nothing.
    let embed: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    let stats = replay(&fx, &db, embed, fast_policy(0), 64, 0);
    assert_eq!(stats.embeddings_recompute_failed, 2);
    assert_eq!(stats.embeddings_skip_rows, 1);
    assert_eq!(
        db.connect()
            .unwrap()
            .list_zero_vector_rows(GROUP, None)
            .unwrap(),
        vec![(ZeroKind::Name, "u0".to_string())]
    );
}

// Atomicity: a batch that rolls back (here: a template that fails to prepare) leaves no phantom
// entry for the zero vector it would have stored.
#[test]
fn rolled_back_batch_leaves_no_phantom_entry() {
    let fx = Fixture::new();
    let lines = vec![wal_line(
        0,
        "MERGE (n:Entity {uuid: $uuid}) SET n.no_such_column = $name, \
         n.name_embedding = $name_embedding",
        entity_params(0, "alpha"),
    )];
    write_wal(&fx.wal_dir, &lines);
    let db = fx.open();
    let embed: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    let stats = replay(&fx, &db, embed, fast_policy(0), 64, 0);
    assert_eq!(stats.failed_lines, 1);
    let conn = db.connect().unwrap();
    assert_eq!(conn.count_nodes("Entity").unwrap(), 0);
    assert_eq!(conn.zero_vector_counts(GROUP).unwrap().total(), 0);
}

// Cancellation during backoff: nothing is stored, counted or recorded for the window.
#[test]
fn cancel_during_backoff_records_nothing() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(4, |i| format!("e{i}")));
    let db = fx.open();
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let embed: RecomputeEmbedFn = Box::new(move |_t: &[&str]| {
        c.fetch_add(1, Ordering::SeqCst);
        Err(timeout_err())
    });
    let c2 = calls.clone();
    let conn = db.connect().unwrap();
    let stats = WalReplayer::new(&fx.wal_dir)
        .replay_opts(
            &conn,
            ReplayOptions {
                embed_window_size: Some(4),
                embed_retry: Some(fast_policy(4)),
                group_id: Some(GROUP.to_string()),
                cancel_fn: Some(Box::new(move || c2.load(Ordering::SeqCst) >= 1)),
                ..ReplayOptions::new(embed, DIM)
            },
        )
        .unwrap();
    assert_eq!(stats.embeddings_recompute_failed, 0);
    assert_eq!(conn.count_nodes("Entity").unwrap(), 0);
    assert_eq!(conn.zero_vector_counts(GROUP).unwrap().total(), 0);
}

// Without a group_id (group-less callers) nothing is recorded.
#[test]
fn no_group_id_records_nothing() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(2, |i| format!("e{i}")));
    let db = fx.open();
    let conn = db.connect().unwrap();
    let embed: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    let stats = WalReplayer::new(&fx.wal_dir)
        .replay_opts(
            &conn,
            ReplayOptions {
                embed_retry: Some(fast_policy(0)),
                ..ReplayOptions::new(embed, DIM)
            },
        )
        .unwrap();
    assert_eq!(stats.embeddings_recompute_failed, 2);
    assert_eq!(conn.zero_vector_counts_all().unwrap().len(), 0);
}

// An invalid env policy fails the replay up front, naming the variable (AC5, lazy path).
#[test]
fn invalid_retry_env_is_a_config_error_naming_the_variable() {
    let err = EmbedRetryPolicy::from_lookup(|k| {
        (k == "LCG_REPLAY_EMBED_RETRIES").then(|| "many".to_string())
    })
    .unwrap_err();
    assert!(matches!(&err, Error::Config(m) if m.contains("LCG_REPLAY_EMBED_RETRIES")));
}

// FR-010: a full rebuild resets the group's record — the clear-for-rebuild purge drops it, so a
// subsequent replay records only its own failures; other groups' records are untouched.
#[test]
fn purge_for_rebuild_resets_only_that_groups_record() {
    let fx = Fixture::new();
    write_wal(&fx.wal_dir, &n_rows(5, |i| format!("entity-{i}")));
    let db = fx.open();
    let failing: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    replay(&fx, &db, failing, fast_policy(0), 64, 0);
    let conn = db.connect().unwrap();
    // A different group's pre-existing record, written through the replay path itself.
    let other: RecomputeEmbedFn = Box::new(|_t: &[&str]| Err(status_err("400 Bad Request")));
    WalReplayer::new(&fx.wal_dir)
        .replay_opts(
            &conn,
            ReplayOptions {
                from_seq: 4,
                embed_window_size: Some(1),
                embed_retry: Some(fast_policy(0)),
                group_id: Some("other".to_string()),
                ..ReplayOptions::new(other, DIM)
            },
        )
        .unwrap();
    // (u4 was already recorded under GROUP; "other" now holds its own entry for it too.)
    assert_eq!(conn.zero_vector_counts(GROUP).unwrap().total(), 5);

    lcg_core::group_purge::purge_groups(&conn, &[GROUP], "2026-01-01T00:00:00Z", false).unwrap();
    assert_eq!(conn.zero_vector_counts(GROUP).unwrap().total(), 0);
    assert_eq!(conn.zero_vector_counts("other").unwrap().total(), 1);

    // The rebuild's own (smaller) failure set is all that remains.
    let one_bad: RecomputeEmbedFn = Box::new(|t: &[&str]| {
        if t.contains(&"entity-2") {
            Err(status_err("400 Bad Request"))
        } else {
            Ok(t.iter().map(|x| real_vec(x)).collect())
        }
    });
    replay(&fx, &db, one_bad, fast_policy(0), 1, 0);
    assert_eq!(
        conn.list_zero_vector_rows(GROUP, None).unwrap(),
        vec![(ZeroKind::Name, "u2".to_string())]
    );
}
