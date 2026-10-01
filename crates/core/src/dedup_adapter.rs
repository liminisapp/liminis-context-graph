use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use sha2::{Digest, Sha256};

use crate::{
    dedup_judge::{DedupSide, DedupVerdict, DuplicatePair},
    env::lcg_env_var,
    error::Error,
    extractor::Extractor,
    prompts::normalize_name,
    types::{EntityRow, ExtractedEntity},
};

/// Most pairs sent to the extractor in a single judge call; larger batches are split into groups.
pub const MAX_PAIRS_PER_CALL: usize = 16;
/// Bound on one judge call. The extractor clients carry no request timeout of their own, so the
/// adapter wraps each group in `tokio::time::timeout`.
pub const JUDGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Cap on the in-process verdict cache; the cache is cleared when it fills.
const CACHE_CAP: usize = 4096;

// ── Dedup mode ────────────────────────────────────────────────────────────────

/// What protects the graph from wrong embedding-path merges, beyond the cosine threshold. The
/// identifier-mismatch veto (ADR-0650) is always on, so the modes differ only by the adapter
/// behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupMode {
    /// Identifier veto only: every candidate that survives it merges.
    VetoOnly,
    /// Identifier veto, then the configured extractor judges each surviving candidate (#652).
    LlmVerified,
}

impl DedupMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DedupMode::VetoOnly => "veto-only",
            DedupMode::LlmVerified => "llm-verified",
        }
    }
}

/// Whether `LCG_DEDUP_LLM` asks for the extractor-backed check. Unset, empty, `0`, `false`,
/// `off` and `no` (case-insensitive) are off; anything else is on.
pub fn dedup_llm_requested() -> bool {
    match lcg_env_var("LCG_DEDUP_LLM", "GRAPHITI_DEDUP_LLM") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "off" | "no"
        ),
        Err(_) => false,
    }
}

/// Whether the (retired) `LCG_DEDUP_ADAPTER_URL` / `GRAPHITI_DEDUP_ADAPTER_URL` is set.
pub fn deprecated_adapter_url_set() -> bool {
    std::env::var_os("LCG_DEDUP_ADAPTER_URL").is_some()
        || std::env::var_os("GRAPHITI_DEDUP_ADAPTER_URL").is_some()
}

/// The mode that `AppState::from_env` will actually use, given the extractor's availability.
pub fn effective_dedup_mode(extractor_configured: bool) -> DedupMode {
    if dedup_llm_requested() && extractor_configured {
        DedupMode::LlmVerified
    } else {
        DedupMode::VetoOnly
    }
}

/// Human-readable description of the active dedup mode for the startup log (#650, #652).
pub fn dedup_mode_description(extractor_configured: bool) -> String {
    let mode = effective_dedup_mode(extractor_configured).as_str();
    let setting = match (dedup_llm_requested(), extractor_configured) {
        (true, true) => "LCG_DEDUP_LLM=on",
        (true, false) => "LCG_DEDUP_LLM=on-but-no-extractor",
        (false, _) => "LCG_DEDUP_LLM=off",
    };
    format!("{mode} ({setting}; identifier veto always on)")
}

/// Builds the adapter for the current environment: [`ExtractorDedupAdapter`] when
/// `LCG_DEDUP_LLM` is on and the extractor can answer, otherwise [`PassthroughDedupAdapter`].
pub fn build_dedup_adapter(extractor: &Arc<dyn Extractor>) -> Arc<dyn DedupAdapter> {
    if effective_dedup_mode(extractor.is_configured()) == DedupMode::LlmVerified {
        Arc::new(ExtractorDedupAdapter::new(Arc::clone(extractor)))
    } else {
        Arc::new(PassthroughDedupAdapter)
    }
}

// ── DedupAdapter trait ────────────────────────────────────────────────────────

pub trait DedupAdapter: Send + Sync {
    /// Judges a batch of `(existing, incoming)` candidate pairs. Returns exactly one verdict per
    /// pair, in order. Never fails: every failure mode is [`DedupVerdict::Unknown`], which the
    /// caller treats as "not a duplicate".
    fn judge_batch<'a>(&'a self, pairs: &'a [DuplicatePair]) -> BoxFuture<'a, Vec<DedupVerdict>>;

    /// The mode this adapter implements, as reported by `knowledge_status`.
    fn mode(&self) -> DedupMode;

