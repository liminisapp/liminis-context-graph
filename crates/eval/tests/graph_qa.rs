//! `lcg-eval graph-qa` (#821): the recorded E13 plans (SC-008), the fixture ontology, and an
//! end-to-end smoke run over canned service responses with a scripted planner and reader — no
//! service, no network (SC-005, SC-006).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use serde_json::{json, Value};

use lcg_core::ontology::load_ontology_from_path;
use lcg_core::OntologyMode;
use lcg_eval::graph_qa::cache::LlmCache;
use lcg_eval::graph_qa::mock::ScriptedModel;
use lcg_eval::graph_qa::plans::{self, PlansFile};
use lcg_eval::graph_qa::report::{Meta, Report};
use lcg_eval::graph_qa::runner::{self, Inputs, QmdSetup, RunError, RunOutput};
use lcg_eval::graph_qa::{load_questions, prompts};
use lcg_eval::llm_client::ModelRequest;
use lcg_eval::retrieval::generator::graph_questions::{self, GraphQuestion};
use lcg_eval::retrieval::lcg::RpcTransport;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn v2() -> PathBuf {
    manifest_dir().join("fixtures/retrieval-v2")
}
fn smoke() -> PathBuf {
    manifest_dir().join("tests/data/graph-qa-smoke")
}

// ── the fixture ontology and the recorded E13 plans (SC-008) ─────────────────────

#[test]
fn ontology_v3_1_loads_unchanged_under_the_current_loader() {
    let o =
        load_ontology_from_path(&v2().join("ontology-v3.1.yaml"), OntologyMode::Strict).unwrap();
    assert_eq!(o.entity_types.len(), 12);
    assert_eq!(o.relation_types.len(), 27);
}

