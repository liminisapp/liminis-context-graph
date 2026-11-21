//! `knowledge_process_document`'s orchestration (issue #785, ADR-0785).
//!
//! One call is a completeness claim: "this is document D, in full". The server chunks it, diffs
//! it against D's previous committed revision and:
//!
//! 1. writes every added or changed chunk's units through the ordinary per-episode pipeline, one
//!    at a time (so cross-chunk entity dedup sees earlier chunks), **pending** — stamped with the
//!    new revision id but no `revision_committed_at`, so belief ignores them;
//! 2. if, and only if, every unit succeeded, runs one **commit step** under the write lock in an
//!    explicit transaction and one WAL chunk: the `DocRevision` row, the commit stamp on the new
//!    episodes, then ADR-0705's supersessions (`revision` for changed chunks, `chunk_removed` for
//!    removed ones, `revision` for stale and mode-transition episodes).
//!
//! The commit boundary is data, not WAL framing (the WAL has no transaction marker, ADR-0047):
//! a failure or crash before the commit step leaves only pending rows, which are never believed,
//! and the previous revision untouched. Unchanged chunks are skipped by content hash before any
//! extraction, and a byte-identical resubmission writes nothing at all.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::chunker::{chunk_document, ChunkedDocument};
use super::context::{render, ContextInput};
use super::diff::{diff, Baseline, Classification};
use super::DocRevisionRecord;
use crate::app_state::{load_db, AppState};
use crate::episode::{self, DocumentEpisode, EpisodeOpts, SupersedePolicy, WritePath};
use crate::error::Error;
use crate::origin::Origin;
use crate::types::SourceType;

/// Default document-size cap: 512 KiB of UTF-8 (the document-level successor of #293's cap).
pub const DOCUMENT_MAX_BYTES_DEFAULT: usize = 512 * 1024;
/// Default per-unit size: below the 8000-char chunk advisory, so no unit ever trips it.
pub const DOCUMENT_CHUNK_MAX_CHARS_DEFAULT: usize = 6_000;

fn positive_env(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Err(_) => default,
        Ok(v) => match v.trim().parse::<usize>() {
            Ok(n) if n > 0 => n,
            _ => {
                eprintln!(
                    "liminis-context-graph: {name}={v:?} is not a valid positive integer; using \
                     default {default}"
                );
                default
            }
        },
    }
}

/// `LCG_DOCUMENT_MAX_BYTES`, falling back (with a warning) to [`DOCUMENT_MAX_BYTES_DEFAULT`].
pub fn resolve_document_max_bytes() -> usize {
    positive_env("LCG_DOCUMENT_MAX_BYTES", DOCUMENT_MAX_BYTES_DEFAULT)
}

/// `LCG_DOCUMENT_CHUNK_MAX_CHARS`, falling back to [`DOCUMENT_CHUNK_MAX_CHARS_DEFAULT`].
pub fn resolve_document_chunk_max_chars() -> usize {
    positive_env(
        "LCG_DOCUMENT_CHUNK_MAX_CHARS",
        DOCUMENT_CHUNK_MAX_CHARS_DEFAULT,
    )
}

/// Validates a document's size and chunks it: the shared front half of ingest and preview, so
/// `knowledge_preview_chunks` returns exactly what `knowledge_process_document` would use
/// (FR-006). Rejects an over-cap document before anything else (FR-016) and a document that
/// yields no chunk (empty, whitespace-only or headings-only).
pub fn chunk_for_ingest(content: &str) -> Result<ChunkedDocument, Error> {
    let cap = resolve_document_max_bytes();
    if content.len() > cap {
        return Err(Error::Ipc(format!(
            "document_too_large: content is {} bytes, over the document-size cap of {cap} bytes \
             (LCG_DOCUMENT_MAX_BYTES); nothing was extracted or written",
            content.len()
        )));
    }
    let chunked = chunk_document(content, resolve_document_chunk_max_chars());
    if chunked.chunks.is_empty() {
        return Err(Error::Ipc(
            "empty_document: the document has no content to extract (it is empty, whitespace-only \
             or headings-only); nothing was written. To retire a whole document use \
             knowledge_supersede_document, knowledge_mark_chunk_removed or \
             knowledge_delete_by_source"
                .to_string(),
        ));
    }
    Ok(chunked)
}

