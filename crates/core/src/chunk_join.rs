//! Joining a duplicate in-flight `knowledge_process_chunk` (issue #914, ADR-0914).
//!
//! A `process_chunk` for a `(group_id, chunk_id)` that already has an identical request in flight
//! does not extract again: it waits for that request (the *leader*) and returns the same result,
//! marked `joined`. This module is the registry and the election; the handler owns the work.
//!
//! * **Election is atomic.** [`enter`] decides "become leader", "join" or "go it alone" inside one
//!   critical section, always taking this registry's lock before `chunk_inflight`'s.
//! * **Followers do not count in `chunk_inflight`.** The counting users (`move_document`, the
//!   unchanged no-op's `alone` rule, `retire`) therefore see exactly what they saw before.
//! * **A follower joins only when the leader is the sole in-flight registration for the key**
//!   (`chunk_inflight_count == 1`). With a differing request also in flight, commit-instant
//!   ordering (ADR-0705, ADR-0858) must decide, so the request takes today's independent path.
//! * **Cancellation is not inherited.** A leader that ends cancelled (disconnect, deadline,
//!   shutdown, panic) publishes [`JoinOutcome::Abandoned`]; each follower goes back through
//!   [`enter`], where one becomes the new leader and the rest join it. A leader that fails for any
//!   other reason publishes the failure and followers share it.
//! * **No lost wake-ups, no hangs.** [`LeaderHandle`] releases its `chunk_inflight` registration
//!   and removes its registry entry (both under the registry lock), and only then publishes, in
//!   `Drop`. An outcome is always delivered even when the leader's future is dropped or panics
//!   (`Abandoned`), and a re-electing follower never sees the departed leader still counted.
//!
//! The registry is a process-wide static keyed by the `AppState` address rather than an `AppState`
//! field, like `document::ingest`'s per-document locks, so the many fixtures that build `AppState`
//! literally stay untouched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::app_state::{AppState, ChunkInflightGuard};
use crate::chunk_unchanged::ChunkRequestView;
use crate::error::Error;
use crate::origin::Origin;

/// What two requests must share to be the same request: the ADR-0858 comparison fields, request
/// side. Built from [`ChunkRequestView`] with an exhaustive destructure so a field added there
/// cannot silently be left out of join identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkFingerprint {
    chunk_text: String,
    /// Parsed JSON, so key order and whitespace do not matter (as the unchanged check).
    attributes: Value,
    origin: Origin,
    source_file: String,
    doc_id: Option<String>,
    /// An *explicit* `reference_time`. Two requests join when both omitted it or both gave the
    /// same instant; omitted-versus-explicit differs, since the omitted one takes "now" at its
    /// own start.
    explicit_reference_time_us: Option<i64>,
}

impl ChunkFingerprint {
    pub fn from_view(view: &ChunkRequestView<'_>) -> Self {
        let ChunkRequestView {
            chunk_text,
            // The chunk id is part of the registry key, not of the fingerprint.
            chunk_id: _,
            attributes,
            origin,
            source_file,
            doc_id,
            explicit_reference_time_us,
        } = view;
        Self {
            chunk_text: (*chunk_text).to_string(),
            attributes: serde_json::from_str::<Value>(attributes)
                .unwrap_or_else(|_| Value::String((*attributes).into())),
            origin: *origin,
            source_file: (*source_file).to_string(),
            doc_id: doc_id.map(str::to_string),
            explicit_reference_time_us: *explicit_reference_time_us,
        }
    }
}

/// A leader's published result.
#[derive(Debug, Clone)]
pub enum JoinOutcome {
    Pending,
    /// The leader's success body without per-request fields (`duration_seconds`, `joined`,
    /// `deadline_exceeded`, `warning`).
    Done(Arc<Value>),
    /// The leader failed for a reason other than cancellation; followers share the failure.
    Failed(Arc<Error>),
    /// The leader was cancelled or vanished; followers re-elect.
    Abandoned,
}

struct Entry {
    id: u64,
    fingerprint: ChunkFingerprint,
    tx: Arc<watch::Sender<JoinOutcome>>,
}

type Key = (usize, String, String);

fn registry() -> &'static Mutex<HashMap<Key, Entry>> {
    static R: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_registry() -> std::sync::MutexGuard<'static, HashMap<Key, Entry>> {
    registry().lock().unwrap_or_else(|e| e.into_inner())
}

/// The caller's role for one `process_chunk` request.
pub enum JoinRole {
    /// First joinable request for the key: do the work, then [`LeaderHandle::finish`].
    Leader { alone: bool, handle: LeaderHandle },
    /// An identical request is in flight: wait for it with [`wait`].
    Follower(watch::Receiver<JoinOutcome>),
    /// A different request is in flight (or the leader is not alone): today's independent path.
    /// The caller registers itself with `chunk_inflight_enter`.
    Independent,
}