#[test]
fn recorded_v5_plans_become_valid_walk_requests_without_a_planner() {
    let onto =
        load_ontology_from_path(&v2().join("ontology-v3.1.yaml"), OntologyMode::Strict).unwrap();
    let rels: BTreeSet<String> = onto.relation_types.iter().map(|r| r.name.clone()).collect();
    let ents: BTreeSet<String> = onto.entity_types.iter().map(|e| e.name.clone()).collect();
    let f: PlansFile = plans::load_plans(&v2().join("e13-v5-plans.json")).unwrap();
    assert_eq!(f.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    let qs = load_questions(&v2().join(graph_questions::FILE)).unwrap();
    let e13: Vec<&GraphQuestion> = qs.iter().filter(|q| q.set == "e13").collect();
    assert_eq!(f.plans.len(), 129);
    let ids: BTreeSet<&str> = qs.iter().map(|q| q.id.as_str()).collect();
    assert!(f.unknown_ids(&ids).is_empty());
    let (mut graph, mut passages) = (0, 0);
    for q in &e13 {
        let p = plans::validate(&f.plans[&q.id], &rels).unwrap_or_else(|e| panic!("{}: {e}", q.id));
        match p.route.as_str() {
            "graph" => {
                graph += 1;
                let req = plans::walk_request(&p, &q.text);
                lcg_core::walk::parse_request(&req).unwrap_or_else(|e| panic!("{}: {e}", q.id));
                assert_eq!(req["direction_mode"], "soft");
                for h in &p.hops {
                    assert!(h.types.iter().all(|t| rels.contains(t)), "{}", q.id);
                }
                let at = p.answer_type.clone().unwrap_or_default();
                assert!(at == "value" || ents.contains(&at), "{}: {at}", q.id);
            }
            "passages" => {
                passages += 1;
                assert_eq!(q.shape, "history", "{}", q.id);
            }
            r => panic!("{}: route {r}", q.id),
        }
    }
    assert_eq!((graph, passages), (118, 11));
    // The recorded reference: route v5 means 0.915 over E13 and 0.940 on its test half.
    let s = f.route_planned5_scores.as_ref().unwrap();
    let mean = |keep: &dyn Fn(&GraphQuestion) -> bool| {
        let xs: Vec<f64> = e13.iter().filter(|q| keep(q)).map(|q| s[&q.id]).collect();
        xs.iter().sum::<f64>() / xs.len() as f64
    };
    assert!((mean(&|_| true) - 0.915).abs() < 0.001);
    assert!((mean(&|q| q.split == "test") - 0.940).abs() < 0.001);
}

// ── smoke run over canned responses ──────────────────────────────────────────────

/// Canned service: an ontology, passages that mention the query, and walks whose facts carry the
/// marker `GOOD` unless the start is `Nobody`.
struct FakeLcg {
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakeLcg {
    fn new() -> Self {
        Self {
            calls: Mutex::default(),
        }
    }
    fn count(&self, method: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .count()
    }
}

impl RpcTransport for FakeLcg {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, String>> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
        let r = match method {
            "knowledge_get_ontology" => Ok(ontology_view()),
            "knowledge_search_passages" => {
                let q = params["query"].as_str().unwrap_or("");
                // History questions are answerable from passages; the rest are not.
                let content = if q.contains("in order") {
                    format!("GOOD passage about {q}")
                } else {
                    format!("passage about {q}")
                };
                let mut r = json!({"passages": [{"source_description": "note-a:0",
                    "valid_at": "2025-01-02T00:00:00Z", "content": content}]});
                if params["rerank"] == json!(true) {
                    r["rerank"] = json!({"applied": true});
                }
                Ok(r)
            }
            "knowledge_walk" => (|| {
                lcg_core::walk::parse_request(&params).map_err(|e| e.to_string())?;
                let start = params["start"].as_str().unwrap_or("");
                Ok(if start == "Nobody" {
                    json!({"found": false, "facts_text": ""})
                } else {
                    json!({"found": true, "facts_text": format!("- GOOD fact from {start} (A [Person] -> B [Project])")})
                })
            })(),
            m => Err(format!("unexpected method {m}")),
        };
        Box::pin(async move { r })
    }
}

fn ontology_view() -> Value {
    json!({"has_ontology": true, "mode": "strict",
        "entity_types": [{"name": "Person"}, {"name": "Project"}],
        "relation_types": [{"name": "OWNS", "description": "A person owns a project."},
                           {"name": "REPORTS_TO", "description": "A person reports to another."},
                           {"name": "HOSTED_BY", "description": "A project is hosted by a provider."}]})
}

fn questions() -> Vec<GraphQuestion> {
    load_questions(&smoke().join("graph-questions.jsonl")).unwrap()
}

/// The reader answers with the gold when the context carries `GOOD`, else with nothing.
fn reader() -> ScriptedModel {
    let gold: BTreeMap<String, Vec<String>> = questions()
        .into_iter()
        .map(|q| (q.text, q.answers))
        .collect();
    ScriptedModel::new(move |r: &ModelRequest| {
        let q = r.user.rsplit("Question: ").next().unwrap_or("").to_string();
        let answers = if r.user.contains("GOOD") {
            gold.get(&q).cloned().unwrap_or_default()
        } else {
            vec![]
        };
        Ok(json!({ "answers": answers }).to_string())
    })
}

fn planner() -> ScriptedModel {
    ScriptedModel::new(|r: &ModelRequest| {
        assert!(r
            .system
            .as_deref()
            .unwrap()
            .contains("- HOSTED_BY: A project is hosted"));
        assert!(
            !r.user.contains("reports-to-single"),
            "the planner never sees the family"
        );
        Ok(json!({"route": "graph", "start": "Ana Silva", "hops": [
            {"types": ["REPORTS_TO"], "phrase": "people reporting", "direction": "in"}],
            "list": false, "history": false, "answer_type": "Person"})
        .to_string())
    })
}

const ALL: [&str; 5] = ["passages", "passages-belief-rerank", "qmd", "walk", "route"];

fn runners(r: &[&str]) -> Vec<String> {
    r.iter().map(|s| s.to_string()).collect()
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    qs: &[GraphQuestion],
    plans_file: &PlansFile,
    plans_only: bool,
    run: &[String],
    planner: &ScriptedModel,
    reader: &ScriptedModel,
    cache: &LlmCache,
    t: Arc<FakeLcg>,
) -> Result<RunOutput, RunError> {
    let view = ontology_view();
    runner::run(Inputs {
        questions: qs,
        runners: run,
        transport: t,
        ontology_view: &view,
        plans_file: Some(plans_file),
        plans_only,
        planner: Some(planner),
        reader,
        planner_model: "planner-m",
        reader_model: "reader-m",
        cache,
        qmd: run
            .iter()
            .any(|r| r == "qmd")
            .then(|| QmdSetup::Skipped("qmd is not installed".into())),
        k: 10,
        concurrency: 3,
    })
    .await
}

fn report_json(
    out: &RunOutput,
    qs: &[GraphQuestion],
    f: &PlansFile,
    cache: &LlmCache,
) -> (Value, String) {
    let r = Report {
        meta: Meta {
            workspace: "ws".into(),
            workspace_kind: "ontology".into(),
            seed: 788,
            resamples: 500,
            k: 10,
            cache: cache.counts(),
            ..Default::default()
        },
        run: out,
        questions: qs,
        reference: f.route_planned5_scores.as_ref(),
    };
    (r.to_json(), r.render_markdown())
}

#[tokio::test]
async fn smoke_every_runner_is_scored_and_the_report_has_every_section() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = LlmCache::load(&tmp.path().join("c.jsonl")).unwrap();
    let qs = questions();
    let f = plans::load_plans(&smoke().join("plans.json")).unwrap();
    let (p, r) = (planner(), reader());
    let t = Arc::new(FakeLcg::new());
    let out = run_once(&qs, &f, false, &runners(&ALL), &p, &r, &cache, t.clone())
        .await
        .unwrap();
    // qmd is skipped with a notice; every other runner scored every question.
    assert_eq!(
        out.runners,
        vec!["passages", "passages-belief-rerank", "walk", "route"]
    );
    assert_eq!(out.skipped["qmd"], "qmd is not installed");
    for qr in &out.questions {
        assert_eq!(qr.per_runner.len(), 4, "{}", qr.id);
    }
    // Only the question the plans file does not cover is planned.
    assert_eq!(p.calls(), 1);
    assert!(p.log.lock().unwrap()[0]
        .user
        .ends_with("Who reports to Ana Silva?"));
    let by: BTreeMap<&str, _> = out.questions.iter().map(|q| (q.id.as_str(), q)).collect();
    assert_eq!(by["s006"].plan_source, "planner");
    assert_eq!(by["s001"].plan_source, "file");
    // Unknown relation type: a plan error, answered from an empty walk and scored 0.
    assert!(by["s004"]
        .plan_error
        .as_deref()
        .unwrap()
        .contains("MADE_UP"));
    assert_eq!(by["s004"].per_runner["walk"].score, 0.0);
    // Unresolved start: counted, scored 0.
    assert_eq!(by["s002"].start_resolved, Some(false));
    assert_eq!(by["s002"].per_runner["walk"].score, 0.0);
    // Route follows the plan: s005 routed to passages (answerable there), the rest walked.
    assert_eq!(by["s005"].route.as_deref(), Some("passages"));
    assert_eq!(by["s005"].per_runner["route"].score, 1.0);
    assert_eq!(by["s005"].per_runner["walk"].score, 1.0);
    assert_eq!(by["s001"].per_runner["route"].score, 1.0);
    assert_eq!(by["s001"].per_runner["passages"].score, 0.0);
    assert_eq!(by["s003"].per_runner["walk"].score, 1.0);
    assert_eq!(by["s006"].per_runner["walk"].score, 1.0);
    // The walk ran soft-direction walks only for valid plans that needed one.
    assert!(t.count("knowledge_walk") >= 4);

    let (j, md) = report_json(&out, &qs, &f, &cache);
    let s = &j["scores"];
    let route = &s["means"]["route"];
    for k in [
        "e13",
        "e13/dev",
        "e13/test",
        "all",
        "all/dev",
        "all/test",
        "shape:chain-2",
        "shape:inverse",
        "shape:history/dev",
        "family:owned-by",
    ] {
        assert!(route.get(k).is_some(), "missing slice {k}");
    }
    // E13 excludes the inverse question.
    assert_eq!(route["e13"]["n"], 5);
    assert_eq!(route["all"]["n"], 6);
    // route: s001, s003, s005 right (3/5) on E13; s006 too on all (4/6).
    assert!((route["e13"]["mean"].as_f64().unwrap() - 0.6).abs() < 1e-12);
    assert!((route["all"]["mean"].as_f64().unwrap() - 4.0 / 6.0).abs() < 1e-12);
    let boot = s["bootstrap"].as_array().unwrap();
    // 4 runners → 6 pairs, each over the same slices.
    assert!(boot
        .iter()
        .any(|d| d["a"] == "passages" && d["b"] == "route" && d["slice"] == "e13/test"));
    assert_eq!(boot.len() % 6, 0);
    let d = &s["diagnostics"];
    assert_eq!(d["plan_sources"], json!({"file": 5, "planner": 1}));
    assert_eq!(d["plan_errors_by_shape"], json!({"join": 1}));
    assert_eq!(d["unresolved_starts_by_shape"], json!({"chain-3": 1}));
    assert_eq!(d["plans_file_unknown_ids"], json!(["zzz"]));
    assert_eq!(d["reference"]["e13"]["n"], 5);
    assert_eq!(s["per_question"].as_array().unwrap().len(), 6);
    assert_eq!(
        j["meta"]["reader_prompt_version"],
        prompts::READER_PROMPT_VERSION
    );
    assert_eq!(
        j["meta"]["planner_prompt_version"],
        prompts::PLANNER_PROMPT_VERSION
    );
    for section in [
        "## Overall",
        "## By shape",
        "**dev half**",
        "**test half**",
        "## By family",
        "## Paired differences",
        "## Diagnostics",
        "## Per question",
        "`qmd`: **skipped**",
        "zzz",
    ] {
        assert!(md.contains(section), "markdown lacks {section}");
    }
}

