//! Network-free end-to-end coverage of `lcg-eval retrieval` (#754): the checked-in fixture, a
//! canned LCG JSON-RPC responder, a fake qmd runner, and a canned evidence judge. Nothing here
//! needs qmd, a model, or a network.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use lcg_eval::retrieval::evidence_judge::{CachedEvidenceJudge, EvidenceJudge};
use lcg_eval::retrieval::fixture::{self, Category, Fixture};
use lcg_eval::retrieval::lcg::{self, IngestMap, LcgEntitiesFacts, LcgPassages, RpcTransport};
use lcg_eval::retrieval::qmd::{self, ProcessRunner, QmdOutput, QmdRetriever, QmdRunner, Skipped};
use lcg_eval::retrieval::report::{IngestCost, Reading, Report, RunMeta, Status};
use lcg_eval::retrieval::{evaluate, resolve, Slot};

const K: usize = 10;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval")
}

fn words(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

/// Ranks `(id, text)` docs by distinct-word overlap with the query (ties by id), top `k`.
fn lexical_rank(query: &str, docs: &[(String, String)], k: usize) -> Vec<(String, String)> {
    let q = words(query);
    let mut scored: Vec<(usize, &(String, String))> = docs
        .iter()
        .map(|d| (words(&d.1).intersection(&q).count(), d))
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1 .0.cmp(&b.1 .0)));
    scored.into_iter().take(k).map(|(_, d)| d.clone()).collect()
}

/// Canned LCG service: stores ingested chunks and answers passage search lexically. Entity and
/// fact reads return nothing (so those configurations score zero, which is a valid outcome).
struct FakeLcg {
    chunks: Mutex<Vec<(String, String)>>, // (source_description, text)
    n: Mutex<usize>,
    /// note id → episode uuids handed out for it, so supersede can answer like the service.
    episodes: Mutex<std::collections::HashMap<String, Vec<String>>>,
}
impl FakeLcg {
    fn new() -> Self {
        Self {
            chunks: Mutex::new(vec![]),
            n: Mutex::new(0),
            episodes: Mutex::new(Default::default()),
        }
    }
}
impl RpcTransport for FakeLcg {
    fn call<'a>(&'a self, method: &'a str, p: Value) -> BoxFuture<'a, Result<Value, String>> {
        let out = match method {
            "knowledge_process_chunk" => {
                let mut n = self.n.lock().unwrap();
                *n += 1;
                self.chunks.lock().unwrap().push((
                    format!(
                        "{}:{}",
                        p["source_file"].as_str().unwrap(),
                        p["chunk_id"].as_str().unwrap()
                    ),
                    p["chunk_text"].as_str().unwrap().to_string(),
                ));
                self.episodes
                    .lock()
                    .unwrap()
                    .entry(p["source_file"].as_str().unwrap().to_string())
                    .or_default()
                    .push(format!("ep-{n}"));
                Ok(json!({"episode_uuid": format!("ep-{n}")}))
            }
            "knowledge_supersede_document" => {
                let old = p["superseded_doc_id"].as_str().unwrap();
                let uuids = self
                    .episodes
                    .lock()
                    .unwrap()
                    .get(old)
                    .cloned()
                    .unwrap_or_default();
                Ok(json!({
                    "success": true, "already_recorded": false,
                    "superseded_count": uuids.len(), "superseded_uuids": uuids,
                }))
            }
            "knowledge_build_indices" => Ok(json!({})),
            "knowledge_get_entities_by_source" => Ok(json!({"nodes": []})),
            "knowledge_search_passages" => {
                assert_eq!(
                    p["num_results"], K,
                    "every configuration must use the shared k"
                );
                let docs: Vec<(String, String)> = self.chunks.lock().unwrap().clone();
                let hits = lexical_rank(p["query"].as_str().unwrap(), &docs, K);
                let passages: Vec<Value> = hits
                    .into_iter()
                    .map(|(sd, text)| json!({"source_description": sd, "content": text}))
                    .collect();
                Ok(json!({"passages": passages}))
            }
            "knowledge_find_entities" => Ok(json!({"nodes": []})),
            "knowledge_find_relationships" => Ok(json!({"edges": []})),
            other => Err(format!("unexpected method {other}")),
        };
        Box::pin(async move { out })
    }
}

