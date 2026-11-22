//! Shared lifecycle for writing to an HNSW-indexed embedding column (issue #809, ADR-0809;
//! extracted from `backfill_summary_embeddings`, issue #470, ADR-0379).
//!
//! lbug rejects a plain `SET` on a column that carries a vector index, so every repair that must
//! rewrite an embedding in place follows the same sequence, which this module owns so that it
//! exists once:
//!
//! 1. the caller holds `AppState::write_lock` (exclusive) — passed in as a guard reference, so the
//!    helper cannot be called without it;
//! 2. remember `indices_built`, then store `false` **before** anything is dropped — the hot-path
//!    search handlers never take `write_lock` for reads, so a search that races in and hits a
//!    missing-index error must take the auto-heal branch (`build_indices_once`, which blocks on
//!    `write_lock` until the repair releases it) rather than a hard failure (ADR-0025, ADR-0036);
//! 3. drop only the indexes named by the caller;
//! 4. run the caller's write closure;
//! 5. **always** attempt to rebuild exactly the dropped indexes — even when the write failed, so a
//!    failed write never strands an index;
//! 6. restore `indices_built` to its **prior** value only if every rebuild succeeded; on any
//!    rebuild failure leave it `false`, so the next search rebuilds everything via
//!    `build_indices_once`.
//!
//! Step 6 differs from the pre-#809 backfill, which stored `true` unconditionally. `indices_built`
//! is one flag for *all* indexes (FTS and lookup too), so storing `true` after rebuilding a single
//! vector index could claim a missing FTS index was built whenever the flag had been `false`
//! beforehand. Restoring the prior value changes the backfill's behaviour only in that already
//! broken case; a normal run still ends with `indices_built == true`.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::RwLockWriteGuard;

use crate::db::Db;
use crate::error::Error;
use crate::zero_vector_record::ZeroKind;

/// One of the four database-wide HNSW vector indexes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IndexKind {
    /// `entity_name_embedding_idx` on `Entity.name_embedding`.
    Name,
    /// `entity_summary_embedding_idx` on `Entity.summary_embedding`.
    Summary,
    /// `episodic_content_embedding_idx` on `Episodic.content_embedding`, together with
    /// `summary_unit_content_embedding_idx` on `SummaryUnit.content_embedding` (issue #850): both
    /// columns are the `content` kind of the zero-vector record, so a repair of that kind may
    /// write either table and drops and rebuilds both indexes.
    Content,
    /// `edge_fact_embedding_idx` on `RelatesToNode_.fact_embedding`.
    Fact,
}

impl IndexKind {
    pub fn as_str(self) -> &'static str {
        match self {
            IndexKind::Name => "name",
            IndexKind::Summary => "summary",
            IndexKind::Content => "content",
            IndexKind::Fact => "fact",
        }
    }

    /// The index guarding the column a zero-vector record of `kind` repairs.
    pub fn from_zero_kind(kind: ZeroKind) -> Self {
        match kind {
            ZeroKind::Name => IndexKind::Name,
            ZeroKind::Summary => IndexKind::Summary,
            ZeroKind::Content => IndexKind::Content,
            ZeroKind::Fact => IndexKind::Fact,
        }
    }
}

/// Creates and drops the vector indexes. A trait so tests can record ordering and inject a
/// rebuild failure; production uses [`DbIndexDdl`].
pub trait IndexDdl: Send + Sync + 'static {
    /// Drops the index. Idempotent in production (an absent index is not an error).
    fn drop_index(&self, kind: IndexKind) -> Result<(), Error>;
    /// Creates the index. Idempotent in production ("already exists" is not an error).
    fn create_index(&self, kind: IndexKind) -> Result<(), Error>;
}

/// The real index DDL against the open database.
pub struct DbIndexDdl {
    db: Arc<Db>,
}

impl DbIndexDdl {
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }
}

impl IndexDdl for DbIndexDdl {
    fn drop_index(&self, kind: IndexKind) -> Result<(), Error> {
        let conn = self
            .db
            .connect()
            .map_err(|e| Error::Ipc(format!("db: {e}")))?;
        match kind {
            IndexKind::Name => conn.drop_entity_name_embedding_index(),
            IndexKind::Summary => conn.drop_entity_summary_embedding_index(),
            IndexKind::Content => {
                conn.drop_episodic_content_embedding_index();
                conn.drop_summary_unit_content_embedding_index();
            }
            IndexKind::Fact => conn.drop_edge_fact_embedding_index(),
        }
        Ok(())
    }

