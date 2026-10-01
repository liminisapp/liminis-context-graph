use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::{
    dedup_judge::{DedupVerdict, DuplicatePair},
    env::lcg_env_var,
    error::Error,
    extractor::{AnthropicExtractor, ExtractOptions, Extractor},
    telemetry::{now_ms, TelemetryEvent, TelemetrySink},
    types::ExtractionOutcome,
};

/// Routes extraction calls to a primary LLM with optional fallback (AD-5).
///
/// Parses `LCG_EXTRACTION_LLM` on `:` — first token is the primary model,
/// second (optional) is the fallback model. Both use the same `ANTHROPIC_API_KEY`.
///
/// When a fallback is configured: on the first primary failure, emits
/// `TelemetryEvent::LlmFallback` once and routes all subsequent calls to the
/// fallback for the rest of the process lifetime.
///
/// When no fallback is configured: primary errors are returned to the caller
/// without latching `primary_failed`, so transient failures do not permanently
/// disable extraction.
pub struct LlmRouter {
    primary: Arc<dyn Extractor>,
    primary_model_name: String,
    fallback: Option<Arc<dyn Extractor>>,
    fallback_model_name: String,
    primary_failed: AtomicBool,
    sink: Arc<dyn TelemetrySink>,
}

impl LlmRouter {
    /// Constructs directly from extractor instances and their model names — for tests and for
    /// callers that already hold a concrete `Arc<dyn Extractor>` (e.g. `OaiExtractor` as
    /// primary with no fallback). Model names are passed explicitly rather than derived via a
    /// trait method, since callers already know the string at construction time and not every
    /// `Extractor` impl (test doubles included) has a meaningful model name to report.
    pub fn new(
        primary: Arc<dyn Extractor>,
        primary_model_name: String,
        fallback: Option<Arc<dyn Extractor>>,
        fallback_model_name: String,
        sink: Arc<dyn TelemetrySink>,
    ) -> Self {
        Self {
            primary,
            primary_model_name,
            fallback,
            fallback_model_name,
            primary_failed: AtomicBool::new(false),
            sink,
        }
    }

    pub fn from_env(sink: Arc<dyn TelemetrySink>) -> Self {
        Self::from_env_with(sink, |extractor, _model_name| extractor)
    }