/// Fake qmd: lexical ranking over the fixture notes, emitted as qmd's JSON.
struct FakeQmd {
    notes: Vec<(String, String)>,
    log: Mutex<Vec<Vec<String>>>,
}
impl QmdRunner for FakeQmd {
    fn run<'a>(&'a self, args: &'a [String]) -> BoxFuture<'a, Result<QmdOutput, Skipped>> {
        self.log.lock().unwrap().push(args.to_vec());
        let ok = |s: String| QmdOutput {
            success: true,
            stdout: s,
            stderr: String::new(),
        };
        let out = if args.iter().any(|a| a == "query") {
            let n: usize = args[args.iter().position(|a| a == "-n").unwrap() + 1]
                .parse()
                .unwrap();
            assert_eq!(n, K);
            let q = args.last().unwrap();
            let rows: Vec<Value> = lexical_rank(q, &self.notes, n)
                .into_iter()
                .map(|(id, text)| json!({"docid": "#x", "score": 0.5, "file": format!("qmd://nb/{id}.md"), "snippet": text}))
                .collect();
            ok(serde_json::to_string(&rows).unwrap())
        } else if args.iter().any(|a| a == "--version") {
            ok("qmd 9.9.9\n".into())
        } else {
            ok(String::new())
        };
        Box::pin(async move { Ok(out) })
    }
}

struct SubstringJudge;
impl EvidenceJudge for SubstringJudge {
    fn model(&self) -> &str {
        "test-judge"
    }
    fn supports<'a>(&'a self, k: &'a str, e: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        // Supports when most of the key fact's words appear in the text.
        let kw = words(k);
        let hit = kw.intersection(&words(e)).count();
        Box::pin(async move { Ok(hit * 10 >= kw.len() * 8) })
    }
}

fn meta(fx: &Fixture) -> RunMeta {
    let plan_counts = resolve(
        &match lcg_eval::retrieval::cli::parse_args(&[]).unwrap() {
            lcg_eval::retrieval::cli::RetrievalMode::Run(a) => *a,
            _ => unreachable!(),
        },
        fx,
    )
    .category_counts;
    RunMeta {
        corpus_version: fx.manifest.corpus_version.clone(),
        corpus_hash: fx.corpus_hash.clone(),
        questions_version: fx.questions_version.clone(),
        questions_hash: fx.questions_hash.clone(),
        k: K,
        category_counts: plan_counts,
        ..Default::default()
    }
}

async fn full_run(
    fx: &Fixture,
    qmd_runner: Option<Arc<dyn QmdRunner>>,
    judge_path: &Path,
) -> Report {
    let transport: Arc<dyn RpcTransport> = Arc::new(FakeLcg::new());
    let map: Arc<IngestMap> = Arc::new(lcg::ingest(transport.as_ref(), fx).await.unwrap());
    let mut slots = vec![
        Slot::Run(Box::new(LcgPassages {
            t: transport.clone(),
            mode: lcg::PassageMode::Vector,
        })),
        Slot::Run(Box::new(LcgEntitiesFacts {
            t: transport.clone(),
            map: map.clone(),
            belief: false,
        })),
    ];
    let mut qmd_seconds = None;
    match qmd_runner {
        Some(r) => match qmd::prepare(r.as_ref(), "idx", &fx.notes_dir()).await {
            Ok(info) => {
                qmd_seconds = Some(info.index_seconds);
                for (id, rerank) in [("qmd", false), ("qmd-rerank", true)] {
                    slots.push(Slot::Run(Box::new(QmdRetriever {
                        runner: r.clone(),
                        index_name: "idx".into(),
                        rerank,
                        id,
                    })));
                }
            }
            Err(Skipped(why)) => {
                for id in ["qmd", "qmd-rerank"] {
                    slots.push(Slot::Skipped {
                        id: id.into(),
                        description: id.into(),
                        reason: why.clone(),
                    });
                }
            }
        },
        None => unreachable!(),
    }
    let inner = SubstringJudge;
    let judge = CachedEvidenceJudge::load(&inner, judge_path).unwrap();
    let configs = evaluate(fx, K, slots, Some(&judge), "no judge").await;
    Report {
        meta: meta(fx),
        ingest: IngestCost {
            lcg_seconds: Some(map.seconds),
            lcg_chunks: map.chunks,
            lcg_extraction_requests: map.extraction_requests,
            lcg_rpc_calls: map.rpc_calls,
            lcg_unconfirmed_entity_links: map.unconfirmed_entity_links,
            lcg_supersessions: map.supersessions,
            lcg_superseded_episodes: map.superseded_episodes,
            qmd_seconds,
        },
        configs,
    }
}

fn fake_qmd(fx: &Fixture) -> Arc<dyn QmdRunner> {
    Arc::new(FakeQmd {
        notes: fx
            .notes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        log: Mutex::new(vec![]),
    })
}

// ── fixture sanity (SC-003, FR-011, FR-012) ──────────────────────────────────────

#[test]
fn default_qmd_index_name_changes_with_the_corpus_hash() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let name = lcg_eval::retrieval::default_qmd_index(&fx);
    assert!(name.starts_with("lcg-eval-"), "{name}");
    assert!(
        name.ends_with(&fx.corpus_hash[..8]),
        "index name must carry the corpus hash so an edited corpus gets a fresh qmd collection: {name}"
    );
}

