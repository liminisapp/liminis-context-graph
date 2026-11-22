//! Real-service round trip for #821 (SC-002, SC-003, SC-007, US3-AS8): record a v1 ingest under an
//! ontology with `--keep-workspace`, replay it with zero misses and identical metrics, refuse a
//! replay under a different ontology before anything starts, run `graph-integrity` on the kept
//! workspace, and run graph-qa on it without changing a byte of it.
//!
//! Drives the shipped `liminis-context-graph` binary with loopback stub embedder and extractor, so
//! it needs no network and no model. The service binary is found beside `lcg-eval` in the target
//! directory, which a workspace-wide `cargo test` builds; under `cargo test -p lcg-eval` alone it
//! may be missing, and the test then fails with instructions (set `LCG_EVAL_SKIP_SERVICE_TESTS=1`
//! to skip it deliberately).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use lcg_eval::graph_qa::cli::{parse_args, Mode};
use lcg_eval::graph_qa::mock::ScriptedModel;
use lcg_eval::graph_qa::run_on_workspace;

const ONTOLOGY: &str = "mode: strict
entity_types:
  - name: Person
    description: A human individual.
  - name: Project
    description: An initiative with an owner.
relation_types:
  - name: OWNS
    description: A person owns a project.
    source_type: Person
    target_type: Project
  - name: REPORTS_TO
    description: A person reports to another person.
  - name: HOSTED_BY
    description: A project is hosted by a provider.
";

fn service_bin() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_BIN_EXE_lcg-eval")).with_file_name("liminis-context-graph");
    if p.exists() {
        return Some(p);
    }
    if std::env::var("LCG_EVAL_SKIP_SERVICE_TESTS").as_deref() == Ok("1") {
        eprintln!(
            "SKIPPED: {} not built (LCG_EVAL_SKIP_SERVICE_TESTS=1)",
            p.display()
        );
        return None;
    }
    panic!(
        "{} is missing: this test drives the real service. Build it first (`cargo build -p \
         lcg-service`, or run the workspace-wide `cargo test`), or set \
         LCG_EVAL_SKIP_SERVICE_TESTS=1 to skip it deliberately.",
        p.display()
    );
}

/// Reads one HTTP/1.1 request body (Content-Length framed).
fn read_body(s: &mut std::net::TcpStream) -> Vec<u8> {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; len];
    let _ = r.read_exact(&mut body);
    body
}

fn respond(s: &mut std::net::TcpStream, body: &str) {
    let _ = write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A deterministic per-text embedder (OpenAI-compatible `/v1/embeddings`).
fn spawn_embedder() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let req: Value = serde_json::from_slice(&read_body(&mut s)).unwrap_or(Value::Null);
            let texts: Vec<String> = match &req["input"] {
                Value::String(t) => vec![t.clone()],
                Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                _ => vec![],
            };
            let data: Vec<Value> = texts
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let h = Sha256::digest(t.as_bytes());
                    let v: Vec<f32> = (0..64)
                        .map(|j| f32::from(h[j % 32] ^ (j as u8)) / 255.0 - 0.5)
                        .collect();
                    json!({"object": "embedding", "embedding": v, "index": i})
                })
                .collect();
            respond(
                &mut s,
                &json!({"object": "list", "data": data, "model": "stub"}).to_string(),
            );
        }
    });
    format!("http://127.0.0.1:{port}/v1/embeddings")
}

/// An OpenAI-compatible extractor whose output is distinct per chunk (cassette classify keys hash
/// the extracted content, so identical extractions would collide). Counts requests.
fn spawn_extractor() -> (String, Arc<AtomicUsize>) {
    let (url, n, _) = spawn_extractor_logging();
    (url, n)
}