    fn create_index(&self, kind: IndexKind) -> Result<(), Error> {
        let conn = self
            .db
            .connect()
            .map_err(|e| Error::Ipc(format!("db: {e}")))?;
        match kind {
            IndexKind::Name => conn.create_entity_name_embedding_index(),
            IndexKind::Summary => conn.create_entity_summary_embedding_index(),
            IndexKind::Content => {
                conn.create_episodic_content_embedding_index()?;
                conn.create_summary_unit_index()
            }
            IndexKind::Fact => conn.create_edge_fact_embedding_index(),
        }
    }
}

/// What [`with_dropped_indexes`] did.
#[derive(Debug)]
pub struct LifecycleOutcome<T> {
    /// The write closure's result (or the drop error that prevented it running).
    pub write_result: Result<T, Error>,
    /// Indexes whose rebuild failed. Non-empty means `indices_built` was left `false`.
    pub rebuild_errors: Vec<(IndexKind, Error)>,
}

async fn blocking_ddl<R: Send + 'static>(
    f: impl FnOnce() -> R + Send + 'static,
) -> Result<R, Error> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Ipc(format!("index task failed: {e}")))
}

/// Drops `kinds`, runs `write`, rebuilds `kinds`, and keeps `indices_built` correct (see module
/// doc). With an empty `kinds` it touches neither the indexes nor the flag and just runs `write`.
///
/// `_write_guard` is the caller's exclusive hold on `AppState::write_lock`; it must stay held
/// until this returns (and, for the backfill, beyond it).
pub async fn with_dropped_indexes<T, Fut>(
    indices_built: &AtomicBool,
    _write_guard: &RwLockWriteGuard<'_, ()>,
    ddl: Arc<dyn IndexDdl>,
    kinds: &[IndexKind],
    write: impl FnOnce() -> Fut,
) -> LifecycleOutcome<T>
where
    Fut: Future<Output = Result<T, Error>>,
{
    let mut kinds: Vec<IndexKind> = kinds.to_vec();
    kinds.sort();
    kinds.dedup();
    if kinds.is_empty() {
        return LifecycleOutcome {
            write_result: write().await,
            rebuild_errors: Vec::new(),
        };
    }

    let prior = indices_built.load(Ordering::Acquire);
    // Cleared before anything is dropped so a racing search auto-heals instead of failing.
    indices_built.store(false, Ordering::Release);

    let mut drop_error: Option<Error> = None;
    for kind in &kinds {
        let ddl = Arc::clone(&ddl);
        let k = *kind;
        match blocking_ddl(move || ddl.drop_index(k)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => {
                drop_error = Some(e);
                break;
            }
        }
    }

    let write_result = match drop_error {
        Some(e) => Err(e),
        None => write().await,
    };

    // Always attempt the rebuild, even after a failed write or drop.
    let mut rebuild_errors: Vec<(IndexKind, Error)> = Vec::new();
    for kind in &kinds {
        let ddl = Arc::clone(&ddl);
        let k = *kind;
        match blocking_ddl(move || ddl.create_index(k)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) | Err(e) => rebuild_errors.push((k, e)),
        }
    }
    if rebuild_errors.is_empty() {
        indices_built.store(prior, Ordering::Release);
    }
    // else: leave `false` — the next search's `build_indices_once` rebuilds every index.

    LifecycleOutcome {
        write_result,
        rebuild_errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::sync::RwLock;

    #[derive(Default)]
    struct FakeDdl {
        events: Mutex<Vec<String>>,
        fail_create: Mutex<Vec<IndexKind>>,
    }

    impl FakeDdl {
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl IndexDdl for FakeDdl {
        fn drop_index(&self, kind: IndexKind) -> Result<(), Error> {
            self.events
                .lock()
                .unwrap()
                .push(format!("drop:{}", kind.as_str()));
            Ok(())
        }
        fn create_index(&self, kind: IndexKind) -> Result<(), Error> {
            self.events
                .lock()
                .unwrap()
                .push(format!("create:{}", kind.as_str()));
            if self.fail_create.lock().unwrap().contains(&kind) {
                return Err(Error::Ipc("boom".into()));
            }
            Ok(())
        }
    }

    async fn run(
        prior: bool,
        ddl: &Arc<FakeDdl>,
        kinds: &[IndexKind],
        write_ok: bool,
    ) -> (AtomicBool, LifecycleOutcome<u32>) {
        let flag = AtomicBool::new(prior);
        let lock = RwLock::new(());
        let guard = lock.write().await;
        let d: Arc<dyn IndexDdl> = ddl.clone();
        let events = Arc::clone(ddl);
        let out = with_dropped_indexes(&flag, &guard, d, kinds, || async move {
            events.events.lock().unwrap().push("write".into());
            if write_ok {
                Ok(7)
            } else {
                Err(Error::Ipc("write failed".into()))
            }
        })
        .await;
        (flag, out)
    }

    #[tokio::test]
    async fn restores_prior_true_and_orders_drop_write_create() {
        let ddl = Arc::new(FakeDdl::default());
        let (flag, out) = run(true, &ddl, &[IndexKind::Fact, IndexKind::Name], true).await;
        assert_eq!(out.write_result.unwrap(), 7);
        assert!(out.rebuild_errors.is_empty());
        assert!(flag.load(Ordering::Acquire));
        assert_eq!(
            ddl.events(),
            [
                "drop:name",
                "drop:fact",
                "write",
                "create:name",
                "create:fact"
            ]
        );
    }

    #[tokio::test]
    async fn restores_prior_false_never_blindly_true() {
        let ddl = Arc::new(FakeDdl::default());
        let (flag, out) = run(false, &ddl, &[IndexKind::Summary], true).await;
        assert!(out.write_result.is_ok());
        assert!(!flag.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn rebuild_failure_leaves_flag_false_and_is_reported() {
        let ddl = Arc::new(FakeDdl::default());
        ddl.fail_create.lock().unwrap().push(IndexKind::Name);
        let (flag, out) = run(true, &ddl, &[IndexKind::Name, IndexKind::Content], true).await;
        assert!(out.write_result.is_ok());
        assert_eq!(out.rebuild_errors.len(), 1);
        assert_eq!(out.rebuild_errors[0].0, IndexKind::Name);
        assert!(!flag.load(Ordering::Acquire));
        // The other index is still rebuilt.
        assert!(ddl.events().contains(&"create:content".to_string()));
    }

    #[tokio::test]
    async fn rebuild_is_attempted_after_a_write_error() {
        let ddl = Arc::new(FakeDdl::default());
        let (flag, out) = run(true, &ddl, &[IndexKind::Content], false).await;
        assert!(out.write_result.is_err());
        assert!(out.rebuild_errors.is_empty());
        assert!(flag.load(Ordering::Acquire));
        assert_eq!(ddl.events(), ["drop:content", "write", "create:content"]);
    }

    #[tokio::test]
    async fn empty_kinds_touch_nothing() {
        let ddl = Arc::new(FakeDdl::default());
        let (flag, out) = run(false, &ddl, &[], true).await;
        assert!(out.write_result.is_ok());
        assert_eq!(ddl.events(), ["write"]);
        assert!(!flag.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn flag_is_false_while_indexes_are_down() {
        let ddl = Arc::new(FakeDdl::default());
        let flag = Arc::new(AtomicBool::new(true));
        let lock = RwLock::new(());
        let guard = lock.write().await;
        let seen = Arc::new(AtomicBool::new(true));
        let (f2, s2) = (Arc::clone(&flag), Arc::clone(&seen));
        let d: Arc<dyn IndexDdl> = ddl;
        let _ = with_dropped_indexes(&flag, &guard, d, &[IndexKind::Name], || async move {
            s2.store(f2.load(Ordering::Acquire), Ordering::Release);
            Ok::<_, Error>(())
        })
        .await;
        assert!(!seen.load(Ordering::Acquire));
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn zero_kind_maps_to_matching_index() {
        assert_eq!(IndexKind::from_zero_kind(ZeroKind::Fact), IndexKind::Fact);
        assert_eq!(
            IndexKind::from_zero_kind(ZeroKind::Summary),
            IndexKind::Summary
        );
    }
}