#[test]
fn checked_in_fixture_loads_and_covers_every_category() {
    let fx = fixture::load(&fixture_dir()).expect("fixture must validate");
    let mut by: HashMap<Category, usize> = HashMap::new();
    for q in &fx.questions {
        *by.entry(q.category).or_default() += 1;
    }
    for c in Category::ALL {
        assert!(
            by.get(&c).copied().unwrap_or(0) >= 10,
            "{} has too few questions: {by:?}",
            c.label()
        );
    }
    assert!(
        fx.manifest.supersedes.len() >= 6,
        "need at least 6 superseded decision pairs"
    );
    assert!(
        fx.questions.iter().any(|q| q.negative),
        "need negative questions"
    );
}

#[test]
fn every_temporal_question_pairs_a_current_note_with_the_stale_note_it_replaced() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    for q in fx
        .questions
        .iter()
        .filter(|q| q.category == Category::Temporal)
    {
        for cur in &q.current {
            for stale in &q.stale {
                assert!(
                    fx.manifest
                        .supersedes
                        .iter()
                        .any(|s| &s.new_note == cur && &s.old_note == stale),
                    "{}: {stale} is not recorded as superseded by {cur} in the manifest",
                    q.id
                );
            }
        }
    }
}

#[test]
fn every_evidence_key_fact_is_actually_stated_in_its_note() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let judge = SubstringJudge;
    for q in &fx.questions {
        for e in &q.evidence {
            let ok = futures::executor::block_on(judge.supports(&e.key_fact, &fx.notes[&e.note]))
                .unwrap();
            assert!(
                ok,
                "{}: key_fact {:?} not supported by note {}",
                q.id, e.key_fact, e.note
            );
        }
    }
}

#[test]
fn malformed_and_dangling_questions_are_rejected_with_the_question_id() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("notes")).unwrap();
    std::fs::write(
        d.path().join("manifest.json"),
        r#"{"corpus_version":"v","notes":[{"id":"a"}]}"#,
    )
    .unwrap();
    std::fs::write(d.path().join("notes/a.md"), "# A\nbody").unwrap();
    std::fs::write(
        d.path().join("questions.jsonl"),
        "{\"id\":\"ok\",\"category\":\"direct\",\"text\":\"t\",\"relevant_notes\":[{\"note\":\"a\"}]}\n\
         {\"id\":\"dangling\",\"category\":\"direct\",\"text\":\"t\",\"relevant_notes\":[{\"note\":\"missing-note\"}]}\n",
    )
    .unwrap();
    let e = fixture::load(d.path()).unwrap_err();
    assert!(e.contains("dangling") && e.contains("missing-note"), "{e}");
}

// ── end-to-end ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn full_comparison_produces_one_table_with_every_configuration_at_one_k() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let d = tempfile::tempdir().unwrap();
    let r = full_run(&fx, Some(fake_qmd(&fx)), &d.path().join("j.jsonl")).await;
    let md = r.render_markdown();
    for id in ["lcg-passages", "lcg-entities-facts", "qmd", "qmd-rerank"] {
        assert!(md.contains(&format!("`{id}`")), "{id} missing:\n{md}");
    }
    for cat in Category::ALL {
        assert!(md.contains(&format!("### {} (n = ", cat.label())), "{md}");
        assert!(
            !r.winners(Some(cat)).is_empty(),
            "{} has no winner",
            cat.label()
        );
    }
    // The hypotheses section gives a reading when both sides ran (never "cannot be read"); with
    // 10-12 questions per category it is usually "not distinguishable from noise".
    let (h1, h2) = r.hypotheses();
    assert!(h1 != Reading::CannotRead && h2 != Reading::CannotRead);
    // Lexical fakes answer direct lookups, so the lexical systems must score well above zero.
    let Status::Ran(passages) = &r.configs[0].status else {
        panic!()
    };
    assert!(
        passages.overall.recall.unwrap() > 0.5,
        "{:?}",
        passages.overall
    );
    // Answer questions got grounded judging; the canned judge makes it non-trivial.
    assert!(passages.overall.n_grounded >= 10);
    assert!(passages.overall.grounded_recall.unwrap() > 0.0);
    // Query-time model calls: LCG makes them, qmd does not expose them.
    let j = r.to_json();
    assert_eq!(j["configs"][0]["model_calls_per_query"], 1.0);
    assert!(j["configs"][2]["model_calls_per_query"].is_null());
    assert!(md.contains("n/a (not exposed)"));
}