/// Atomically elects the caller leader, follower or independent for `(group_id, chunk_id)`.
pub fn enter(
    state: &AppState,
    group_id: &str,
    chunk_id: &str,
    fingerprint: ChunkFingerprint,
) -> JoinRole {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let key: Key = (
        state as *const AppState as usize,
        group_id.to_string(),
        chunk_id.to_string(),
    );
    // Lock order: registry, then `chunk_inflight` (inside `chunk_inflight_count` / `_enter`).
    let mut reg = lock_registry();
    if let Some(entry) = reg.get(&key) {
        if entry.fingerprint == fingerprint && state.chunk_inflight_count(group_id, chunk_id) == 1 {
            return JoinRole::Follower(entry.tx.subscribe());
        }
        return JoinRole::Independent;
    }
    let (inflight, alone) = state.chunk_inflight_enter(group_id, chunk_id);
    let (tx, _rx) = watch::channel(JoinOutcome::Pending);
    let tx = Arc::new(tx);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    reg.insert(
        key.clone(),
        Entry {
            id,
            fingerprint,
            tx: Arc::clone(&tx),
        },
    );
    JoinRole::Leader {
        alone,
        handle: LeaderHandle {
            key,
            id,
            tx,
            inflight: Some(inflight),
            outcome: None,
        },
    }
}

/// The leader's side of the registry entry. Dropping it releases the leader's `chunk_inflight` registration, removes the entry and then publishes the
/// outcome set by [`LeaderHandle::finish`], or [`JoinOutcome::Abandoned`] if none was set (an
/// error return, a panic, or the connection task being aborted).
pub struct LeaderHandle {
    key: Key,
    id: u64,
    tx: Arc<watch::Sender<JoinOutcome>>,
    /// The leader's `chunk_inflight` registration. Owned here so that [`Drop`] releases it in the
    /// same critical section that removes the registry entry, before any follower is woken: a
    /// follower that re-elects must see the count without the departed leader, or a second
    /// re-electing follower would see `count == 2` and extract independently.
    inflight: Option<ChunkInflightGuard>,
    outcome: Option<JoinOutcome>,
}

impl LeaderHandle {
    /// Records the outcome to publish when the handle drops.
    pub fn finish(mut self, outcome: JoinOutcome) {
        self.outcome = Some(outcome);
    }
}

impl Drop for LeaderHandle {
    fn drop(&mut self) {
        {
            // Entry removal and the `chunk_inflight` release happen under the registry lock (lock
            // order: registry, then `chunk_inflight`), so `enter` never observes one without the
            // other, and the wake-up below comes after both.
            let mut reg = lock_registry();
            if reg.get(&self.key).is_some_and(|e| e.id == self.id) {
                reg.remove(&self.key);
            }
            drop(self.inflight.take());
        }
        self.tx
            .send_replace(self.outcome.take().unwrap_or(JoinOutcome::Abandoned));
    }
}

/// Waits for the leader's outcome. Returns `Err(Error::Cancelled)` as soon as `cancel` (the
/// follower's own token — disconnect, its own deadline, shutdown) fires; the leader is unaffected.
pub async fn wait(
    mut rx: watch::Receiver<JoinOutcome>,
    cancel: &CancellationToken,
) -> Result<JoinOutcome, Error> {
    tokio::select! {
        biased;
        r = rx.wait_for(|o| !matches!(o, JoinOutcome::Pending)) => Ok(match r {
            Ok(g) => (*g).clone(),
            // The sender vanished without publishing: treat as a vanished leader.
            Err(_) => JoinOutcome::Abandoned,
        }),
        _ = cancel.cancelled() => Err(Error::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ChunkRequestView<'static> {
        ChunkRequestView {
            chunk_text: "hello",
            chunk_id: "c1",
            attributes: r#"{"a":1,"b":2}"#,
            origin: Origin::Primary,
            source_file: "f.md",
            doc_id: None,
            explicit_reference_time_us: None,
        }
    }

    fn fp(v: &ChunkRequestView<'_>) -> ChunkFingerprint {
        ChunkFingerprint::from_view(v)
    }

    #[test]
    fn identical_requests_match() {
        assert_eq!(fp(&view()), fp(&view()));
    }

    #[test]
    fn attribute_key_order_and_whitespace_do_not_matter() {
        let mut v = view();
        v.attributes = r#"{ "b": 2,  "a": 1 }"#;
        assert_eq!(fp(&view()), fp(&v));
    }

    #[test]
    fn every_compared_field_differentiates() {
        let base = fp(&view());
        let mut v = view();
        v.chunk_text = "hellO";
        assert_ne!(base, fp(&v));
        let mut v = view();
        v.attributes = r#"{"a":1}"#;
        assert_ne!(base, fp(&v));
        let mut v = view();
        v.origin = Origin::Derived;
        assert_ne!(base, fp(&v));
        let mut v = view();
        v.source_file = "g.md";
        assert_ne!(base, fp(&v));
        let mut v = view();
        v.doc_id = Some("d");
        assert_ne!(base, fp(&v));
        let mut v = view();
        v.explicit_reference_time_us = Some(5);
        assert_ne!(base, fp(&v), "omitted vs explicit reference_time differs");
    }

    #[test]
    fn reference_time_equal_explicit_values_match() {
        let mut a = view();
        a.explicit_reference_time_us = Some(7);
        let mut b = view();
        b.explicit_reference_time_us = Some(7);
        assert_eq!(fp(&a), fp(&b));
        b.explicit_reference_time_us = Some(8);
        assert_ne!(fp(&a), fp(&b));
    }
}