    /// Single-pair convenience over [`DedupAdapter::judge_batch`]: `true` only for a definite
    /// "duplicate" verdict.
    fn is_duplicate<'a>(
        &'a self,
        candidate: &'a EntityRow,
        incoming: &'a ExtractedEntity,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            let pair = pair_from(candidate, incoming);
            let verdicts = self.judge_batch(std::slice::from_ref(&pair)).await;
            Ok(verdicts.first() == Some(&DedupVerdict::Duplicate))
        })
    }
}

/// The pair shown to the judge for an existing row and an incoming extracted entity.
pub fn pair_from(candidate: &EntityRow, incoming: &ExtractedEntity) -> DuplicatePair {
    let existing_type = candidate
        .labels
        .iter()
        .rev()
        .find(|l| l.as_str() != "Entity")
        .cloned()
        .unwrap_or_default();
    DuplicatePair {
        existing: DedupSide {
            name: candidate.name.clone(),
            entity_type: existing_type,
            summary: candidate.summary.clone(),
        },
        incoming: DedupSide {
            name: incoming.name.clone(),
            entity_type: incoming.entity_type.clone(),
            summary: incoming.summary.clone(),
        },
    }
}

// ── PassthroughDedupAdapter ───────────────────────────────────────────────────

/// Confirms every candidate — the veto-only mode (cosine + identifier veto, no LLM).
pub struct PassthroughDedupAdapter;

impl DedupAdapter for PassthroughDedupAdapter {
    fn judge_batch<'a>(&'a self, pairs: &'a [DuplicatePair]) -> BoxFuture<'a, Vec<DedupVerdict>> {
        Box::pin(async move { vec![DedupVerdict::Duplicate; pairs.len()] })
    }

    fn mode(&self) -> DedupMode {
        DedupMode::VetoOnly
    }
}

// ── ExtractorDedupAdapter ─────────────────────────────────────────────────────

/// Asks the already-configured extractor (Anthropic, or the OpenAI-compatible UDS/HTTP endpoint)
/// to judge candidate pairs (#652, ADR-0652). A wrong merge cannot be undone without
/// re-ingestion while a missed merge is recoverable, so every error, timeout, malformed or
/// unattributable answer is [`DedupVerdict::Unknown`] and a warning is logged.
pub struct ExtractorDedupAdapter {
    extractor: Arc<dyn Extractor>,
    timeout: Duration,
    /// Definite verdicts keyed by a hash of the full content of both sides, so a changed summary
    /// is a new key.
    cache: Mutex<HashMap<String, DedupVerdict>>,
}

impl ExtractorDedupAdapter {
    pub fn new(extractor: Arc<dyn Extractor>) -> Self {
        Self::with_timeout(extractor, JUDGE_TIMEOUT)
    }