/// One `knowledge_process_document` call.
#[derive(Debug, Clone, Default)]
pub struct DocumentRequest {
    pub doc_id: String,
    pub group_id: String,
    pub content: String,
    pub origin: Option<Origin>,
    pub title: Option<String>,
    pub source_path: Option<String>,
    /// RFC 3339; recorded on the revision only.
    pub effective_at: Option<String>,
    /// The extraction reference time (same meaning as chunk mode's `reference_time`).
    pub reference_time: String,
    /// A JSON object string, applied to every episode and recorded on the revision.
    pub attributes: String,
}

// ── Per-document serialization (FR-022) ─────────────────────────────────────

type DocLockKey = (usize, String, String);
type DocLockMap = Mutex<HashMap<DocLockKey, Arc<tokio::sync::Mutex<()>>>>;

/// Process-wide per-`(state, group_id, doc_id)` async locks. Lives here rather than on
/// `AppState` so the many test fixtures that build `AppState` literally stay untouched; the state
/// pointer in the key keeps two `AppState`s from sharing a lock.
fn doc_locks() -> &'static DocLockMap {
    static LOCKS: OnceLock<DocLockMap> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

struct DocLockGuard {
    key: DocLockKey,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for DocLockGuard {
    fn drop(&mut self) {
        drop(self.guard.take());
        let mut map = doc_locks().lock().unwrap_or_else(|p| p.into_inner());
        // Prune when nobody else holds or waits on the lock (only the map's reference is left).
        if map
            .get(&self.key)
            .is_some_and(|l| Arc::strong_count(l) == 1)
        {
            map.remove(&self.key);
        }
    }
}

async fn lock_document(state: &Arc<AppState>, group_id: &str, doc_id: &str) -> DocLockGuard {
    let key = (
        Arc::as_ptr(state) as usize,
        group_id.to_string(),
        doc_id.to_string(),
    );
    let lock = {
        let mut map = doc_locks().lock().unwrap_or_else(|p| p.into_inner());
        Arc::clone(map.entry(key.clone()).or_default())
    };
    let guard = lock.lock_owned().await;
    DocLockGuard {
        key,
        guard: Some(guard),
    }
}

// ── The ingest ──────────────────────────────────────────────────────────────

fn progress(tx: &Option<UnboundedSender<Value>>, event: Value) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

fn mint_name(doc_id: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{doc_id}::{}", &id[..12])
}

/// Errors that must keep their own type (and so their own IPC error code) through the wrap.
fn passes_through(e: &Error) -> bool {
    matches!(
        e,
        Error::Cancelled | Error::DbUnavailable(_) | Error::IdentitySetChangeRefused { .. }
    ) || e
        .to_string()
        .contains(crate::origin::ORIGIN_CONFLICT_PREFIX)
}

/// Per-new-chunk bookkeeping while the ingest runs.
struct ChunkOutcome {
    classification: Classification,
    name: String,
    episode_uuids: Vec<String>,
    previous_episode_uuids: Vec<String>,
}

/// Runs one `knowledge_process_document` call and returns its report (FR-020).
pub async fn process_document(
    state: Arc<AppState>,
    req: DocumentRequest,
    progress_tx: Option<UnboundedSender<Value>>,
) -> Result<Value, Error> {
    let started = std::time::Instant::now();
    let chunked = chunk_for_ingest(&req.content)?;
    let content_hash = crate::sha256_hex::sha256_hex(req.content.as_bytes());
    let origin = req.origin.unwrap_or(Origin::Primary);
    state.check_identity(&req.group_id)?;

    // Serialize ingests of one document (FR-022) *before* reading the baseline, so the later
    // revision always diffs against the earlier one's committed result.
    let _doc_lock = lock_document(&state, &req.group_id, &req.doc_id).await;

    // Baseline: origin guard (FR-018) plus the previous committed revision and live episodes.
    let db = load_db(&state)?;
    let (gid, did) = (req.group_id.clone(), req.doc_id.clone());
    let (previous, baseline) = tokio::task::spawn_blocking(move || {
        let conn = db.connect()?;
        if let Some(other) = conn
            .origins_for_doc(&gid, &did)?
            .into_iter()
            .find(|o| *o != origin)
        {
            return Err(crate::origin::conflict(format!(
                "doc_id {did:?} already has origin {:?}; a submission with origin {:?} is \
                 refused (one document has one origin)",
                other.as_str(),
                origin.as_str()
            )));
        }
        let previous = conn.latest_doc_revision(&gid, &did)?;
        let manifest = previous
            .as_ref()
            .map(|p| p.chunk_names.clone())
            .unwrap_or_default();
        let episodes = conn.document_baseline_episodes(&gid, &did)?;
        Ok::<_, Error>((previous, Baseline::from_episodes(episodes, &manifest)))
    })
    .await??;

    let doc_id = req.doc_id.clone();
    let d = diff(&baseline, &chunked, &mut || mint_name(&doc_id));
    let to_extract: usize = d
        .matches
        .iter()
        .zip(&chunked.chunks)
        .filter(|(m, _)| m.classification != Classification::Unchanged)
        .map(|(_, c)| c.units.len())
        .sum();
    let nothing_to_retire = d.removed.is_empty()
        && baseline.stale_uuids.is_empty()
        && baseline.mode_transition_uuids.is_empty();
    progress(
        &progress_tx,
        json!({
            "type": "progress",
            "phase": "chunked",
            "chunk_count": chunked.chunks.len(),
            "units_to_extract": to_extract,
        }),
    );

    // A byte-identical resubmission with the same document metadata (FR-021): no extraction, no
    // revision, no belief change. Identical content with new metadata (`attributes`, `title`,
    // `source_path`, `effective_at`) is a new completeness claim and falls through to commit a
    // revision with zero extraction, so the metadata is recorded rather than silently dropped.
    //
    // It also requires every live episode to carry the latest revision's id. A complete commit
    // re-stamps every carried-forward episode, so a mismatch means a torn WAL tail cut that
    // commit's re-stamps after its revision row and commit stamp landed: those episodes still
    // hold an older revision's spans. Falling through to the zero-extraction commit re-stamps
    // them, so resending the same bytes repairs the document instead of returning `unchanged`
    // over spans that no longer slice it (SC-006).
    if to_extract == 0
        && nothing_to_retire
        && previous.as_ref().is_some_and(|p| {
            p.content_hash == content_hash
                && same_metadata(p, &req)
                && baseline
                    .chunks
                    .iter()
                    .flat_map(|c| &c.episode_revision_ids)
                    .all(|r| *r == p.revision_id)
        })
    {
        let prev = previous.as_ref();
        let chunks: Vec<Value> = d
            .matches
            .iter()
            .zip(&chunked.chunks)
            .map(|(m, c)| {
                let eps = m
                    .previous
                    .map(|j| baseline.chunks[j].episode_uuids.clone())
                    .unwrap_or_default();
                chunk_json(c, &m.name, m.classification, &eps, &[])
            })
            .collect();
        return Ok(json!({
            "success": true,
            "status": "unchanged",
            "doc_id": req.doc_id,
            "group_id": req.group_id,
            "revision_id": prev.map(|p| p.revision_id.clone()),
            "previous_revision_id": prev.map(|p| p.revision_id.clone()),
            "chunker_version": chunked.chunker_version,
            "extraction_calls": 0,
            "chunks": chunks,
            "removed": [],
            "superseded_episode_uuids": [],
            "retired_episode_uuids": [],
            "stale_superseded_episode_uuids": [],
            "mode_transition_superseded_episode_uuids": [],
            "nodes_extracted": 0,
            "edges_extracted": 0,
            "duration_seconds": started.elapsed().as_secs_f64(),
        }));
    }

    // ── Pending writes: one episode per unit of every added/changed chunk ────
    let revision_id = uuid::Uuid::new_v4().to_string();
    let source_path_or_doc = req
        .source_path
        .clone()
        .unwrap_or_else(|| req.doc_id.clone());
    let source_type = if source_path_or_doc.to_lowercase().ends_with(".json") {
        SourceType::Json
    } else {
        SourceType::Text
    };
    let mut outcomes: Vec<ChunkOutcome> = Vec::with_capacity(chunked.chunks.len());
    let mut extraction_calls = 0usize;
    let mut nodes_extracted = 0usize;
    let mut edges_extracted = 0usize;
    let mut new_episode_uuids: Vec<String> = Vec::new();
    let mut extracted_so_far = 0usize;
    for (i, (m, chunk)) in d.matches.iter().zip(&chunked.chunks).enumerate() {
        let previous_episode_uuids = m
            .previous
            .map(|j| baseline.chunks[j].episode_uuids.clone())
            .unwrap_or_default();
        if m.classification == Classification::Unchanged {
            outcomes.push(ChunkOutcome {
                classification: m.classification,
                name: m.name.clone(),
                episode_uuids: previous_episode_uuids,
                previous_episode_uuids: Vec::new(),
            });
            continue;
        }
        let mut unit_uuids = Vec::with_capacity(chunk.units.len());
        let n_units = chunk.units.len();
        for (k, unit) in chunk.units.iter().enumerate() {
            // A cancelled ingest retires nothing: stop before the next unit.
            if state.cancel_token.is_cancelled() {
                state.cancelled_chunks.fetch_add(1, Ordering::Relaxed);
                return Err(Error::Cancelled);
            }
            let context = render(&ContextInput {
                title: req.title.as_deref(),
                source_path: req.source_path.as_deref(),
                origin: origin.as_str(),
                heading_path: &chunk.heading_path,
                map: &chunked.map,
                chunk_byte_start: chunk.byte_start,
                part: (n_units > 1).then_some((k + 1, n_units)),
                table_header: unit.table_header.as_deref(),
            });
            let body = &req.content[unit.byte_start..unit.byte_end];
            let opts = EpisodeOpts {
                // All supersession is deferred to the commit step, which supersedes the old
                // units together with `superseded_by` = the new final unit — the split-unit
                // protocol's guarantees without its per-call flags (FR-017).
                supersede: SupersedePolicy::None,
                doc_id: Some(req.doc_id.clone()),
                origin: Some(origin),
                guard_chunk_collision: false,
                write_path: WritePath::Document,
                document: Some(DocumentEpisode {
                    revision_id: revision_id.clone(),
                    byte_start: unit.byte_start as i64,
                    byte_end: unit.byte_end as i64,
                    heading_path: chunk.heading_path.clone(),
                    chunk_hash: chunk.chunk_hash.clone(),
                    context,
                }),
                // Issue #853: the same source file the display value is composed from.
                source_file: Some(source_path_or_doc.clone()),
            };
            extraction_calls += 1;
            let result = episode::add_episode_with_opts(
                Arc::clone(&state),
                &m.name,
                body,
                "text",
                &format!("{source_path_or_doc}:{}", m.name),
                &req.reference_time,
                &req.group_id,
                source_type,
                None,
                &req.attributes,
                opts,
            )
            .await
            .map_err(|e| {
                if passes_through(&e) {
                    e
                } else {
                    Error::Ipc(format!(
                        "document ingest failed at chunk {i} ({}) unit {}/{n_units}: {e}; \
                         revision {revision_id} was not committed, so nothing was superseded or \
                         retired and the previous revision is still believed",
                        if chunk.heading_path.is_empty() {
                            "preamble".to_string()
                        } else {
                            chunk.heading_path.join(" > ")
                        },
                        k + 1
                    ))
                }
            })?;
            nodes_extracted += result.nodes_extracted;
            edges_extracted += result.edges_extracted;
            unit_uuids.push(result.episode_uuid);
            extracted_so_far += 1;
            progress(
                &progress_tx,
                json!({
                    "type": "progress",
                    "phase": "extracting",
                    "chunk_index": i,
                    "chunk_count": chunked.chunks.len(),
                    "classification": m.classification.as_str(),
                    "unit": k + 1,
                    "units": n_units,
                    "extracted": extracted_so_far,
                    "total": to_extract,
                }),
            );
        }
        new_episode_uuids.extend(unit_uuids.iter().cloned());
        outcomes.push(ChunkOutcome {
            classification: m.classification,
            name: m.name.clone(),
            episode_uuids: unit_uuids,
            previous_episode_uuids,
        });
    }

    // ── Commit step: the revision's single commit boundary ───────────────────
    if state.cancel_token.is_cancelled() {
        state.cancelled_chunks.fetch_add(1, Ordering::Relaxed);
        return Err(Error::Cancelled);
    }
    progress(
        &progress_tx,
        json!({"type": "progress", "phase": "committing", "chunk_count": chunked.chunks.len()}),
    );
    // (old uuids, superseding final unit) per changed chunk.
    let changed: Vec<(Vec<String>, String)> = outcomes
        .iter()
        .filter(|o| o.classification == Classification::Changed)
        .filter_map(|o| {
            o.episode_uuids
                .last()
                .map(|f| (o.previous_episode_uuids.clone(), f.clone()))
        })
        .collect();
    let removed_uuids: Vec<String> = d
        .removed
        .iter()
        .flat_map(|&j| baseline.chunks[j].episode_uuids.iter().cloned())
        .collect();
    let record = DocRevisionRecord {
        revision_id: revision_id.clone(),
        group_id: req.group_id.clone(),
        doc_id: req.doc_id.clone(),
        title: req.title.clone(),
        source_path: req.source_path.clone(),
        origin: origin.as_str().to_string(),
        effective_at: req.effective_at.clone(),
        committed_at: String::new(),
        reference_time: req.reference_time.clone(),
        attributes: req.attributes.clone(),
        content_hash: content_hash.clone(),
        byte_len: req.content.len() as i64,
        chunker_version: i64::from(chunked.chunker_version),
        chunk_names: outcomes.iter().map(|o| o.name.clone()).collect(),
        previous_revision_id: previous.as_ref().map(|p| p.revision_id.clone()),
    };
    // Unchanged chunks' episodes are carried forward, not rewritten, but their position and
    // heading path belong to this revision now: re-stamp them so every live episode's span
    // slices *this* revision's content (FR-013, SC-006). The chunk's text is identical (same
    // hash) and its episodes' texts tiled it exactly when they were written, so each episode is
    // re-located by tiling those texts across the chunk again. That needs no stored offset, so it
    // stays exact when the episodes carry spans from different revisions (a torn tail that cut a
    // previous re-stamp part-way) and when the unit split itself has changed.
    let mut carried: Vec<(String, i64, i64, Vec<String>)> = Vec::new();
    for (m, c) in d.matches.iter().zip(&chunked.chunks) {
        let (Classification::Unchanged, Some(j)) = (m.classification, m.previous) else {
            continue;
        };
        let b = &baseline.chunks[j];
        match tile(&c.text, &b.episode_texts) {
            Some(offsets) => {
                for ((uuid, text), off) in b.episode_uuids.iter().zip(&b.episode_texts).zip(offsets)
                {
                    let s = (c.byte_start + off) as i64;
                    carried.push((
                        uuid.clone(),
                        s,
                        s + text.len() as i64,
                        c.heading_path.clone(),
                    ));
                }
            }
            // Unreachable for rows this code wrote; never write a span that might not slice.
            None => eprintln!(
                "liminis-context-graph: process_document: episodes of chunk {:?} do not tile its \
                 text; their spans are left as they were",
                b.name
            ),
        }
    }
    let stale = baseline.stale_uuids.clone();
    let transition = baseline.mode_transition_uuids.clone();
    let db_c = load_db(&state)?;
    let state_c = Arc::clone(&state);
    let gid_c = req.group_id.clone();
    let new_c = new_episode_uuids.clone();
    let rid_c = revision_id.clone();
    let (removed_c, stale_c, transition_c) =
        (removed_uuids.clone(), stale.clone(), transition.clone());
    let _write_guard = state.write_lock.write().await;
    // Taken under the write lock, like every Phase C instant (ADR-0705): the old beliefs end at
    // the very instant the new ones begin — no gap and no overlap.
    let committed_at = crate::db::ingested_at_or_now("");
    let committed_at_c = committed_at.clone();
    tokio::task::spawn_blocking(move || -> Result<(), Error> {
        let conn = db_c.connect()?;
        let mut record = record;
        record.committed_at = committed_at_c.clone();
        let t = committed_at_c.as_str();
        conn.exec_transaction_control("BEGIN TRANSACTION")?;
        let result = (|| -> Result<(), Error> {
            conn.insert_doc_revision(&record)?;
            conn.mark_revision_committed(&new_c, t)?;
            for (old, by) in &changed {
                conn.supersede_episodes_by_uuids(old, t, Some(by), "revision")?;
            }
            conn.supersede_episodes_by_uuids(&removed_c, t, None, "chunk_removed")?;
            let mut replaced = stale_c.clone();
            replaced.extend(transition_c.iter().cloned());
            conn.supersede_episodes_by_uuids(&replaced, t, None, "revision")?;
            // Last, so a torn tail can only cut re-stamps (Baseline groups by commit instant, so
            // a partly re-stamped chunk stays one chunk; the next commit re-stamps the rest).
            for (uuid, start, end, heading_path) in &carried {
                conn.restamp_carried_episode(uuid, &rid_c, *start, *end, heading_path)?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            // An execute-time failure already rolled the engine back; a ROLLBACK then errors,
            // which is harmless (db.rs pinning tests).
            let _ = conn.exec_transaction_control("ROLLBACK");
            let _ = conn.drain_mutations();
            return Err(e);
        }
        conn.exec_transaction_control("COMMIT")?;
        // One WAL chunk for the whole commit step; replay reproduces the commit as data.
        let flushed = crate::wal_exec::wal_flush_chunk(&state_c, &gid_c, conn.drain_mutations());
        if let Some((seq, generation)) = flushed {
            // Same identity and live setter as Phase C (`episode.rs`): a document commit is a
            // live write, so it never silently re-stamps a recorded, different document prefix
            // (ADR-0790); only a rebuild that recomputes every vector may claim a new one.
            let identity = crate::embedding_cache::EmbeddingIdentity {
                model: state_c.embedding_model.clone(),
                dim: state_c.embedder.dim() as i64,
                doc_prefix: state_c.embedder.instructions().document_prefix,
            };
            if let Err(e) =
                conn.set_wal_position_live(&gid_c, seq, generation.as_deref(), &identity)
            {
                eprintln!(
                    "liminis-context-graph: process_document: failed to persist applied_seq={seq} \
                     (non-fatal): {e}"
                );
            }
        }
        Ok(())
    })
    .await?
    .map_err(|e| {
        Error::Ipc(format!(
            "document ingest failed at the commit step: {e}; revision {revision_id} was not \
             committed, so nothing was superseded or retired"
        ))
    })?;
    drop(_write_guard);

    // ── Report ───────────────────────────────────────────────────────────────
    let chunks: Vec<Value> = outcomes
        .iter()
        .zip(&chunked.chunks)
        .map(|(o, c)| {
            chunk_json(
                c,
                &o.name,
                o.classification,
                &o.episode_uuids,
                &o.previous_episode_uuids,
            )
        })
        .collect();
    let removed: Vec<Value> = d
        .removed
        .iter()
        .map(|&j| {
            let b = &baseline.chunks[j];
            json!({
                "name": b.name,
                "heading_path": b.heading_path,
                "classification": "removed",
                "episode_uuids": b.episode_uuids,
            })
        })
        .collect();
    let superseded: Vec<String> = outcomes
        .iter()
        .filter(|o| o.classification == Classification::Changed)
        .flat_map(|o| o.previous_episode_uuids.iter().cloned())
        .collect();
    Ok(json!({
        "success": true,
        "status": "committed",
        "doc_id": req.doc_id,
        "group_id": req.group_id,
        "revision_id": revision_id,
        "previous_revision_id": previous.as_ref().map(|p| p.revision_id.clone()),
        "committed_at": committed_at,
        "chunker_version": chunked.chunker_version,
        "extraction_calls": extraction_calls,
        "chunks": chunks,
        "removed": removed,
        "superseded_episode_uuids": superseded,
        "retired_episode_uuids": removed_uuids,
        "stale_superseded_episode_uuids": stale,
        "mode_transition_superseded_episode_uuids": transition,
        "nodes_extracted": nodes_extracted,
        "edges_extracted": edges_extracted,
        "duration_seconds": started.elapsed().as_secs_f64(),
    }))
}

/// Whether `req` carries the same document metadata as revision `prev`: `title`, `source_path`,
/// `effective_at` (compared as an instant, since the column is a `TIMESTAMP` read back in its own
/// format) and `attributes` (compared as JSON values, so key order and whitespace do not count).
/// `reference_time` is not metadata — it defaults to the call's start instant.
fn same_metadata(prev: &DocRevisionRecord, req: &DocumentRequest) -> bool {
    let effective = req.effective_at.as_deref().map(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| crate::belief::format_us(t.timestamp_micros()))
            .unwrap_or_else(|_| s.to_string())
    });
    let json =
        |s: &str| serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.into()));
    prev.title == req.title
        && prev.source_path == req.source_path
        && prev.effective_at == effective
        && json(&prev.attributes) == json(&req.attributes)
}