#[tokio::test]
async fn a_fully_cached_second_run_makes_no_model_call_and_identical_scores() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("c.jsonl");
    let qs = questions();
    let f = plans::load_plans(&smoke().join("plans.json")).unwrap();
    let run = runners(&["passages", "walk", "route"]);
    let first = {
        let cache = LlmCache::load(&path).unwrap();
        let (p, r) = (planner(), reader());
        let out = run_once(
            &qs,
            &f,
            false,
            &run,
            &p,
            &r,
            &cache,
            Arc::new(FakeLcg::new()),
        )
        .await
        .unwrap();
        // Identical contexts are read once: route repeats walk or passages contexts.
        assert!(r.calls() < qs.len() * 3, "{}", r.calls());
        report_json(&out, &qs, &f, &cache).0
    };
    let cache = LlmCache::load(&path).unwrap();
    let (p, r) = (planner(), reader());
    let out = run_once(
        &qs,
        &f,
        false,
        &run,
        &p,
        &r,
        &cache,
        Arc::new(FakeLcg::new()),
    )
    .await
    .unwrap();
    assert_eq!((p.calls(), r.calls()), (0, 0));
    let second = report_json(&out, &qs, &f, &cache).0;
    assert_eq!(
        serde_json::to_string(&first["scores"]).unwrap(),
        serde_json::to_string(&second["scores"]).unwrap()
    );
    assert_eq!(second["meta"]["cache"]["reader"]["paid_calls"], 0);
}