    pub fn with_timeout(extractor: Arc<dyn Extractor>, timeout: Duration) -> Self {
        Self {
            extractor,
            timeout,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn cache_key(pair: &DuplicatePair) -> String {
        let mut h = Sha256::new();
        for side in [&pair.existing, &pair.incoming] {
            h.update(normalize_name(&side.name).as_bytes());
            h.update([0x1f]);
            h.update(side.entity_type.as_bytes());
            h.update([0x1f]);
            h.update(side.summary.as_bytes());
            h.update([0x1e]);
        }
        format!("{:x}", h.finalize())
    }

    fn cache_get(&self, key: &str) -> Option<DedupVerdict> {
        self.cache.lock().ok()?.get(key).copied()
    }

    fn cache_put(&self, key: String, verdict: DedupVerdict) {
        if verdict == DedupVerdict::Unknown {
            return;
        }
        if let Ok(mut c) = self.cache.lock() {
            if c.len() >= CACHE_CAP {
                c.clear();
            }
            c.insert(key, verdict);
        }
    }

    async fn judge_group(&self, group: &[DuplicatePair]) -> Vec<DedupVerdict> {
        let unknown = vec![DedupVerdict::Unknown; group.len()];
        match tokio::time::timeout(self.timeout, self.extractor.judge_duplicates(group)).await {
            Ok(Ok(v)) if v.len() == group.len() => v,
            Ok(Ok(v)) => {
                eprintln!(
                    "liminis-context-graph: dedup judge returned {} verdicts for {} pairs; \
                     treating the group as not duplicate",
                    v.len(),
                    group.len()
                );
                unknown
            }
            Ok(Err(Error::CassetteMiss(m))) => {
                eprintln!(
                    "liminis-context-graph: dedup judge: no cassette record ({m}); \
                     treating the group as not duplicate"
                );
                unknown
            }
            Ok(Err(e)) => {
                eprintln!(
                    "liminis-context-graph: dedup judge failed ({e}); treating {} candidate(s) \
                     as not duplicate",
                    group.len()
                );
                unknown
            }
            Err(_) => {
                eprintln!(
                    "liminis-context-graph: dedup judge timed out after {}s; treating {} \
                     candidate(s) as not duplicate",
                    self.timeout.as_secs(),
                    group.len()
                );
                unknown
            }
        }
    }
}

impl DedupAdapter for ExtractorDedupAdapter {
    fn judge_batch<'a>(&'a self, pairs: &'a [DuplicatePair]) -> BoxFuture<'a, Vec<DedupVerdict>> {
        Box::pin(async move {
            let mut out = vec![DedupVerdict::Unknown; pairs.len()];
            let mut pending: Vec<(usize, String)> = Vec::new();
            for (i, pair) in pairs.iter().enumerate() {
                let key = Self::cache_key(pair);
                match self.cache_get(&key) {
                    Some(v) => out[i] = v,
                    None => pending.push((i, key)),
                }
            }
            for group in pending.chunks(MAX_PAIRS_PER_CALL) {
                let batch: Vec<DuplicatePair> =
                    group.iter().map(|(i, _)| pairs[*i].clone()).collect();
                let verdicts = self.judge_group(&batch).await;
                for ((i, key), v) in group.iter().zip(verdicts) {
                    out[*i] = v;
                    self.cache_put(key.clone(), v);
                }
            }
            out
        })
    }

    fn mode(&self) -> DedupMode {
        DedupMode::LlmVerified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::ExtractOptions;
    use crate::types::ExtractionOutcome;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Scripted judge: records every group it sees and answers via `answer`.
    struct ScriptedJudge {
        calls: AtomicUsize,
        group_sizes: Mutex<Vec<usize>>,
        answer: Box<dyn Fn(&[DuplicatePair]) -> Result<Vec<DedupVerdict>, Error> + Send + Sync>,
        delay: Duration,
    }

    impl ScriptedJudge {
        fn new(
            answer: impl Fn(&[DuplicatePair]) -> Result<Vec<DedupVerdict>, Error>
                + Send
                + Sync
                + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                group_sizes: Mutex::new(vec![]),
                answer: Box::new(answer),
                delay: Duration::ZERO,
            })
        }
    }

    impl Extractor for ScriptedJudge {
        fn extract<'a>(
            &'a self,
            _o: ExtractOptions<'a>,
        ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
            Box::pin(async { Err(Error::Ipc("unused".into())) })
        }
        fn classify_entities<'a>(
            &'a self,
            _e: &'a [(&'a str, &'a str)],
            _a: Option<&'a [String]>,
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Err(Error::Ipc("unused".into())) })
        }
        fn classify_relations<'a>(
            &'a self,
            _e: &'a [(&'a str, &'a str)],
            _a: &'a [(String, Option<String>)],
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Err(Error::Ipc("unused".into())) })
        }
        fn judge_duplicates<'a>(
            &'a self,
            pairs: &'a [DuplicatePair],
        ) -> BoxFuture<'a, Result<Vec<DedupVerdict>, Error>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.group_sizes.lock().unwrap().push(pairs.len());
            let delay = self.delay;
            let r = (self.answer)(pairs);
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                r
            })
        }
    }

    fn pair(name: &str) -> DuplicatePair {
        let side = |n: &str| DedupSide {
            name: n.to_string(),
            entity_type: "Person".to_string(),
            summary: String::new(),
        };
        DuplicatePair {
            existing: side(&format!("{name} existing")),
            incoming: side(&format!("{name} incoming")),
        }
    }

    /// Verdict derived from the pair's own name, so attribution is checkable.
    fn by_name(pairs: &[DuplicatePair]) -> Result<Vec<DedupVerdict>, Error> {
        Ok(pairs
            .iter()
            .map(|p| {
                if p.existing.name.starts_with("dup") {
                    DedupVerdict::Duplicate
                } else {
                    DedupVerdict::Distinct
                }
            })
            .collect())
    }

    #[tokio::test]
    async fn groups_over_the_cap_and_attributes_across_groups() {
        let judge = ScriptedJudge::new(by_name);
        let adapter = ExtractorDedupAdapter::new(judge.clone());
        let pairs: Vec<DuplicatePair> = (0..(MAX_PAIRS_PER_CALL + 4))
            .map(|i| pair(&format!("{}{i}", if i % 3 == 0 { "dup" } else { "no" })))
            .collect();
        let verdicts = adapter.judge_batch(&pairs).await;
        assert_eq!(*judge.group_sizes.lock().unwrap(), vec![16, 4]);
        for (i, v) in verdicts.iter().enumerate() {
            let want = if i % 3 == 0 {
                DedupVerdict::Duplicate
            } else {
                DedupVerdict::Distinct
            };
            assert_eq!(*v, want, "pair {i}");
        }
    }

    #[tokio::test]
    async fn error_short_answer_and_timeout_are_unknown() {
        let err = ScriptedJudge::new(|_| Err(Error::Ipc("boom".into())));
        let a = ExtractorDedupAdapter::new(err);
        assert_eq!(
            a.judge_batch(&[pair("dup1"), pair("dup2")]).await,
            vec![DedupVerdict::Unknown; 2]
        );

        let short = ScriptedJudge::new(|_| Ok(vec![DedupVerdict::Duplicate]));
        let a = ExtractorDedupAdapter::new(short);
        assert_eq!(
            a.judge_batch(&[pair("dup1"), pair("dup2")]).await,
            vec![DedupVerdict::Unknown; 2]
        );

        let slow = Arc::new(ScriptedJudge {
            calls: AtomicUsize::new(0),
            group_sizes: Mutex::new(vec![]),
            answer: Box::new(by_name),
            delay: Duration::from_millis(500),
        });
        let a = ExtractorDedupAdapter::with_timeout(slow, Duration::from_millis(20));
        assert_eq!(
            a.judge_batch(&[pair("dup1")]).await,
            vec![DedupVerdict::Unknown]
        );
    }

    #[tokio::test]
    async fn definite_verdicts_are_cached_unknown_is_not() {
        let judge = ScriptedJudge::new(by_name);
        let a = ExtractorDedupAdapter::new(judge.clone());
        let p = [pair("dup1")];
        a.judge_batch(&p).await;
        a.judge_batch(&p).await;
        assert_eq!(judge.calls.load(Ordering::SeqCst), 1);

        let flaky = ScriptedJudge::new(|_| Err(Error::Ipc("boom".into())));
        let a = ExtractorDedupAdapter::new(flaky.clone());
        a.judge_batch(&p).await;
        a.judge_batch(&p).await;
        assert_eq!(flaky.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn changed_summary_is_a_new_cache_key() {
        let judge = ScriptedJudge::new(by_name);
        let a = ExtractorDedupAdapter::new(judge.clone());
        let mut p = pair("dup1");
        a.judge_batch(std::slice::from_ref(&p)).await;
        p.existing.summary = "now different".to_string();
        a.judge_batch(std::slice::from_ref(&p)).await;
        assert_eq!(judge.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn passthrough_confirms_everything_and_reports_veto_only() {
        let a = PassthroughDedupAdapter;
        assert_eq!(
            a.judge_batch(&[pair("x"), pair("y")]).await,
            vec![DedupVerdict::Duplicate; 2]
        );
        assert_eq!(a.mode(), DedupMode::VetoOnly);
        assert_eq!(
            ExtractorDedupAdapter::new(ScriptedJudge::new(by_name)).mode(),
            DedupMode::LlmVerified
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// The startup log line (issue #650) must track the same `LCG_DEDUP_LLM` switch that
    /// `AppState::from_env` uses to pick the adapter.
    #[test]
    fn dedup_mode_description_tracks_lcg_dedup_llm() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (new, old) = (
            std::env::var("LCG_DEDUP_LLM").ok(),
            std::env::var("GRAPHITI_DEDUP_LLM").ok(),
        );
        std::env::remove_var("LCG_DEDUP_LLM");
        std::env::remove_var("GRAPHITI_DEDUP_LLM");
        assert_eq!(dedup_mode_description(), "passthrough + identifier veto");
        std::env::set_var("LCG_DEDUP_LLM", "1");
        assert_eq!(
            dedup_mode_description(),
            "local-adapter (LCG_DEDUP_LLM) + identifier veto"
        );
        std::env::remove_var("LCG_DEDUP_LLM");
        if let Some(v) = new {
            std::env::set_var("LCG_DEDUP_LLM", v);
        }
        if let Some(v) = old {
            std::env::set_var("GRAPHITI_DEDUP_LLM", v);
        }
    }
}
