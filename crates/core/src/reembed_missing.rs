//! `knowledge_reembed_missing` (issue #809, ADR-0809): repairs the rows that replay stored with a
//! fallback zero vector because the embedder failed (issue #808, ADR-0808), for any of the four
//! embedding kinds — `name`, `summary`, `fact`, `content`.
//!
//! **Selection is the record, never a scan.** Candidates are exactly the `(kind, uuid)` entries
//! [`Conn::list_zero_vector_rows`] returns for the group. Nothing reads a stored vector back to
//! detect a zero (there is no cheap way to).
//!
//! **Phase order is forced by the index constraint.** All four embedding columns carry a
//! database-wide HNSW index and lbug rejects a `SET` on an indexed column (ADR-0379), so the write
//! must happen with the relevant index dropped. The indexes span *all* groups, so while one is down
//! writes are blocked everywhere and vector search for every group falls back to auto-heal. The run
//! therefore keeps that window as small as it can:
//!
//! 1. **Select + classify** under a brief read lock: for each recorded row read its source text from
//!    the same row. A row that no longer exists is *stale* (its record entry is removed, counted
//!    `skipped_no_source`); a row with empty/whitespace text, or one whose own `group_id` is not the
//!    requested group, is `skipped_no_source` and stays recorded.
//! 2. `dry_run` stops here: per-kind counts, no embedder call, no write, no lock beyond the read.
//!    Stale record entries are removed only after the embed phase (its last cancellation point), so
//!    a cancelled run changes nothing.
//! 3. **Embed everything** with no lock held and no index touched: batched `embed_with_retry`
//!    (the same bounded retry/backoff/splitting as replay, #808) through the document-role embedder
//!    path (#790). Each vector is validated — wrong dimension, non-finite or all-zero is a `failed`
//!    row, never written.
//! 4. If nothing is writable, return: no lock, no index drop, no `indices_built` change.
//! 5. **Write** under the exclusive write lock via [`index_lifecycle::with_dropped_indexes`], which
//!    drops only the indexes of kinds that have a vector to write, runs a guarded `SET` per row,
//!    always rebuilds, and keeps `indices_built` correct (shared with the summary backfill).
//!
//! The write is guarded by `WHERE <text column> = $text`: because embedding happens *outside* the
//! write lock, the row's text may change in the gap, and a vector computed from stale text must not
//! be persisted. A row whose guard no longer matches counts as `failed` and stays recorded, so a
//! re-run retries it.
//!
//! Writes are **unrecorded** (never reach the WAL): vectors are a local cache stripped from the WAL
//! and recomputed on replay from co-located text (ADR-0526), so a repair adds nothing a replay could
//! not recompute. A row is removed from the record only after its new vector was actually written.
//!
//! Cancellation is honoured between embedding windows, where nothing is dirty. Once the write phase
//! starts it always finishes and rebuilds, so a cancel can never strand a dropped index.
//!
//! Callers must add `knowledge_reembed_missing` to `service_protocol.py` in the liminis-app repo.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::app_state::AppState;
use crate::db::{value_as_string, Conn};
use crate::embedding_cache::EmbedderContext;
use crate::error::Error;
use crate::index_lifecycle::{with_dropped_indexes, DbIndexDdl, IndexDdl, IndexKind};
use crate::replay::{reject_zero_vector, validate_recomputed_vector, vector_to_json};
use crate::replay_embed_retry::{
    embed_with_retry, real_jitter, real_sleep, EmbedRetryPolicy, RetryBreaker, RetryEnv,
    RetryOutcome,
};
use crate::zero_vector_record::ZeroKind;

/// Embed window size and write chunk size.
const BATCH_SIZE: usize = 100;
/// Chunk for `WHERE x.uuid IN $uuids` source-text lookups.
const LOOKUP_CHUNK: usize = 500;

// ── Params / report ───────────────────────────────────────────────────────────