#[tokio::test]
async fn relevance_metrics_reproduce_exactly_on_rerun() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let d = tempfile::tempdir().unwrap();
    let a = full_run(&fx, Some(fake_qmd(&fx)), &d.path().join("a.jsonl")).await;
    let b = full_run(&fx, Some(fake_qmd(&fx)), &d.path().join("b.jsonl")).await;
    for (x, y) in a.configs.iter().zip(&b.configs) {
        let (Status::Ran(x), Status::Ran(y)) = (&x.status, &y.status) else {
            panic!()
        };
        assert_eq!(x.scores, y.scores);
        assert_eq!(x.overall, y.overall);
    }
}

#[tokio::test]
async fn missing_qmd_skips_those_columns_but_lcg_still_runs() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let d = tempfile::tempdir().unwrap();
    let missing: Arc<dyn QmdRunner> = Arc::new(ProcessRunner {
        bin: PathBuf::from("/definitely/not/qmd"),
    });
    let r = full_run(&fx, Some(missing), &d.path().join("j.jsonl")).await;
    assert!(matches!(r.configs[0].status, Status::Ran(_)));
    assert!(matches!(r.configs[1].status, Status::Ran(_)));
    for c in &r.configs[2..] {
        let Status::Skipped(why) = &c.status else {
            panic!("qmd must be skipped, not scored")
        };
        assert!(why.contains("could not be started"), "{why}");
    }
    let md = r.render_markdown();
    assert!(md.contains("**skipped**"), "{md}");
    assert!(
        md.contains("cannot be read"),
        "hypotheses cannot be read without qmd"
    );
}

#[tokio::test]
async fn grounded_measure_is_skipped_not_zero_without_a_judge() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let transport: Arc<dyn RpcTransport> = Arc::new(FakeLcg::new());
    lcg::ingest(transport.as_ref(), &fx).await.unwrap();
    let slots = vec![Slot::Run(Box::new(LcgPassages {
        t: transport,
        mode: lcg::PassageMode::Vector,
    }))];
    let out = evaluate(&fx, K, slots, None, "ANTHROPIC_API_KEY not set").await;
    let Status::Ran(run) = &out[0].status else {
        panic!()
    };
    assert_eq!(run.overall.grounded_recall, None);
}

#[tokio::test]
async fn qmd_reranker_toggle_is_a_separate_configuration() {
    let runner = Arc::new(FakeQmd {
        notes: vec![],
        log: Mutex::new(vec![]),
    });
    let mk = |rerank, id| QmdRetriever {
        runner: runner.clone(),
        index_name: "idx".into(),
        rerank,
        id,
    };
    use lcg_eval::retrieval::Retriever;
    mk(false, "qmd")
        .retrieve("anything at all", K)
        .await
        .unwrap();
    mk(true, "qmd-rerank")
        .retrieve("anything at all", K)
        .await
        .unwrap();
    let log = runner.log.lock().unwrap();
    assert!(log[0].contains(&"--no-rerank".to_string()));
    assert!(!log[1].contains(&"--no-rerank".to_string()));
}

// ── CLI ──────────────────────────────────────────────────────────────────────────

#[test]
fn retrieval_dry_run_prints_the_resolved_plan_without_side_effects() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lcg-eval"))
        .args(["retrieval", "--dry-run", "--corpus"])
        .arg(fixture_dir())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = String::from_utf8_lossy(&out.stdout);
    for needle in [
        "fernhill-notebook-1",
        "fernhill-questions-1",
        "k:          10",
        "lcg-passages",
        "qmd-rerank",
        "temporal",
    ] {
        assert!(s.contains(needle), "missing {needle:?} in:\n{s}");
    }
}

#[test]
fn retrieval_run_refuses_without_a_real_embedder() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lcg-eval"))
        .args(["retrieval", "--configs", "lcg-passages", "--corpus"])
        .arg(fixture_dir())
        .env_remove("LCG_EMBEDDING_URL")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no real embedder"));
}

#[test]
fn extraction_cli_is_unchanged() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lcg-eval"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("--backend"));
    // And the extraction parser still demands a backend rather than treating `retrieval` specially.
    let owned = vec!["--limit".to_string(), "3".to_string()];
    assert!(lcg_eval::cli::parse_args(&owned).is_err());
}

// ── v1 is frozen (#788 FR-026 / SC-009) ──────────────────────────────────────────

#[test]
fn v1_fixture_hashes_and_shape_are_unchanged() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    assert_eq!(fx.corpus_hash, "26b9c77bff672642");
    assert_eq!(fx.questions_hash, "9a7c6129bf1afbd8");
    assert_eq!(fx.manifest.notes.len(), 31);
    assert!(
        fx.manifest
            .notes
            .iter()
            .all(|n| n.origin.is_none() && n.cites.is_empty()),
        "v1 declares no origins"
    );
}