/// Where each of `parts` sits in `text` when they tile it exactly (concatenated in some order,
/// each used once), as byte offsets in `parts` order; `None` if they do not. A depth-first search
/// that tries each distinct part once per position, so identical parts (repeated boilerplate
/// units) never multiply the work and any assignment among them is equally exact.
fn tile(text: &str, parts: &[String]) -> Option<Vec<usize>> {
    fn go(text: &str, pos: usize, parts: &[String], out: &mut [Option<usize>]) -> bool {
        if pos == text.len() {
            return out.iter().all(Option::is_some);
        }
        let mut tried: Vec<&str> = Vec::new();
        for i in 0..parts.len() {
            let p = parts[i].as_str();
            if out[i].is_some() || p.is_empty() || tried.contains(&p) {
                continue;
            }
            if text[pos..].starts_with(p) {
                tried.push(p);
                out[i] = Some(pos);
                if go(text, pos + p.len(), parts, out) {
                    return true;
                }
                out[i] = None;
            }
        }
        false
    }
    if parts.iter().map(String::len).sum::<usize>() != text.len() {
        return None;
    }
    let mut out = vec![None; parts.len()];
    go(text, 0, parts, &mut out).then(|| out.into_iter().flatten().collect())
}

fn chunk_json(
    c: &super::chunker::LogicalChunk,
    name: &str,
    classification: Classification,
    episode_uuids: &[String],
    previous_episode_uuids: &[String],
) -> Value {
    json!({
        "index": c.index,
        "name": name,
        "heading_path": c.heading_path,
        "byte_start": c.byte_start,
        "byte_end": c.byte_end,
        "content_hash": c.chunk_hash,
        "classification": classification.as_str(),
        "units": c.units.len(),
        "episode_uuids": episode_uuids,
        "previous_episode_uuids": previous_episode_uuids,
    })
}