pub struct ReembedParams {
    pub group_id: String,
    pub kinds: Vec<ZeroKind>,
    pub dry_run: bool,
    /// Overrides `EmbedRetryPolicy::from_env()` (tests only; the handler passes `None`).
    pub retry_policy: Option<EmbedRetryPolicy>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindCounts {
    pub candidates: usize,
    pub reembedded: usize,
    pub failed: usize,
    pub skipped_no_source: usize,
}

impl KindCounts {
    fn to_json(self) -> Value {
        json!({
            "candidates": self.candidates,
            "reembedded": self.reembedded,
            "failed": self.failed,
            "skipped_no_source": self.skipped_no_source,
        })
    }
}

#[derive(Debug, Default)]
pub struct ReembedReport {
    pub group_id: String,
    pub dry_run: bool,
    pub cancelled: bool,
    pub kinds: BTreeMap<ZeroKind, KindCounts>,
    pub index_rebuild_errors: Vec<(IndexKind, String)>,
    pub write_error: Option<String>,
}

impl ReembedReport {
    pub fn to_json(&self) -> Value {
        let mut kinds = serde_json::Map::new();
        for k in ZeroKind::ALL {
            kinds.insert(
                k.as_str().to_string(),
                self.kinds.get(&k).copied().unwrap_or_default().to_json(),
            );
        }
        let mut out = json!({
            "group_id": self.group_id,
            "dry_run": self.dry_run,
            "kinds": Value::Object(kinds),
        });
        if self.cancelled {
            out["cancelled"] = json!(true);
        }
        if !self.index_rebuild_errors.is_empty() {
            out["index_rebuild_errors"] = self
                .index_rebuild_errors
                .iter()
                .map(|(k, e)| json!({ "kind": k.as_str(), "error": e }))
                .collect();
        }
        if let Some(e) = &self.write_error {
            out["write_error"] = json!(e);
        }
        out
    }
}

/// Parses the `kinds` param: absent/null → all four; otherwise a non-empty array of valid kind
/// names (deduplicated). Anything else is rejected, naming the valid values.
pub fn parse_kinds(v: &Value) -> Result<Vec<ZeroKind>, Error> {
    let valid = || {
        ZeroKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if v.is_null() {
        return Ok(ZeroKind::ALL.to_vec());
    }
    let arr = v.as_array().ok_or_else(|| {
        Error::Ipc(format!(
            "kinds must be an array of kind names; valid kinds are {}",
            valid()
        ))
    })?;
    if arr.is_empty() {
        return Err(Error::Ipc(format!(
            "kinds must not be empty; valid kinds are {}",
            valid()
        )));
    }
    let mut out: Vec<ZeroKind> = Vec::new();
    for item in arr {
        let s = item.as_str().unwrap_or_default();
        let kind = ZeroKind::parse(s).ok_or_else(|| {
            Error::Ipc(format!(
                "invalid kind '{}': valid kinds are {}",
                item.as_str()
                    .map(str::to_string)
                    .unwrap_or(item.to_string()),
                valid()
            ))
        })?;
        if !out.contains(&kind) {
            out.push(kind);
        }
    }
    out.sort();
    Ok(out)
}

// ── Source-text reads ─────────────────────────────────────────────────────────

impl Conn<'_> {
    /// `uuid → (source text, group_id)` for the rows of `kind` among `uuids` (a uuid with no row is
    /// absent). Reads the co-located text the embedding was computed from: entity name / summary,
    /// relationship fact (straight from `RelatesToNode_`, so an edge whose endpoint was deleted is
    /// still found), episode content.
    pub(crate) fn get_embedding_source_rows(
        &self,
        kind: ZeroKind,
        uuids: &[String],
    ) -> Result<HashMap<String, (String, String)>, Error> {
        let cypher = match kind {
            ZeroKind::Name => {
                "MATCH (n:Entity) WHERE n.uuid IN $uuids RETURN n.uuid, n.name, n.group_id"
            }
            ZeroKind::Summary => {
                "MATCH (n:Entity) WHERE n.uuid IN $uuids RETURN n.uuid, n.summary, n.group_id"
            }
            ZeroKind::Content => {
                "MATCH (n:Episodic) WHERE n.uuid IN $uuids RETURN n.uuid, n.content, n.group_id"
            }
            ZeroKind::Fact => {
                "MATCH (n:RelatesToNode_) WHERE n.uuid IN $uuids \
                 RETURN n.uuid, n.fact, n.group_id"
            }
        };
        let mut out = HashMap::new();
        for chunk in uuids.chunks(LOOKUP_CHUNK) {
            for row in self.query_params(cypher, json!({ "uuids": chunk }))? {
                if row.len() < 3 {
                    continue;
                }
                out.insert(
                    value_as_string(&row[0]),
                    (value_as_string(&row[1]), value_as_string(&row[2])),
                );
            }
        }
        Ok(out)
    }
}

/// The guarded, row-addressed `SET` for one kind. Ends in `RETURN count(*)` so a guard that no
/// longer matches (text changed, row gone) is distinguishable from a write that applied.
fn set_cypher(kind: ZeroKind) -> &'static str {
    match kind {
        ZeroKind::Name => {
            "MATCH (n:Entity {uuid: $uuid}) WHERE n.name = $text \
             SET n.name_embedding = $emb RETURN count(*)"
        }
        ZeroKind::Summary => {
            "MATCH (n:Entity {uuid: $uuid}) WHERE n.summary = $text \
             SET n.summary_embedding = $emb RETURN count(*)"
        }
        ZeroKind::Content => {
            "MATCH (n:Episodic {uuid: $uuid}) WHERE n.content = $text \
             SET n.content_embedding = $emb RETURN count(*)"
        }
        ZeroKind::Fact => {
            "MATCH (n:RelatesToNode_ {uuid: $uuid}) WHERE n.fact = $text \
             SET n.fact_embedding = $emb RETURN count(*)"
        }
    }
}

