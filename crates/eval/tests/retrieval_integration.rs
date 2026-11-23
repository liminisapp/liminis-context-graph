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
            mode: lcg::PassageMode::Default,
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
        inputs: None,
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
        mode: lcg::PassageMode::Default,
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

// ── #872: inputs digests, --compare, --record-ranked ─────────────────────────────

/// Runs `lcg-eval retrieval` over the checked-in fixture with qmd absent, so every config is a
/// skipped row and nothing needs a service, a model or a network. The report still carries the
/// `inputs` block, which is all the comparison tests need.
fn cli_run(extra: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_lcg-eval"))
        .args([
            "retrieval",
            "--configs",
            "qmd",
            "--qmd-bin",
            "/nonexistent/qmd",
            "--no-grounded",
            "--corpus",
        ])
        .arg(fixture_dir())
        .args(extra)
        .output()
        .unwrap()
}

fn code(o: &std::process::Output) -> i32 {
    o.status.code().unwrap()
}

fn stderr(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A first run's `report.json`, edited by `edit`, written to `dir/prev.json`.
fn previous_report(dir: &Path, edit: impl FnOnce(&mut Value)) -> PathBuf {
    let first = dir.join("first");
    let o = cli_run(&["--out", first.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let mut j: Value =
        serde_json::from_str(&std::fs::read_to_string(first.join("report.json")).unwrap()).unwrap();
    edit(&mut j);
    let p = dir.join("prev.json");
    std::fs::write(&p, serde_json::to_string(&j).unwrap()).unwrap();
    p
}

#[test]
fn report_json_gains_an_inputs_block_with_full_digests_and_none_for_unused_inputs() {
    let d = tempfile::tempdir().unwrap();
    let prev = previous_report(d.path(), |_| {});
    let j: Value = serde_json::from_str(&std::fs::read_to_string(prev).unwrap()).unwrap();
    let fx = fixture::load(&fixture_dir()).unwrap();
    assert_eq!(j["inputs"]["schema_version"], 1);
    assert_eq!(j["inputs"]["corpus"], fx.corpus_digest.as_str());
    assert_eq!(j["inputs"]["questions"], fx.questions_digest.as_str());
    assert_eq!(j["inputs"]["corpus"].as_str().unwrap().len(), 64);
    // A run with no cassette, variants file or LCG service has nothing to digest.
    for k in [
        "variants",
        "cassette",
        "embedder_model",
        "service_binary",
        "service_env",
    ] {
        assert_eq!(j["inputs"][k], "none", "{k}");
    }
    // The pre-existing meta hashes are the first 16 hex of the same digests.
    assert_eq!(j["meta"]["corpus_hash"], fx.corpus_hash.as_str());
    assert!(fx.corpus_digest.starts_with(&fx.corpus_hash));
}

#[test]
fn compare_against_an_identical_run_exits_0_and_writes_compare_md() {
    let d = tempfile::tempdir().unwrap();
    let prev = previous_report(d.path(), |_| {});
    let out = d.path().join("second");
    let o = cli_run(&[
        "--compare",
        prev.to_str().unwrap(),
        "--strict",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(out.join("report.json").exists());
    let md = std::fs::read_to_string(out.join("compare.md")).unwrap();
    assert!(md.contains("# Retrieval comparison"), "{md}");
    assert!(!md.contains("Soft inputs differ"));
}

#[test]
fn a_hard_input_mismatch_exits_2_after_writing_the_report_and_prints_no_deltas() {
    let d = tempfile::tempdir().unwrap();
    let prev = previous_report(d.path(), |j| {
        j["inputs"]["questions"] = json!("0".repeat(64));
        j["meta"]["k"] = json!(7);
    });
    let out = d.path().join("second");
    let o = cli_run(&[
        "--compare",
        prev.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2);
    let err = stderr(&o);
    assert!(
        err.contains("questions") && err.contains("k (earlier 7"),
        "{err}"
    );
    // This run's own report survives the refusal; no comparison is produced.
    assert!(out.join("report.json").exists());
    assert!(!out.join("compare.md").exists());
    assert!(!String::from_utf8_lossy(&o.stdout).contains("Retrieval comparison"));
}

#[test]
fn an_earlier_report_without_inputs_exits_2_before_anything_runs() {
    let d = tempfile::tempdir().unwrap();
    let prev = previous_report(d.path(), |j| {
        j.as_object_mut().unwrap().remove("inputs");
    });
    let out = d.path().join("second");
    let o = cli_run(&[
        "--compare",
        prev.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("re-run"), "{}", stderr(&o));
    assert!(
        !out.exists(),
        "an inputs-less report is refused before any spend"
    );
}

#[test]
fn an_unreadable_or_invalid_compare_file_exits_1() {
    let d = tempfile::tempdir().unwrap();
    let o = cli_run(&["--compare", d.path().join("missing.json").to_str().unwrap()]);
    assert_eq!(code(&o), 1);
    assert!(stderr(&o).contains("cannot read --compare file"));
    let bad = d.path().join("bad.json");
    std::fs::write(&bad, "{\"not\": \"a report\"}").unwrap();
    let o = cli_run(&["--compare", bad.to_str().unwrap()]);
    assert_eq!(code(&o), 1);
    assert!(stderr(&o).contains("not a valid report"));
}

#[test]
fn a_soft_difference_prints_a_banner_and_strict_refuses_it() {
    let d = tempfile::tempdir().unwrap();
    let prev = previous_report(d.path(), |j| {
        j["inputs"]["cassette"] = json!("c".repeat(64));
    });
    let out = d.path().join("lax");
    let o = cli_run(&[
        "--compare",
        prev.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let md = std::fs::read_to_string(out.join("compare.md")).unwrap();
    assert!(
        md.contains("Soft inputs differ") && md.contains("`cassette`"),
        "{md}"
    );

    let strict = d.path().join("strict");
    let o = cli_run(&[
        "--compare",
        prev.to_str().unwrap(),
        "--strict",
        "--out",
        strict.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("cassette"));
    assert!(strict.join("report.json").exists());
    assert!(!strict.join("compare.md").exists());
}

#[test]
fn strict_without_compare_and_ranked_misuse_are_usage_errors() {
    assert_eq!(code(&cli_run(&["--strict"])), 2);
    assert_eq!(code(&cli_run(&["--record-ranked"])), 2);
}

#[test]
fn private_record_ranked_is_refused_and_leaves_no_ranked_file() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("priv");
    let o = cli_run(&[
        "--private",
        "--record-ranked",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert!(!out.join("ranked.jsonl").exists());
    // A private run may still compare, and still records digests.
    let prev = previous_report(d.path(), |_| {});
    let o = cli_run(&[
        "--private",
        "--compare",
        prev.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let j: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("report.json")).unwrap()).unwrap();
    assert_eq!(j["inputs"]["corpus"].as_str().unwrap().len(), 64);
    assert!(!out.join("ranked.jsonl").exists());
}

#[tokio::test]
async fn ranked_jsonl_joins_dense_and_hybrid_configs_on_question_id_with_no_missing_ids() {
    let fx = fixture::load(&fixture_dir()).unwrap();
    let transport: Arc<dyn RpcTransport> = Arc::new(FakeLcg::new());
    lcg::ingest(transport.as_ref(), &fx).await.unwrap();
    let slots = [lcg::PassageMode::Raw, lcg::PassageMode::Hybrid]
        .into_iter()
        .map(|mode| {
            Slot::Run(Box::new(LcgPassages {
                t: transport.clone(),
                mode,
            }))
        })
        .collect();
    let outcomes = evaluate(&fx, K, slots, None, "no judge").await;
    let d = tempfile::tempdir().unwrap();
    let path = lcg_eval::retrieval::ranked::write_ranked(d.path(), &outcomes).unwrap();
    assert_eq!(path, d.path().join("ranked.jsonl"));

    let mut by_config: HashMap<String, BTreeSet<String>> = HashMap::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        by_config
            .entry(v["config"].as_str().unwrap().to_string())
            .or_default()
            .insert(v["question"].as_str().unwrap().to_string());
        for h in v["hits"].as_array().unwrap() {
            assert!(h["rank"].as_u64().unwrap() >= 1);
            // The fake service supplies no legs; the field is absent, not null.
            assert!(h.get("legs").is_none());
        }
    }
    let raw = &by_config["lcg-passages-raw"];
    let hybrid = &by_config["lcg-passages-hybrid"];
    let all: BTreeSet<String> = fx.questions.iter().map(|q| q.id.clone()).collect();
    assert_eq!(raw, &all, "every question has a raw record");
    assert_eq!(
        raw, hybrid,
        "the two configs join on question id with none missing"
    );
}

#[test]
fn a_reused_out_dir_does_not_keep_a_stale_compare_md_or_ranked_jsonl() {
    let d = tempfile::tempdir().unwrap();
    let ok = previous_report(d.path(), |_| {});
    let out = d.path().join("reused");
    // First run: a compatible compare plus ranked output leaves both files behind.
    let o = cli_run(&[
        "--compare",
        ok.to_str().unwrap(),
        "--record-ranked",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(out.join("compare.md").exists() && out.join("ranked.jsonl").exists());
    // Second run in the same directory: refused comparison, no --record-ranked.
    // `previous_report` always writes `<dir>/prev.json`, so give the second one its own directory.
    let other = d.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let bad = previous_report(&other, |j| {
        j["inputs"]["questions"] = json!("0".repeat(64));
    });
    let o = cli_run(&[
        "--compare",
        bad.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2);
    assert!(out.join("report.json").exists());
    assert!(!out.join("compare.md").exists(), "stale compare.md cleared");
    assert!(
        !out.join("ranked.jsonl").exists(),
        "stale ranked.jsonl cleared"
    );
}

#[test]
fn ranked_jsonl_is_not_written_without_the_flag() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("o");
    let o = cli_run(&["--out", out.to_str().unwrap()]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(out.join("report.json").exists());
    assert!(!out.join("ranked.jsonl").exists());
    let out2 = d.path().join("o2");
    let o = cli_run(&["--out", out2.to_str().unwrap(), "--record-ranked"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(out2.join("ranked.jsonl").exists());
}