/// `knowledge_preview_chunks`: the chunking alone, read-only (FR-006).
pub fn preview(content: &str) -> Result<Value, Error> {
    let chunked = chunk_for_ingest(content)?;
    let chunks: Vec<Value> = chunked
        .chunks
        .iter()
        .map(|c| {
            json!({
                "index": c.index,
                "heading_path": c.heading_path,
                "byte_start": c.byte_start,
                "byte_end": c.byte_end,
                "content_hash": c.chunk_hash,
                "char_count": c.char_count(),
                "units": c.units.iter().map(|u| json!({
                    "byte_start": u.byte_start,
                    "byte_end": u.byte_end,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "chunker_version": chunked.chunker_version,
        "byte_len": chunked.byte_len,
        "chunk_count": chunks.len(),
        "chunks": chunks,
        "document_map": chunked.map,
    }))
}

#[cfg(test)]
mod tests {
    use super::tile;

    fn v(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn tile_relocates_parts_in_any_stored_order() {
        // Parts stored in any order are placed where they belong.
        assert_eq!(
            tile("aaabbbcc", &v(&["cc", "aaa", "bbb"])),
            Some(vec![6, 0, 3])
        );
        // Identical parts (repeated boilerplate units) each take a distinct occurrence.
        assert_eq!(tile("abcabc", &v(&["abc", "abc"])), Some(vec![0, 3]));
        // A first choice that dead-ends is backtracked: "a" fits at 0 but strands "ab".
        assert_eq!(tile("aba", &v(&["a", "ab"])), Some(vec![2, 0]));
        // Lengths that do not sum to the text, or a part that is not in it, do not tile.
        assert_eq!(tile("abc", &v(&["ab", "a", "bc"])), None);
        assert_eq!(tile("xyz", &v(&["xy", "q"])), None);
    }
}