#[tokio::test]
async fn plans_only_fails_before_any_reader_call_naming_the_uncovered_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = LlmCache::load(&tmp.path().join("c.jsonl")).unwrap();
    let qs = questions();
    let f = plans::load_plans(&smoke().join("plans.json")).unwrap();
    let (p, r) = (planner(), reader());
    let t = Arc::new(FakeLcg::new());
    let e = run_once(
        &qs,
        &f,
        true,
        &runners(&["walk"]),
        &p,
        &r,
        &cache,
        t.clone(),
    )
    .await
    .unwrap_err();
    match e {
        RunError::Uncovered(ids) => assert_eq!(ids, vec!["s006"]),
        e => panic!("{e}"),
    }
    assert_eq!((p.calls(), r.calls()), (0, 0));
    assert_eq!(t.count("knowledge_walk"), 0);
    // Runners that need no plan do not need the plans file to cover anything.
    let out = run_once(&qs, &f, true, &runners(&["passages"]), &p, &r, &cache, t)
        .await
        .unwrap();
    assert!(out.questions.iter().all(|q| q.plan_source == "none"));
}

#[tokio::test]
async fn a_provider_failure_aborts_and_keeps_what_was_cached() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("c.jsonl");
    let cache = LlmCache::load(&path).unwrap();
    let qs = questions();
    let f = plans::load_plans(&smoke().join("plans.json")).unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    let r = ScriptedModel::new(move |_| {
        if n2.fetch_add(1, Ordering::SeqCst) >= 2 {
            Err("overloaded".into())
        } else {
            Ok("{\"answers\": []}".into())
        }
    });
    let p = planner();
    let view = ontology_view();
    let run = runners(&["passages"]);
    let e = runner::run(Inputs {
        questions: &qs,
        runners: &run,
        transport: Arc::new(FakeLcg::new()),
        ontology_view: &view,
        plans_file: Some(&f),
        plans_only: false,
        planner: Some(&p),
        reader: &r,
        planner_model: "p",
        reader_model: "r",
        cache: &cache,
        qmd: None,
        k: 10,
        concurrency: 1,
    })
    .await
    .unwrap_err();
    assert!(e.to_string().contains("overloaded"), "{e}");
    assert_eq!(LlmCache::load(&path).unwrap().len(), 2);
}