/// [`spawn_extractor`], also keeping every request body so a test can check what the service
/// actually put in the extraction prompts (#881: the stub used to ignore them, which is how a
/// service running free-form passed this suite).
fn spawn_extractor_logging() -> (String, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let bodies2 = bodies.clone();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            n2.fetch_add(1, Ordering::SeqCst);
            let raw = read_body(&mut s);
            bodies2
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&raw).into_owned());
            let req: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
            let msg = |role: &str| -> String {
                req["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|m| m["role"] == role)
                    .and_then(|m| m["content"].as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let (sys, user) = (msg("system"), msg("user"));
            let tag: String = Sha256::digest(user.as_bytes())
                .iter()
                .take(4)
                .map(|b| format!("{b:02x}"))
                .collect();
            let content = if sys.contains("classifier") {
                let items: Vec<Value> = user
                    .find('[')
                    .and_then(|i| serde_json::from_str(&user[i..]).ok())
                    .unwrap_or_default();
                json!({"types": vec![""; items.len()]})
            } else if sys.contains("\"edges\": [{\"source_name\"") {
                json!({"edges": [{"source_name": format!("Person {tag}"),
                    "target_name": format!("Project {tag}"),
                    "fact": format!("Person {tag} owns Project {tag}"),
                    "relation_type": "OWNS", "valid_at": null, "invalid_at": null}]})
            } else if sys.contains("\"entities\": [{\"name\"") {
                json!({"entities": [
                    {"name": format!("Person {tag}"), "entity_type": "Person", "summary": format!("A person ({tag}).")},
                    {"name": format!("Project {tag}"), "entity_type": "Project", "summary": format!("A project ({tag}).")}]})
            } else {
                json!({})
            };
            respond(
                &mut s,
                &json!({"choices": [{"finish_reason": "stop",
                    "message": {"role": "assistant", "content": content.to_string()}}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1}})
                .to_string(),
            );
        }
    });
    (
        format!("http://127.0.0.1:{port}/v1/chat/completions"),
        n,
        bodies,
    )
}

/// True when some recorded extraction request carries the ontology's entity-type and
/// relation-type sections (their descriptions are in `ONTOLOGY`).
fn prompts_carry_ontology(bodies: &Mutex<Vec<String>>) -> (bool, bool) {
    let b = bodies.lock().unwrap();
    (
        b.iter()
            .any(|r| r.contains("ENTITY_TYPES") && r.contains("An initiative with an owner.")),
        b.iter()
            .any(|r| r.contains("FACT_TYPES") && r.contains("A project is hosted by a provider.")),
    )
}

fn lcg_eval(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lcg-eval"))
        .args(args)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("LCG_ANTHROPIC_ENDPOINT")
        .env_remove("LCG_EMBEDDING_URL")
        .output()
        .unwrap()
}

fn ok(o: &Output) {
    assert!(
        o.status.success(),
        "exit {:?}\nstderr:\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr)
    );
}