// ── Selection ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Selection {
    /// Recorded entries for the kind (the report's `candidates`).
    recorded: usize,
    /// Entries whose row no longer exists: record entry to remove.
    stale: Vec<String>,
    /// Rows that exist but cannot be repaired (no text / another group): stay recorded.
    skipped: usize,
    /// `(uuid, text)` to embed.
    candidates: Vec<(String, String)>,
}

fn select(
    conn: &Conn<'_>,
    group_id: &str,
    kinds: &[ZeroKind],
) -> Result<BTreeMap<ZeroKind, Selection>, Error> {
    let recorded = conn.list_zero_vector_rows(group_id, None)?;
    let mut out = BTreeMap::new();
    for kind in kinds {
        let uuids: Vec<String> = recorded
            .iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, u)| u.clone())
            .collect();
        let mut sel = Selection {
            recorded: uuids.len(),
            ..Default::default()
        };
        if !uuids.is_empty() {
            let rows = conn.get_embedding_source_rows(*kind, &uuids)?;
            for uuid in uuids {
                match rows.get(&uuid) {
                    None => sel.stale.push(uuid),
                    Some((text, row_group)) => {
                        if text.trim().is_empty() || row_group != group_id {
                            sel.skipped += 1;
                        } else {
                            sel.candidates.push((uuid, text.clone()));
                        }
                    }
                }
            }
        }
        out.insert(*kind, sel);
    }
    Ok(out)
}