    /// Like [`Self::from_env`], but passes each constructed leaf extractor (primary, and
    /// fallback if configured) through `wrap` — along with its model name — before storing it.
    /// `from_env` delegates here with an identity closure, so its behavior is byte-for-byte
    /// unchanged; this seam exists so a caller (e.g. `main.rs` under `LCG_RECORD_LLM`) can wrap
    /// each leaf in a `RecordingExtractor` individually, producing distinguishable,
    /// correctly-attributed cassette entries on primary→fallback failover (#232 User Story 4)
    /// without duplicating this function's `LCG_EXTRACTION_LLM` parsing logic elsewhere.
    pub fn from_env_with(
        sink: Arc<dyn TelemetrySink>,
        wrap: impl Fn(Arc<dyn Extractor>, &str) -> Arc<dyn Extractor>,
    ) -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        // deprecated: remove in Phase B (see #59)
        let spec = lcg_env_var("LCG_EXTRACTION_LLM", "GRAPHITI_EXTRACTION_LLM")
            .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());

        let mut parts = spec.splitn(2, ':');
        let primary_model = parts
            .next()
            .unwrap_or("claude-haiku-4-5-20251001")
            .to_string();
        let fallback_model = parts.next().map(str::to_string);

        let primary: Arc<dyn Extractor> = wrap(
            Arc::new(AnthropicExtractor::with_model(
                primary_model.clone(),
                api_key.clone(),
                Arc::clone(&sink),
            )),
            &primary_model,
        );
        let fallback_model_name = fallback_model.clone().unwrap_or_default();
        let fallback: Option<Arc<dyn Extractor>> = fallback_model.map(|m| {
            wrap(
                Arc::new(AnthropicExtractor::with_model(
                    m.clone(),
                    api_key,
                    Arc::clone(&sink),
                )),
                &m,
            )
        });

        Self {
            primary,
            primary_model_name: primary_model,
            fallback,
            fallback_model_name,
            primary_failed: AtomicBool::new(false),
            sink,
        }
    }

    async fn do_classify_entities(
        &self,
        entities: &[(&str, &str)],
        allowed_types: Option<&[String]>,
    ) -> Result<Vec<String>, Error> {
        if !self
            .primary_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            match self
                .primary
                .classify_entities(entities, allowed_types)
                .await
            {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if let Some(fb) = &self.fallback {
                        if self
                            .primary_failed
                            .compare_exchange(
                                false,
                                true,
                                std::sync::atomic::Ordering::AcqRel,
                                std::sync::atomic::Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            self.sink.emit(TelemetryEvent::LlmFallback {
                                ts_ms: now_ms(),
                                role: "classification".to_string(),
                                primary_model: self.primary_model_name.clone(),
                                fallback_model: self.fallback_model_name.clone(),
                                error_reason: err.to_string(),
                            });
                        }
                        return fb.classify_entities(entities, allowed_types).await;
                    }
                    return Err(err);
                }
            }
        }
        if let Some(fb) = &self.fallback {
            fb.classify_entities(entities, allowed_types).await
        } else {
            Err(Error::Ipc(
                "BUG: primary_failed set without fallback".to_string(),
            ))
        }
    }

    /// Unlike the extraction/classification routes, a failed primary here does NOT latch
    /// `primary_failed`: consolidation is best-effort (the merge path degrades to a deterministic
    /// fallback on any error), so one transient failure must not demote the primary model for
    /// every other role for the rest of the process. The fallback is tried for this call only.
    async fn do_consolidate_summary(
        &self,
        entity_name: &str,
        existing: &str,
        incoming: &str,
    ) -> Result<String, Error> {
        let primary_ok = !self
            .primary_failed
            .load(std::sync::atomic::Ordering::Acquire);
        if primary_ok {
            match self
                .primary
                .consolidate_summary(entity_name, existing, incoming)
                .await
            {
                Ok(result) => return Ok(result),
                Err(err) => {
                    return match &self.fallback {
                        Some(fb) => {
                            fb.consolidate_summary(entity_name, existing, incoming)
                                .await
                        }
                        None => Err(err),
                    };
                }
            }
        }
        match &self.fallback {
            Some(fb) => {
                fb.consolidate_summary(entity_name, existing, incoming)
                    .await
            }
            None => Err(Error::Ipc(
                "BUG: primary_failed set without fallback".to_string(),
            )),
        }
    }

    async fn do_classify_relations(
        &self,
        edges: &[(&str, &str)],
        allowed_types: &[(String, Option<String>)],
    ) -> Result<Vec<String>, Error> {
        if !self
            .primary_failed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            match self.primary.classify_relations(edges, allowed_types).await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if let Some(fb) = &self.fallback {
                        if self
                            .primary_failed
                            .compare_exchange(
                                false,
                                true,
                                std::sync::atomic::Ordering::AcqRel,
                                std::sync::atomic::Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            self.sink.emit(TelemetryEvent::LlmFallback {
                                ts_ms: now_ms(),
                                role: "relation_classification".to_string(),
                                primary_model: self.primary_model_name.clone(),
                                fallback_model: self.fallback_model_name.clone(),
                                error_reason: err.to_string(),
                            });
                        }
                        return fb.classify_relations(edges, allowed_types).await;
                    }
                    return Err(err);
                }
            }
        }
        if let Some(fb) = &self.fallback {
            fb.classify_relations(edges, allowed_types).await
        } else {
            Err(Error::Ipc(
                "BUG: primary_failed set without fallback".to_string(),
            ))
        }
    }

    async fn do_extract(&self, opts: ExtractOptions<'_>) -> Result<ExtractionOutcome, Error> {
        // primary_failed is only ever set to true when a fallback is configured, so if it is
        // true here there must be a fallback to try.
        if !self.primary_failed.load(Ordering::Acquire) {
            match self.primary.extract(opts).await {
                Ok(result) => return Ok(result),
                Err(err) => {
                    if let Some(fb) = &self.fallback {
                        // Redirect to fallback and log the transition exactly once per session.
                        if self
                            .primary_failed
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            self.sink.emit(TelemetryEvent::LlmFallback {
                                ts_ms: now_ms(),
                                role: "extraction".to_string(),
                                primary_model: self.primary_model_name.clone(),
                                fallback_model: self.fallback_model_name.clone(),
                                error_reason: err.to_string(),
                            });
                            eprintln!(
                                "liminis-context-graph: extraction primary '{}' failed ({}); switching to fallback '{}' for this session",
                                self.primary_model_name, err, self.fallback_model_name
                            );
                        }
                        return fb.extract(opts).await;
                    }
                    // No fallback — return the error without setting primary_failed so that
                    // transient failures do not permanently disable the primary.
                    return Err(err);
                }
            }
        }

        // primary_failed is true, which means a fallback is configured.
        if let Some(fb) = &self.fallback {
            fb.extract(opts).await
        } else {
            // Unreachable: primary_failed is only set when fallback.is_some().
            Err(Error::Ipc(
                "BUG: primary_failed set without fallback".to_string(),
            ))
        }
    }
    /// Primary → fallback for the dedup judgement (#652). Unlike `extract`/`classify_*`, a
    /// failure here never latches `primary_failed` or emits `LlmFallback`: dedup is advisory
    /// (every failure resolves to "not a duplicate"), so a bad judge call must not permanently
    /// reroute *extraction* to the fallback model. Once extraction has latched, dedup follows it.
    async fn do_judge_duplicates(
        &self,
        pairs: &[DuplicatePair],
    ) -> Result<Vec<DedupVerdict>, Error> {
        if !self.primary_failed.load(Ordering::Acquire) {
            let mut verdicts = match self.primary.judge_duplicates(pairs).await {
                Ok(v) if v.len() == pairs.len() => v,
                Ok(_) => vec![DedupVerdict::Unknown; pairs.len()],
                Err(err) => {
                    return match &self.fallback {
                        Some(fb) => fb.judge_duplicates(pairs).await,
                        None => Err(err),
                    };
                }
            };
            // A malformed primary answer parses to `Unknown` rather than an error: re-ask the
            // fallback only for those pairs, keeping the primary's definite verdicts.
            if let Some(fb) = &self.fallback {
                let unresolved: Vec<usize> = (0..pairs.len())
                    .filter(|&i| verdicts[i] == DedupVerdict::Unknown)
                    .collect();
                if !unresolved.is_empty() {
                    let subset: Vec<DuplicatePair> =
                        unresolved.iter().map(|&i| pairs[i].clone()).collect();
                    // A failed or short fallback answer leaves the pairs `Unknown` (not a duplicate).
                    if let Ok(fv) = fb.judge_duplicates(&subset).await {
                        if fv.len() == subset.len() {
                            for (&i, v) in unresolved.iter().zip(fv) {
                                verdicts[i] = v;
                            }
                        }
                    }
                }
            }
            return Ok(verdicts);
        }
        match &self.fallback {
            Some(fb) => fb.judge_duplicates(pairs).await,
            // Unreachable: primary_failed is only set when fallback.is_some().
            None => Err(Error::Ipc(
                "BUG: primary_failed set without fallback".to_string(),
            )),
        }
    }
}