#[test]
fn paired_bootstrap_is_deterministic_and_exact_on_a_constant_difference() {
    let qs = questions();
    let mk = |a: f64, b: f64| -> RunOutput {
        RunOutput {
            runners: vec!["a".into(), "b".into()],
            questions: qs
                .iter()
                .map(|q| runner::QuestionRun {
                    id: q.id.clone(),
                    plan: None,
                    plan_source: "none",
                    plan_error: None,
                    route: None,
                    start_resolved: None,
                    per_runner: [("a", a), ("b", b)]
                        .into_iter()
                        .map(|(k, s)| {
                            (
                                k.to_string(),
                                runner::RunnerAnswer {
                                    answers: vec![],
                                    score: s,
                                    reader_parse_error: false,
                                },
                            )
                        })
                        .collect(),
                })
                .collect(),
            ..Default::default()
        }
    };
    let out = mk(1.0, 0.0);
    let rep = |o: &RunOutput| {
        Report {
            meta: Meta {
                seed: 788,
                resamples: 1000,
                ..Default::default()
            },
            run: o,
            questions: &qs,
            reference: None,
        }
        .scores_json()
    };
    let s1 = rep(&out);
    assert_eq!(s1, rep(&out), "same seed, same intervals");
    let all = s1["bootstrap"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["slice"] == "all")
        .unwrap()
        .clone();
    assert_eq!(all["n"], 6);
    assert_eq!(all["mean_diff"], 1.0);
    assert_eq!(
        (all["lo"].as_f64(), all["hi"].as_f64()),
        (Some(1.0), Some(1.0))
    );
    // A single runner still reports per-half means, with no pairs.
    let one = RunOutput {
        runners: vec!["a".into()],
        ..mk(0.5, 0.5)
    };
    let s = rep(&one);
    assert!(s["bootstrap"].as_array().unwrap().is_empty());
    assert_eq!(s["means"]["a"]["all/dev"]["mean"], 0.5);
}

// ── CLI ─────────────────────────────────────────────────────────────────────────

fn cli(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_lcg-eval"))
        .arg("graph-qa")
        .args(args)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL")
        .output()
        .unwrap()
}

#[test]
fn cli_exit_codes_for_usage_and_unusable_workspaces() {
    assert_eq!(cli(&[]).status.code(), Some(2));
    assert_eq!(
        cli(&["--workspace", "w", "--runners", "nope"])
            .status
            .code(),
        Some(2)
    );
    assert!(cli(&["--help"]).status.success());
    let out = cli(&[
        "--workspace",
        "/nonexistent/ws",
        "--embedder-http",
        "http://127.0.0.1:1",
    ]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A directory that is not an ingested workspace.
    let empty = tempfile::tempdir().unwrap();
    let out = cli(&[
        "--workspace",
        empty.path().to_str().unwrap(),
        "--embedder-http",
        "http://127.0.0.1:1",
    ]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an ingested workspace"));
    // An embedder other than the one the workspace was built with.
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join(".lcg/db")).unwrap();
    std::fs::write(ws.path().join(".lcg/db/liminis.db"), "").unwrap();
    std::fs::write(
        ws.path().join("lcg-eval-workspace.json"),
        json!({"format": 1, "group_id": "retrieval-eval", "corpus_version": "x", "corpus_hash": "h",
               "ontology_hash": "none", "ontology_mode": null, "embedder": "http://ingest-embedder",
               "embedding_profile": null, "extractor": "e", "cassette": null, "lcg_build": "b"})
        .to_string(),
    )
    .unwrap();
    let out = cli(&[
        "--workspace",
        ws.path().to_str().unwrap(),
        "--embedder-http",
        "http://other",
    ]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("http://ingest-embedder"));
    // The right embedder but a different embedding profile than the ingest's.
    std::fs::write(
        ws.path().join("lcg-eval-workspace.json"),
        json!({"format": 1, "group_id": "retrieval-eval", "corpus_version": "x", "corpus_hash": "h",
               "ontology_hash": "none", "ontology_mode": null, "embedder": "http://ingest-embedder",
               "embedding_profile": "bge-v1.5", "extractor": "e", "cassette": null, "lcg_build": "b"})
        .to_string(),
    )
    .unwrap();
    let out = cli(&[
        "--workspace",
        ws.path().to_str().unwrap(),
        "--embedder-http",
        "http://ingest-embedder",
        "--embedding-profile",
        "none",
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(
        stderr.contains("--embedding-profile bge-v1.5"),
        "the refusal names the ingest profile: {stderr}"
    );
}

#[test]
fn the_smoke_questions_cover_every_shape_and_scoring_mode() {
    let qs = questions();
    let shapes: BTreeSet<&str> = qs.iter().map(|q| q.shape.as_str()).collect();
    assert_eq!(shapes.len(), 6);
    let modes: BTreeSet<&str> = qs.iter().map(|q| q.scoring.as_str()).collect();
    assert_eq!(modes, BTreeSet::from(["ordered", "set", "single"]));
    assert!(Path::new(&smoke().join("plans.json")).exists());
}