fn progress(tx: &Option<UnboundedSender<Value>>, phase: &str, processed: usize, total: usize) {
    if let Some(tx) = tx {
        let _ = tx.send(json!({
            "type": "progress",
            "phase": phase,
            "processed": processed,
            "total": total,
        }));
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Runs `knowledge_reembed_missing` against the live database.
pub async fn reembed_missing(
    state: Arc<AppState>,
    params: ReembedParams,
    progress_tx: Option<UnboundedSender<Value>>,
) -> Result<Value, Error> {
    reembed_missing_with_ddl(state, params, progress_tx, None).await
}

/// Like [`reembed_missing`] with an injectable index DDL (`None` = the real one). Exists so tests
/// can record drop/rebuild ordering and inject a rebuild failure.
#[doc(hidden)]
pub async fn reembed_missing_with_ddl(
    state: Arc<AppState>,
    params: ReembedParams,
    progress_tx: Option<UnboundedSender<Value>>,
    ddl_override: Option<Arc<dyn IndexDdl>>,
) -> Result<Value, Error> {
    // A bad retry env value fails before any work starts.
    let policy = match params.retry_policy {
        Some(p) => p,
        None => EmbedRetryPolicy::from_env()?,
    };
    let group_id = params.group_id.clone();
    let mut report = ReembedReport {
        group_id: group_id.clone(),
        dry_run: params.dry_run,
        ..Default::default()
    };
    for k in &params.kinds {
        report.kinds.insert(*k, KindCounts::default());
    }

    // ── Phase 1: select + classify (brief read lock) ─────────────────────────────────────────
    let db = state
        .db
        .load_full()
        .ok_or_else(|| Error::DbUnavailable("DB unavailable".to_string()))?;
    let selection = {
        let _read = state.write_lock.read().await;
        let (db_s, gid_s, kinds_s) = (Arc::clone(&db), group_id.clone(), params.kinds.clone());
        tokio::task::spawn_blocking(move || {
            let conn = db_s.connect().map_err(|e| Error::Ipc(format!("db: {e}")))?;
            select(&conn, &gid_s, &kinds_s)
        })
        .await??
    };
    for (kind, sel) in &selection {
        let c = report.kinds.entry(*kind).or_default();
        c.candidates = sel.recorded;
        c.skipped_no_source = sel.stale.len() + sel.skipped;
    }

    // ── Phase 2: dry run ─────────────────────────────────────────────────────────────────────
    if params.dry_run {
        return Ok(report.to_json());
    }

    // ── Phase 3: embed everything (no lock, no index touched) ────────────────────────────────
    // Stale record entries (row no longer exists) are removed only once the run is past its last
    // cancellation point, so a cancelled run changes nothing.
    let stale: Vec<(ZeroKind, String)> = selection
        .iter()
        .flat_map(|(k, s)| s.stale.iter().map(move |u| (*k, u.clone())))
        .collect();
    let total: usize = selection.values().map(|s| s.candidates.len()).sum();
    if total == 0 {
        remove_stale(&state, &db, &group_id, stale).await?;
        return Ok(report.to_json());
    }
    let embed_state = Arc::clone(&state);
    let embed_tx = progress_tx.clone();
    let candidates: BTreeMap<ZeroKind, Vec<(String, String)>> = selection
        .into_iter()
        .map(|(k, s)| (k, s.candidates))
        .collect();
    let embedded = tokio::task::spawn_blocking(move || {
        embed_all(&embed_state, &candidates, &policy, &embed_tx)
    })
    .await??;
    let EmbedPhase::Done {
        writable,
        failed: embed_failed,
    } = embedded
    else {
        report.cancelled = true;
        return Ok(report.to_json());
    };
    remove_stale(&state, &db, &group_id, stale).await?;
    for (kind, n) in &embed_failed {
        report.kinds.entry(*kind).or_default().failed += n;
    }

    // ── Phase 4: nothing writable → touch nothing ────────────────────────────────────────────
    let write_kinds: Vec<ZeroKind> = writable
        .iter()
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(k, _)| *k)
        .collect();
    if write_kinds.is_empty() {
        return Ok(report.to_json());
    }

    // ── Phase 5: write under the lifecycle (write lock held for drop → write → rebuild) ──────
    let guard = state.write_lock.write().await;
    // Re-load: the database may have been swapped (recovery) while embedding.
    let db = state
        .db
        .load_full()
        .ok_or_else(|| Error::DbUnavailable("DB unavailable".to_string()))?;
    let ddl: Arc<dyn IndexDdl> = match ddl_override {
        Some(d) => d,
        None => Arc::new(DbIndexDdl::new(Arc::clone(&db))),
    };
    let index_kinds: Vec<IndexKind> = write_kinds
        .iter()
        .map(|k| IndexKind::from_zero_kind(*k))
        .collect();
    let written: Arc<Mutex<BTreeMap<ZeroKind, usize>>> = Arc::default();
    let attempted: BTreeMap<ZeroKind, usize> =
        writable.iter().map(|(k, rows)| (*k, rows.len())).collect();
    let write_total: usize = attempted.values().sum();
    let (written_w, db_w, gid_w, tx_w) = (
        Arc::clone(&written),
        Arc::clone(&db),
        group_id.clone(),
        progress_tx.clone(),
    );
    let outcome = with_dropped_indexes(&state.indices_built, &guard, ddl, &index_kinds, || {
        write_all(db_w, gid_w, writable, written_w, tx_w, write_total)
    })
    .await;
    drop(guard);

    let written = written.lock().map(|m| m.clone()).unwrap_or_default();
    for (kind, n) in &attempted {
        let w = written.get(kind).copied().unwrap_or(0);
        let c = report.kinds.entry(*kind).or_default();
        c.reembedded = w;
        c.failed += n - w;
    }
    if let Err(e) = outcome.write_result {
        report.write_error = Some(e.to_string());
    }
    report.index_rebuild_errors = outcome
        .rebuild_errors
        .into_iter()
        .map(|(k, e)| (k, e.to_string()))
        .collect();
    Ok(report.to_json())
}

/// Removes record entries whose row no longer exists (FR-011). Takes the write lock briefly; no
/// index is touched.
async fn remove_stale(
    state: &Arc<AppState>,
    db: &Arc<crate::db::Db>,
    group_id: &str,
    stale: Vec<(ZeroKind, String)>,
) -> Result<(), Error> {
    if stale.is_empty() {
        return Ok(());
    }
    let _w = state.write_lock.write().await;
    let (db_s, gid_s) = (Arc::clone(db), group_id.to_string());
    tokio::task::spawn_blocking(move || {
        let conn = db_s.connect().map_err(|e| Error::Ipc(format!("db: {e}")))?;
        conn.remove_zero_vector_rows(&gid_s, &stale)
    })
    .await??;
    Ok(())
}

// ── Embed phase ───────────────────────────────────────────────────────────────

enum EmbedPhase {
    Done {
        /// `(uuid, text, vector)` rows that embedded and validated, per kind. Vectors stay as
        /// `Vec<f32>` (4 bytes a component) until their write batch, where they are converted to
        /// JSON: every vector is held until all embedding finishes (FR-014), and a JSON array
        /// costs ~32 bytes a component, which at ~100k rows would be gigabytes.
        writable: BTreeMap<ZeroKind, Vec<(String, String, Vec<f32>)>>,
        /// Per-kind count of rows whose embedding failed or was invalid.
        failed: BTreeMap<ZeroKind, usize>,
    },
    Cancelled,
}

/// Runs on a blocking thread: the retry policy sleeps and the embedder bridge uses
/// `Handle::block_on`, neither of which may run on an async worker.
fn embed_all(
    state: &Arc<AppState>,
    candidates: &BTreeMap<ZeroKind, Vec<(String, String)>>,
    policy: &EmbedRetryPolicy,
    progress_tx: &Option<UnboundedSender<Value>>,
) -> Result<EmbedPhase, Error> {
    let ctx = EmbedderContext {
        embedder: Arc::clone(&state.embedder),
        model: state.embedding_model.clone(),
        cache: Arc::clone(&state.embedding_cache),
    };
    let boxed = ctx.recompute_fn_via_handle();
    let embed_fn = |texts: &[&str]| boxed(texts);
    let dim = state.embedder.dim();
    let cancel =
        || progress_tx.as_ref().is_some_and(|t| t.is_closed()) || state.cancel_token.is_cancelled();
    let env = RetryEnv {
        cancel: &cancel,
        sleep: &real_sleep,
        jitter: &real_jitter,
    };
    let mut breaker = RetryBreaker::default();
    let total: usize = candidates.values().map(Vec::len).sum();
    let mut processed = 0usize;
    let mut writable: BTreeMap<ZeroKind, Vec<(String, String, Vec<f32>)>> = BTreeMap::new();
    let mut failed: BTreeMap<ZeroKind, usize> = BTreeMap::new();

    for (kind, rows) in candidates {
        for window in rows.chunks(BATCH_SIZE) {
            // `embed_with_retry` polls `cancel` only on its failure paths (replay polls per row
            // itself), so a healthy run must check between windows here.
            if cancel() {
                return Ok(EmbedPhase::Cancelled);
            }
            let texts: Vec<&str> = window.iter().map(|(_, t)| t.as_str()).collect();
            let results = match embed_with_retry(&texts, policy, &mut breaker, &env, &embed_fn) {
                RetryOutcome::Resolved(r) => r,
                RetryOutcome::Cancelled => return Ok(EmbedPhase::Cancelled),
            };
            for ((uuid, text), result) in window.iter().zip(results) {
                // Validate (dimension, finiteness, not all-zero) but keep the compact `Vec<f32>`;
                // the JSON form `validate_recomputed_vector` returns is built per write batch.
                let valid = result.and_then(|v| {
                    reject_zero_vector(&v)
                        .and_then(|()| validate_recomputed_vector(v.clone(), dim))
                        .map(|_| v)
                        .map_err(|e| e.to_string())
                });
                match valid {
                    Ok(vec) => {
                        writable
                            .entry(*kind)
                            .or_default()
                            .push((uuid.clone(), text.clone(), vec))
                    }
                    Err(msg) => {
                        eprintln!(
                            "liminis-context-graph: reembed_missing: {} {uuid}: {msg}",
                            kind.as_str()
                        );
                        *failed.entry(*kind).or_default() += 1;
                    }
                }
            }
            processed += window.len();
            progress(progress_tx, "embedding", processed, total);
        }
    }
    Ok(EmbedPhase::Done { writable, failed })
}

// ── Write phase ───────────────────────────────────────────────────────────────

/// Runs with the indexes dropped and the write lock held. Always processes every row (one row's
/// failure never aborts the rest) and removes each batch's written rows from the record.
async fn write_all(
    db: Arc<crate::db::Db>,
    group_id: String,
    writable: BTreeMap<ZeroKind, Vec<(String, String, Vec<f32>)>>,
    written: Arc<Mutex<BTreeMap<ZeroKind, usize>>>,
    progress_tx: Option<UnboundedSender<Value>>,
    total: usize,
) -> Result<(), Error> {
    let mut processed = 0usize;
    for (kind, rows) in writable {
        for batch in rows.chunks(BATCH_SIZE) {
            let batch: Vec<(String, String, Vec<f32>)> = batch.to_vec();
            let n = batch.len();
            let (db_b, gid_b, written_b) =
                (Arc::clone(&db), group_id.clone(), Arc::clone(&written));
            tokio::task::spawn_blocking(move || -> Result<(), Error> {
                let conn = db_b.connect().map_err(|e| Error::Ipc(format!("db: {e}")))?;
                let mut stmt = conn.prepare(set_cypher(kind))?;
                let mut ok: Vec<(ZeroKind, String)> = Vec::new();
                for (uuid, text, vec) in &batch {
                    // Validated finite at embed time, so the conversion cannot fail.
                    let Some(emb) = vector_to_json(vec) else {
                        eprintln!(
                            "liminis-context-graph: reembed_missing: {} {uuid}: non-finite vector",
                            kind.as_str()
                        );
                        continue;
                    };
                    let params = json!({ "uuid": uuid, "text": text, "emb": emb });
                    match conn.execute_prepared_returning_count(&mut stmt, &params) {
                        Ok(n) if n > 0 => ok.push((kind, uuid.clone())),
                        // Guard no longer matches: the row changed or vanished in the gap.
                        Ok(_) => eprintln!(
                            "liminis-context-graph: reembed_missing: {} {uuid}: row changed \
                             since it was read; left recorded",
                            kind.as_str()
                        ),
                        Err(e) => eprintln!(
                            "liminis-context-graph: reembed_missing: {} {uuid}: write failed: {e}",
                            kind.as_str()
                        ),
                    }
                }
                if let Ok(mut m) = written_b.lock() {
                    *m.entry(kind).or_default() += ok.len();
                }
                if let Err(e) = conn.remove_zero_vector_rows(&gid_b, &ok) {
                    // The vectors are written; a stale record entry is re-embedded harmlessly
                    // by the next run.
                    eprintln!("liminis-context-graph: reembed_missing: record update failed: {e}");
                }
                Ok(())
            })
            .await??;
            processed += n;
            progress(&progress_tx, "writing", processed, total);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_default_to_all() {
        assert_eq!(parse_kinds(&Value::Null).unwrap(), ZeroKind::ALL.to_vec());
    }

    #[test]
    fn kinds_dedupe_and_sort() {
        let k = parse_kinds(&json!(["content", "name", "content"])).unwrap();
        assert_eq!(k, vec![ZeroKind::Name, ZeroKind::Content]);
    }

    #[test]
    fn kinds_reject_invalid_empty_and_non_array() {
        let e = parse_kinds(&json!(["name", "bogus"]))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("bogus") && e.contains("name, fact, summary, content"),
            "{e}"
        );
        assert!(parse_kinds(&json!([])).is_err());
        assert!(parse_kinds(&json!("name")).is_err());
        assert!(parse_kinds(&json!([1])).is_err());
    }

    #[test]
    fn zero_vector_is_rejected_but_real_vector_passes() {
        assert!(reject_zero_vector(&[0.0, 0.0, 0.0]).is_err());
        assert!(reject_zero_vector(&[]).is_err());
        assert!(reject_zero_vector(&[0.0, 0.1]).is_ok());
    }

    #[test]
    fn invalid_vectors_fail_validation() {
        assert!(validate_recomputed_vector(vec![1.0, 2.0], 3).is_err());
        assert!(validate_recomputed_vector(vec![1.0, f32::NAN, 0.0], 3).is_err());
        assert!(validate_recomputed_vector(vec![1.0, f32::INFINITY, 0.0], 3).is_err());
        assert!(validate_recomputed_vector(vec![1.0, 2.0, 3.0], 3).is_ok());
    }

    #[test]
    fn report_always_carries_all_four_kinds() {
        let mut r = ReembedReport {
            group_id: "g".into(),
            ..Default::default()
        };
        r.kinds.insert(
            ZeroKind::Fact,
            KindCounts {
                candidates: 3,
                reembedded: 2,
                failed: 1,
                skipped_no_source: 0,
            },
        );
        let j = r.to_json();
        for k in ["name", "fact", "summary", "content"] {
            assert!(j["kinds"][k].is_object(), "{k}");
        }
        assert_eq!(j["kinds"]["fact"]["reembedded"], 2);
        assert_eq!(j["kinds"]["name"]["candidates"], 0);
        assert!(j.get("cancelled").is_none());
        assert!(j.get("index_rebuild_errors").is_none());
    }
}