/// The report with run-to-run noise (timings, paths, the extraction source) removed.
fn stable(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| {
                    ![
                        "latency",
                        "seconds",
                        "_ms",
                        "extractor",
                        "kept_workspace",
                        "lcg_build",
                    ]
                    .iter()
                    .any(|x| k.contains(x))
                })
                .map(|(k, v)| (k.clone(), stable(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(stable).collect()),
        x => x.clone(),
    }
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Every file under `root`, with its bytes.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                stack.push(p);
            } else if e.file_type().unwrap().is_file() {
                out.insert(
                    p.strip_prefix(root).unwrap().display().to_string(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ontology_ingest_round_trips_and_the_kept_workspace_serves_integrity_and_graph_qa() {
    let Some(bin) = service_bin() else { return };
    let bin = bin.to_str().unwrap().to_string();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval");
    let corpus = corpus.to_str().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let t = |p: &str| tmp.path().join(p).to_str().unwrap().to_string();
    let onto = t("onto.yaml");
    std::fs::write(&onto, ONTOLOGY).unwrap();
    let embedder = spawn_embedder();
    let (extractor, extracted, prompts) = spawn_extractor_logging();
    let common = [
        "retrieval",
        "--corpus",
        corpus,
        "--configs",
        "lcg-passages",
        "--no-grounded",
        "--service-bin",
        &bin,
        "--embedder-http",
        &embedder,
        "--bootstrap-resamples",
        "100",
    ];

    // 1. Record under the ontology and keep the workspace.
    let (cassette, kept) = (t("cassette.jsonl"), t("kept"));
    let (out1, out2) = (t("r1"), t("r2"));
    let mut a: Vec<&str> = common.to_vec();
    a.extend([
        "--extractor-http",
        &extractor,
        "--ontology",
        &onto,
        "--record-cassette",
        &cassette,
        "--keep-workspace",
        &kept,
        "--out",
        &out1,
    ]);
    let rec = lcg_eval(&a);
    ok(&rec);
    assert!(extracted.load(Ordering::SeqCst) > 0);
    // #881 FR-007: the service really extracted under the ontology — its types are in the prompts.
    assert_eq!(
        prompts_carry_ontology(&prompts),
        (true, true),
        "the extraction prompts carry no ontology: the service ran free-form"
    );
    let r1 = read_json(&tmp.path().join("r1/report.json"));
    let hash = r1["meta"]["ontology_hash"].as_str().unwrap().to_string();
    assert_ne!(hash, "none");
    assert_eq!(r1["meta"]["ontology_mode"], "strict");
    assert_eq!(r1["meta"]["kept_workspace"], kept.as_str());
    let side = read_json(Path::new(&format!("{cassette}.meta.json")));
    assert_eq!(side["ontology_hash"], hash.as_str());
    let kept_p = Path::new(&kept);
    assert!(kept_p.join(".lcg/ontology.yaml").exists());
    assert!(kept_p.join(".lcg/db").exists());
    assert!(!kept_p.join(".lcg/service.sock").exists());
    let manifest = read_json(&kept_p.join("lcg-eval-workspace.json"));
    assert_eq!(manifest["ontology_hash"], hash.as_str());
    assert_eq!(manifest["embedder"], embedder.as_str());

    // 2. Replay with the same ontology: zero misses (a miss would reach the dead extractor URL and
    //    abort the ingest), identical metrics.
    let mut a: Vec<&str> = common.to_vec();
    a.extend([
        "--extractor-http",
        "http://127.0.0.1:1/v1/chat/completions",
        "--ontology",
        &onto,
        "--replay-cassette",
        &cassette,
        "--out",
        &out2,
    ]);
    let before = extracted.load(Ordering::SeqCst);
    let rep = lcg_eval(&a);
    ok(&rep);
    assert_eq!(extracted.load(Ordering::SeqCst), before);
    let r2 = read_json(&tmp.path().join("r2/report.json"));
    assert_eq!(stable(&r1["configs"]), stable(&r2["configs"]));
    assert_eq!(stable(&r1["ingest"]), stable(&r2["ingest"]));
    assert_eq!(r2["meta"]["ontology_hash"], hash.as_str());

    // 3. Replay under another ontology hash (open mode; no ontology): refused before ingest, and
    //    the error names both the cassette's hash and this run's.
    let open_hash = lcg_core::ontology::content_hash(Some(
        &lcg_core::ontology::load_ontology_from_path(
            Path::new(&onto),
            lcg_core::OntologyMode::Open,
        )
        .unwrap(),
    ));
    assert_ne!(open_hash, hash);
    for (extra, run_hash) in [
        (
            vec!["--ontology", onto.as_str(), "--ontology-mode", "open"],
            open_hash.as_str(),
        ),
        (vec![], "none"),
    ] {
        let mut a: Vec<&str> = common.to_vec();
        a.extend([
            "--extractor-http",
            &extractor,
            "--replay-cassette",
            &cassette,
        ]);
        a.extend(extra);
        // No wall-clock bound (it would flake on a loaded machine): the stderr checks below show
        // the refusal comes before the service starts, and no extraction request is made.
        let o = lcg_eval(&a);
        assert_eq!(o.status.code(), Some(1));
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(&hash), "{err}");
        assert!(err.contains(&format!("is {run_hash}")), "{err}");
        assert!(!err.contains("starting LCG service"), "{err}");
    }
    assert_eq!(extracted.load(Ordering::SeqCst), before);

    // 4. graph-integrity reads the kept workspace.
    let snap = snapshot(kept_p);
    let skeleton =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval-v2/skeleton.json");
    let gi = lcg_eval(&[
        "graph-integrity",
        "--workspace",
        &kept,
        "--skeleton",
        skeleton.to_str().unwrap(),
        "--out",
        &t("gi"),
    ]);
    ok(&gi);

    // 5. graph-qa on the kept workspace leaves it byte-for-byte unchanged.
    let smoke = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/graph-qa-smoke");
    let Mode::Run(args) = parse_args(
        &[
            "--workspace",
            &kept,
            "--corpus",
            corpus,
            "--questions",
            smoke.join("graph-questions.jsonl").to_str().unwrap(),
            "--plans",
            smoke.join("plans.json").to_str().unwrap(),
            "--runners",
            "passages,walk,route",
            "--service-bin",
            &bin,
            "--embedder-http",
            &embedder,
            "--llm-cache",
            &t("llm.jsonl"),
            "--bootstrap-resamples",
            "100",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>(),
    )
    .unwrap() else {
        panic!()
    };
    let model = ScriptedModel::new(|r| {
        Ok(
            if r.system.as_deref().unwrap_or("").starts_with("You plan") {
                json!({"route": "graph", "start": "Ana Silva", "hops": [{"types": ["REPORTS_TO"],
                "phrase": "reports", "direction": "in"}], "list": false, "history": false,
                "answer_type": "Person"})
                .to_string()
            } else {
                "{\"answers\": []}".to_string()
            },
        )
    });
    let out = run_on_workspace(&args, Some(&model), &model, "scripted", None)
        .await
        .unwrap_or_else(|f| panic!("graph-qa failed ({}): {}", f.code, f.message));
    let s = &out.json["scores"];
    assert_eq!(s["runners"], json!(["passages", "walk", "route"]));
    assert_eq!(s["per_question"].as_array().unwrap().len(), 6);
    assert_eq!(out.json["meta"]["workspace_kind"], "ontology");
    assert_eq!(out.json["meta"]["ontology_hash"], hash.as_str());
    assert!(
        snapshot(kept_p) == snap,
        "graph-qa or graph-integrity modified the workspace"
    );
}

fn onto_hash(path: &str, mode: lcg_core::OntologyMode) -> String {
    lcg_core::ontology::content_hash(Some(
        &lcg_core::ontology::load_ontology_from_path(Path::new(path), mode).unwrap(),
    ))
}

/// #881 FR-009: a service started through the harness's spawn path with a staged ontology reports
/// that ontology's hash for the eval group. Fails on the pre-fix code, where the child never got
/// `LIMINIS_WORKSPACE_ROOT` and so started free-form (and `spawn_service` could not tell).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_service_loads_the_staged_ontology() {
    use lcg_eval::retrieval::lcg::{spawn_service, RpcTransport, ServiceOpts, EVAL_GROUP};
    let Some(bin) = service_bin() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let onto = tmp.path().join("onto.yaml");
    std::fs::write(&onto, ONTOLOGY).unwrap();
    let hash = onto_hash(onto.to_str().unwrap(), lcg_core::OntologyMode::Strict);
    let opts = ServiceOpts {
        bin,
        embedder_http: Some(spawn_embedder()),
        extractor_http: Some(spawn_extractor().0),
        ontology_file: Some(onto),
        ontology_mode: Some(lcg_core::OntologyMode::Strict),
        ontology_hash: Some(hash.clone()),
        // No expectation: this test must see the hash itself, not rely on the guard.
        expect_ontology_hash: None,
        ..Default::default()
    };
    let guard = spawn_service(&opts).await.unwrap();
    let view = guard
        .client
        .call("knowledge_get_ontology", json!({"group_id": EVAL_GROUP}))
        .await
        .unwrap();
    assert_eq!(view["has_ontology"], true, "{view}");
    assert_eq!(view["ontology_hash"], hash.as_str(), "{view}");
    assert_eq!(guard.ontology.hash, hash);
    assert_eq!(guard.ontology.mode.as_deref(), Some("strict"));
}

/// #881 FR-010: a service that comes up without the staged ontology stops the run before any
/// ingest, with an error naming the expected hash. A wrapper script deletes the staged file just
/// before it `exec`s the real service — the same observable state as the original bug.
#[cfg(unix)]
#[test]
fn a_service_without_the_staged_ontology_fails_the_run_before_ingest() {
    use std::os::unix::fs::PermissionsExt;
    let Some(bin) = service_bin() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let t = |p: &str| tmp.path().join(p).to_str().unwrap().to_string();
    let onto = t("onto.yaml");
    std::fs::write(&onto, ONTOLOGY).unwrap();
    let wrapper = t("service-without-ontology.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n[ \"$1\" = \"--version\" ] || rm -f .lcg/ontology.yaml\nexec '{}' \"$@\"\n",
            bin.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let embedder = spawn_embedder();
    let (extractor, extracted) = spawn_extractor();
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval");
    let o = lcg_eval(&[
        "retrieval",
        "--corpus",
        corpus.to_str().unwrap(),
        "--configs",
        "lcg-passages",
        "--no-grounded",
        "--service-bin",
        &wrapper,
        "--embedder-http",
        &embedder,
        "--extractor-http",
        &extractor,
        "--ontology",
        &onto,
        "--out",
        &t("out"),
    ]);
    assert!(!o.status.success(), "the run must fail");
    let err = String::from_utf8_lossy(&o.stderr);
    let expected = onto_hash(&onto, lcg_core::OntologyMode::Strict);
    assert!(err.contains("ontology verification failed"), "{err}");
    assert!(err.contains("no ontology"), "{err}");
    assert!(err.contains(&expected), "{err}");
    assert!(!err.contains("ingesting"), "{err}");
    assert_eq!(extracted.load(Ordering::SeqCst), 0, "no extraction call");
}