impl Extractor for LlmRouter {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        Box::pin(self.do_extract(opts))
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(self.do_classify_entities(entities, allowed_types))
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        Box::pin(self.do_classify_relations(edges, allowed_types))
    }

    fn consolidate_summary<'a>(
        &'a self,
        entity_name: &'a str,
        existing: &'a str,
        incoming: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        Box::pin(self.do_consolidate_summary(entity_name, existing, incoming))
    }

    fn judge_duplicates<'a>(
        &'a self,
        pairs: &'a [DuplicatePair],
    ) -> BoxFuture<'a, Result<Vec<DedupVerdict>, Error>> {
        Box::pin(self.do_judge_duplicates(pairs))
    }

    fn is_configured(&self) -> bool {
        self.primary.is_configured()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::CaptureSink;
    use crate::types::{ExtractedEntity, ExtractionResult, SourceType};
    use std::sync::atomic::AtomicUsize;

    /// Always fails `extract()` with the given message, counting how many times it was called
    /// — used to stand in for real edge-budget-exhaustion (#307 FR-004 now returns `Err` from
    /// that path, same as any other extraction failure) without needing a live HTTP stub.
    struct FailingExtractor {
        message: &'static str,
        calls: AtomicUsize,
    }

    impl FailingExtractor {
        fn new(message: &'static str) -> Self {
            Self {
                message,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Extractor for FailingExtractor {
        fn extract<'a>(
            &'a self,
            _opts: ExtractOptions<'a>,
        ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Err(Error::Ipc(self.message.to_string())) })
        }

        fn classify_entities<'a>(
            &'a self,
            _entities: &'a [(&'a str, &'a str)],
            _allowed_types: Option<&'a [String]>,
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Err(Error::Ipc(self.message.to_string())) })
        }

        fn classify_relations<'a>(
            &'a self,
            _edges: &'a [(&'a str, &'a str)],
            _allowed_types: &'a [(String, Option<String>)],
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Err(Error::Ipc(self.message.to_string())) })
        }
    }

    /// Always succeeds `extract()` with a fixed one-entity result, counting how many times it
    /// was called — used as the fallback in the permanent-switch tests below.
    struct SucceedingExtractor {
        calls: AtomicUsize,
    }

    impl SucceedingExtractor {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Extractor for SucceedingExtractor {
        fn extract<'a>(
            &'a self,
            _opts: ExtractOptions<'a>,
        ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(ExtractionResult {
                    entities: vec![ExtractedEntity {
                        name: "Alice".to_string(),
                        entity_type: "Person".to_string(),
                        summary: "a person".to_string(),
                        original_entity_type: None,
                    }],
                    edges: vec![],
                }
                .into())
            })
        }

        fn classify_entities<'a>(
            &'a self,
            _entities: &'a [(&'a str, &'a str)],
            _allowed_types: Option<&'a [String]>,
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn classify_relations<'a>(
            &'a self,
            _edges: &'a [(&'a str, &'a str)],
            _allowed_types: &'a [(String, Option<String>)],
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Ok(vec![]) })
        }
    }

    fn opts(episode_body: &str) -> ExtractOptions<'_> {
        ExtractOptions {
            episode_body,
            group_id: "g1",
            source_type: SourceType::Text,
            custom_instructions: None,
            reference_time: "2026-01-01T00:00:00Z",
            ontology: None,
            chunk_key: None,
        }
    }

    #[tokio::test]
    async fn edge_budget_exhaustion_error_triggers_the_permanent_fallback_switch() {
        // #307 Research's top-flagged risk, resolved as a deliberate no-op in llm_router.rs
        // (Plan's "LlmRouter interaction" decision): edge-budget exhaustion now returns Err
        // like any other extraction failure (FR-004), and this Err is treated exactly like any
        // other primary failure — it trips the same one-shot permanent-fallback switch. This is
        // intentional, not an oversight: FR-002's proportional budget should make exhaustion
        // rare, so treating it identically to a transport/HTTP failure is the same choice
        // FR-004 already made when it said "matches the entity path".
        let primary = Arc::new(FailingExtractor::new(
            "edge extraction budget exhausted after retry",
        ));
        let fallback = Arc::new(SucceedingExtractor::new());
        let sink = Arc::new(CaptureSink::new());

        let router = LlmRouter::new(
            Arc::clone(&primary) as Arc<dyn Extractor>,
            "primary-model".to_string(),
            Some(Arc::clone(&fallback) as Arc<dyn Extractor>),
            "fallback-model".to_string(),
            Arc::clone(&sink) as Arc<dyn TelemetrySink>,
        );

        // First call: primary's edge-exhaustion Err triggers the switch; the fallback serves
        // this call and every one after it.
        let first = router.extract(opts("chunk one")).await.unwrap();
        assert_eq!(first.result.entities.len(), 1);
        assert_eq!(primary.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback.calls.load(Ordering::SeqCst), 1);

        let fallback_events: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|e| matches!(e, TelemetryEvent::LlmFallback { .. }))
            .collect();
        assert_eq!(
            fallback_events.len(),
            1,
            "exactly one LlmFallback event for the switch"
        );

        // Second call: primary is never retried — the switch is permanent for the rest of the
        // process lifetime, even though the second call has nothing to do with the original
        // edge-truncation event.
        let second = router.extract(opts("chunk two")).await.unwrap();
        assert_eq!(second.result.entities.len(), 1);
        assert_eq!(
            primary.calls.load(Ordering::SeqCst),
            1,
            "primary must not be called again once the switch has tripped"
        );
        assert_eq!(fallback.calls.load(Ordering::SeqCst), 2);

        // No second LlmFallback event — the switch fires exactly once per session.
        let fallback_events_after: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|e| matches!(e, TelemetryEvent::LlmFallback { .. }))
            .collect();
        assert_eq!(fallback_events_after.len(), 1);
    }

    #[tokio::test]
    async fn primary_failure_without_fallback_returns_err_without_latching() {
        // Sanity check for the no-fallback branch this test module otherwise doesn't touch:
        // an edge-exhaustion-shaped Err with no fallback configured must propagate to the
        // caller and must not set primary_failed (there being no fallback to switch to).
        let primary = Arc::new(FailingExtractor::new(
            "edge extraction budget exhausted after retry",
        ));
        let sink = Arc::new(CaptureSink::new());
        let router = LlmRouter::new(
            Arc::clone(&primary) as Arc<dyn Extractor>,
            "primary-model".to_string(),
            None,
            String::new(),
            Arc::clone(&sink) as Arc<dyn TelemetrySink>,
        );

        let err = router.extract(opts("chunk one")).await.unwrap_err();
        assert!(matches!(err, Error::Ipc(_)));
        assert_eq!(primary.calls.load(Ordering::SeqCst), 1);
        assert!(
            sink.events().is_empty(),
            "no fallback means no LlmFallback event"
        );
    }

    /// Judge that answers a fixed verdict vector (cycled to the pair count), counting its calls.
    struct FixedJudge {
        verdicts: Vec<DedupVerdict>,
        calls: AtomicUsize,
    }

    impl Extractor for FixedJudge {
        fn extract<'a>(
            &'a self,
            _opts: ExtractOptions<'a>,
        ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
            Box::pin(async { Err(Error::Ipc("unused".into())) })
        }

        fn classify_entities<'a>(
            &'a self,
            _entities: &'a [(&'a str, &'a str)],
            _allowed_types: Option<&'a [String]>,
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn classify_relations<'a>(
            &'a self,
            _edges: &'a [(&'a str, &'a str)],
            _allowed_types: &'a [(String, Option<String>)],
        ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
            Box::pin(async { Ok(vec![]) })
        }

        fn judge_duplicates<'a>(
            &'a self,
            pairs: &'a [DuplicatePair],
        ) -> BoxFuture<'a, Result<Vec<DedupVerdict>, Error>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let out = (0..pairs.len())
                .map(|i| self.verdicts[i % self.verdicts.len()])
                .collect();
            Box::pin(async move { Ok(out) })
        }
    }

    #[tokio::test]
    async fn judge_duplicates_retries_only_unknown_pairs_on_fallback() {
        use crate::dedup_judge::DedupSide;
        let side = |n: &str| DedupSide {
            name: n.to_string(),
            entity_type: String::new(),
            summary: String::new(),
        };
        let pairs: Vec<DuplicatePair> = ["a", "b"]
            .iter()
            .map(|n| DuplicatePair {
                existing: side(n),
                incoming: side("x"),
            })
            .collect();
        // Primary: pair 0 definite "duplicate", pair 1 malformed (Unknown).
        let primary = Arc::new(FixedJudge {
            verdicts: vec![DedupVerdict::Duplicate, DedupVerdict::Unknown],
            calls: AtomicUsize::new(0),
        });
        let fallback = Arc::new(FixedJudge {
            verdicts: vec![DedupVerdict::Distinct],
            calls: AtomicUsize::new(0),
        });
        let router = LlmRouter::new(
            Arc::clone(&primary) as Arc<dyn Extractor>,
            "p".to_string(),
            Some(Arc::clone(&fallback) as Arc<dyn Extractor>),
            "f".to_string(),
            Arc::new(CaptureSink::new()) as Arc<dyn TelemetrySink>,
        );
        let v = router.judge_duplicates(&pairs).await.unwrap();
        assert_eq!(v, vec![DedupVerdict::Duplicate, DedupVerdict::Distinct]);
        assert_eq!(fallback.calls.load(Ordering::SeqCst), 1);
        assert!(!router.primary_failed.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn judge_duplicates_falls_back_without_latching_extraction() {
        let primary = Arc::new(FailingExtractor::new("judge boom"));
        let fallback = Arc::new(SucceedingExtractor::new());
        let sink = Arc::new(CaptureSink::new());
        let router = LlmRouter::new(
            Arc::clone(&primary) as Arc<dyn Extractor>,
            "primary-model".to_string(),
            Some(Arc::clone(&fallback) as Arc<dyn Extractor>),
            "fallback-model".to_string(),
            Arc::clone(&sink) as Arc<dyn TelemetrySink>,
        );
        // FailingExtractor / SucceedingExtractor use the trait default, which errors — so with a
        // fallback both fail, but the router must not latch or emit a fallback event.
        assert!(router.judge_duplicates(&[]).await.is_err());
        assert!(!router.primary_failed.load(Ordering::Acquire));
        assert!(sink.events().is_empty());
        assert!(router.is_configured());
    }
}
