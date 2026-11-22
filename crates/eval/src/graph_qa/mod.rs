//! `lcg-eval graph-qa` (#821): graph question answering over a kept workspace, the reproducible
//! form of experiment E13 and the regression check for `knowledge_walk` (#820).
//!
//! Runners (FR-014): `passages` (the shipped default passage search), `passages-belief-rerank`,
//! `qmd` (optional), `walk` (an LLM planner sees the question and the workspace's ontology menu;
//! its plan runs through `knowledge_walk`) and `route` (the planner picks walk or passages). One
//! reader prompt answers from every runner's context; [`scorer`] scores the answers.
//!
//! The workspace is never opened in place: it is copied to a temp dir and the service runs on the
//! copy (ADR-0821, applying ADR-0819's copy principle to the service route of ADR-0754).

pub mod cache;
pub mod cli;
pub mod mock;
pub mod plans;
pub mod prompts;
pub mod report;
pub mod runner;
pub mod scorer;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::graph_integrity::source::find_db;
use crate::llm_client::{process_env, provider_configured, MessagesClient, ModelClient};
use crate::retrieval::generator::graph_questions::{self, GraphQuestion};
use crate::retrieval::lcg::{self, RpcTransport, EVAL_GROUP};
use crate::retrieval::{fixture, qmd, workspace};
use cache::LlmCache;
use cli::{GraphQaArgs, Mode};
use runner::{QmdSetup, QmdSource, RunError};

/// Exit codes (see `cli::usage`).
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_WORKSPACE: i32 = 3;
pub const EXIT_UNCOVERED: i32 = 4;

/// A run failure with its exit code.
#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub message: String,
}

fn fail(code: i32, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}

/// What a workspace is, read without opening its DB.
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    /// `ontology` or `free-form`.
    pub kind: String,
    pub ontology_hash: String,
    pub ontology_mode: Option<String>,
    pub manifest: Option<workspace::WorkspaceManifest>,
}

/// Checks that `ws` is an ingested workspace and reads its ontology and manifest. The embedder the
/// run will use must be the one the workspace was ingested with: a different one would compare
/// query vectors against incompatible stored vectors and give silently meaningless numbers.
///
/// The manifest records the embedder as an address, not a model identity, so the comparison goes
/// through [`embedder_key`] (loopback aliases, case and a trailing slash do not count), and
/// `allow_mismatch` (`--allow-embedder-mismatch`) downgrades a remaining mismatch to a warning, for
/// the same embedder reached at a different address (a relocated socket, say).
pub fn inspect_workspace(
    ws: &Path,
    embedder: &str,
    allow_mismatch: bool,
) -> Result<WorkspaceInfo, Failure> {
    find_db(ws, None).map_err(|e| fail(EXIT_WORKSPACE, e.to_string()))?;
    let manifest = workspace::read_manifest(ws).map_err(|e| fail(EXIT_WORKSPACE, e))?;
    if let Some(m) = &manifest {
        if !m.embedder.is_empty()
            && !embedder.is_empty()
            && embedder_key(&m.embedder) != embedder_key(embedder)
        {
            if allow_mismatch {
                eprintln!(
                    "lcg-eval graph-qa: WARNING workspace {} was ingested with embedder {} but \
                     this run uses {}; proceeding because of --allow-embedder-mismatch. The scores \
                     are only meaningful if both addresses serve the same embedding model.",
                    ws.display(),
                    m.embedder,
                    workspace::redact_endpoint(embedder)
                );
            } else {
                return Err(fail(
                    EXIT_WORKSPACE,
                    format!(
                        "workspace {} was ingested with embedder {} but this run is configured \
                         with {}; its stored vectors are not comparable. Pass the ingest \
                         embedder, or --allow-embedder-mismatch if this address serves the same \
                         embedding model.",
                        ws.display(),
                        m.embedder,
                        workspace::redact_endpoint(embedder)
                    ),
                ));
            }
        }
    }
    // The ontology the service will use for the eval group: the per-group file first, then the
    // workspace file (ADR-0446) — the same resolution order the service applies (#881).
    let onto = lcg_core::ontology::load_group_ontology(ws, EVAL_GROUP)
        .or_else(|| lcg_core::ontology::load_ontology(Some(ws)));
    Ok(WorkspaceInfo {
        kind: if onto.is_some() {
            "ontology"
        } else {
            "free-form"
        }
        .into(),
        ontology_hash: lcg_core::ontology::content_hash(onto.as_ref()),
        ontology_mode: onto.as_ref().map(|o| o.mode.to_string()),
        manifest,
    })
}

/// An embedder address in comparable form. For an `http(s)` URL the scheme and host are lowercased,
/// `localhost` and `[::1]` become `127.0.0.1`, and a trailing `/` is dropped, so the same local
/// embedder typed two ways compares equal. Anything else (a socket path) is only trimmed.
pub fn embedder_key(addr: &str) -> String {
    let redacted = workspace::redact_endpoint(addr);
    let s = redacted.as_str();
    let Some((scheme, rest)) = s.split_once("://") else {
        return s.to_string();
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return s.to_string();
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let authority = authority.to_ascii_lowercase();
    let (host, port) = if let Some(after) = authority.strip_prefix('[') {
        match after.split_once(']') {
            Some((h, p)) => (format!("[{h}]"), p.to_string()),
            None => (authority.clone(), String::new()),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), format!(":{p}")),
            None => (authority.clone(), String::new()),
        }
    };
    let host = match host.as_str() {
        "localhost" | "[::1]" => "127.0.0.1".to_string(),
        _ => host,
    };
    format!("{scheme}://{host}{port}{}", path.trim_end_matches('/'))
}

/// The embedding profile to run the service with. The query and hop-phrase vectors must be made with
/// the same instruction prefixes as the stored ones, so a profile recorded in the manifest is used
/// when none is given, and a different one given explicitly is refused (exit 3).
pub fn resolve_embedding_profile(
    ws: &Path,
    manifest: Option<&workspace::WorkspaceManifest>,
    requested: Option<&str>,
) -> Result<Option<String>, Failure> {
    let recorded = manifest.and_then(|m| m.embedding_profile.as_deref());
    match (recorded, requested) {
        (Some(r), Some(q)) if r != q => Err(fail(
            EXIT_WORKSPACE,
            format!(
                "workspace {} was ingested with --embedding-profile {r} but this run asks for {q}; \
                 its stored vectors were embedded with that profile's prefixes. Pass \
                 --embedding-profile {r}, or omit it.",
                ws.display()
            ),
        )),
        (_, Some(q)) => Ok(Some(q.to_string())),
        (Some(r), None) => Ok(Some(r.to_string())),
        (None, None) => Ok(None),
    }
}

pub fn load_questions(path: &Path) -> Result<Vec<GraphQuestion>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("graph questions {}: {e}", path.display()))?;
    let qs = graph_questions::parse_jsonl(&text)?;
    if qs.is_empty() {
        return Err(format!("graph questions {}: no questions", path.display()));
    }
    Ok(qs)
}

/// Everything a graph-qa run produced.
pub struct Outcome {
    pub json: Value,
    pub markdown: String,
    /// Set when a runner's reader parse-error rate is over [`READER_ERROR_LIMIT`] (and
    /// `--allow-reader-errors` is off): the run then exits 1 after writing its report.
    pub reader_failure: Option<Failure>,
}

/// The share of a runner's reader responses that may fail to parse before the run fails. A reader
/// that answers in prose (a bridge that ignores `output_config.format`, say) scores every question
/// 0, which must never pass as a measurement.
pub const READER_ERROR_LIMIT: f64 = 0.10;

/// Checks the reader parse-error rate of every runner. Any parse error is reported on stderr; a
/// runner over [`READER_ERROR_LIMIT`] fails the run (exit 1) unless `allow` (`--allow-reader-errors`)
/// downgrades it to a warning.
pub fn check_reader_errors(run: &runner::RunOutput, allow: bool) -> Result<(), Failure> {
    let errs = report::reader_parse_errors(run);
    let mut over = Vec::new();
    for (r, (e, n)) in &errs {
        if *e == 0 {
            continue;
        }
        let rate = *e as f64 / (*n).max(1) as f64;
        eprintln!(
            "lcg-eval graph-qa: WARNING runner {r}: {e} of {n} reader responses did not parse \
             ({:.0}%) and were scored 0",
            rate * 100.0
        );
        if rate > READER_ERROR_LIMIT {
            over.push(format!("{r} ({e}/{n})"));
        }
    }
    if over.is_empty() || allow {
        return Ok(());
    }
    Err(fail(
        EXIT_FAILED,
        format!(
            "reader parse-error rate over {:.0}% for runner(s) {}: the reader is not answering in \
             the expected JSON (does the provider honour output_config.format?), so these scores \
             are not a measurement. Pass --allow-reader-errors to report them anyway.",
            READER_ERROR_LIMIT * 100.0,
            over.join(", ")
        ),
    ))
}

/// A whole graph-qa run over `a.workspace`, with the given planner and reader clients (`planner:
/// None` means no planner may be called). Used by the CLI and by the end-to-end tests.
pub async fn run_on_workspace(
    a: &GraphQaArgs,
    planner: Option<&dyn ModelClient>,
    reader: &dyn ModelClient,
    provider: &str,
    qmd_runner: Option<Arc<dyn qmd::QmdRunner>>,
) -> Result<Outcome, Failure> {
    let embedder = a
        .embedder_http
        .clone()
        .or(a.embedder_uds.clone())
        .or_else(|| std::env::var("LCG_EMBEDDING_URL").ok())
        .unwrap_or_default();
    let info = inspect_workspace(&a.workspace, &embedder, a.allow_embedder_mismatch)?;
    let embedding_profile = resolve_embedding_profile(
        &a.workspace,
        info.manifest.as_ref(),
        a.embedding_profile.as_deref(),
    )?;
    if info.manifest.is_none() {
        eprintln!(
            "lcg-eval graph-qa: WARNING {} has no {}; its provenance (corpus, embedder) is unknown",
            a.workspace.display(),
            workspace::MANIFEST_FILE
        );
    }
    let qpath = a
        .questions
        .clone()
        .unwrap_or_else(|| a.corpus.join(graph_questions::FILE));
    let questions = load_questions(&qpath).map_err(|e| fail(EXIT_USAGE, e))?;
    let plans_file = match &a.plans {
        Some(p) => Some(plans::load_plans(p).map_err(|e| fail(EXIT_USAGE, e))?),
        None => None,
    };
    let cache = LlmCache::load(&a.llm_cache).map_err(|e| fail(EXIT_FAILED, e))?;

    // qmd before the service, so a skipped qmd costs nothing.
    let qmd = if a.runners.iter().any(|r| r == "qmd") {
        Some(qmd_setup(a, qmd_runner).await)
    } else {
        None
    };

    let opts = lcg::ServiceOpts {
        bin: a
            .service_bin
            .clone()
            .unwrap_or_else(crate::retrieval::default_service_bin),
        embedder_http: a.embedder_http.clone(),
        embedder_uds: a.embedder_uds.clone(),
        reranker_http: a.reranker_http.clone(),
        reranker_uds: a.reranker_uds.clone(),
        embedding_profile,
        seed_workspace: Some(a.workspace.clone()),
        // #881: a workspace that has an ontology must be served under it, or the walk planner's
        // menu and the graph are silently free-form.
        expect_ontology_hash: (info.kind == "ontology").then(|| info.ontology_hash.clone()),
        ..Default::default()
    };
    eprintln!(
        "lcg-eval graph-qa: starting LCG service on a copy of {}",
        a.workspace.display()
    );
    let guard = lcg::spawn_service(&opts)
        .await
        .map_err(|e| fail(EXIT_FAILED, e))?;
    let transport: Arc<dyn RpcTransport> = guard.client.clone();
    let view = transport
        .call("knowledge_get_ontology", json!({"group_id": EVAL_GROUP}))
        .await
        .map_err(|e| fail(EXIT_FAILED, e))?;
    let out = runner::run(runner::Inputs {
        questions: &questions,
        runners: &a.runners,
        transport,
        ontology_view: &view,
        plans_file: plans_file.as_ref(),
        plans_only: a.plans_only,
        planner,
        reader,
        planner_model: &a.planner_model,
        reader_model: &a.reader_model,
        cache: &cache,
        qmd,
        k: a.k,
        concurrency: a.concurrency,
    })
    .await
    .map_err(|e| match e {
        RunError::Uncovered(_) => fail(EXIT_UNCOVERED, e.to_string()),
        RunError::Failed(_) => fail(EXIT_FAILED, e.to_string()),
    })?;
    let meta = report::Meta {
        workspace: a.workspace.display().to_string(),
        workspace_kind: info.kind,
        // What the service reported (#881); equals the workspace's when it has an ontology.
        ontology_hash: guard.ontology.hash.clone(),
        ontology_mode: guard.ontology.mode.clone(),
        manifest: info.manifest,
        lcg_build: guard.version.clone(),
        // The report is written to disk and stdout: never the endpoint's credential.
        embedder: workspace::redact_endpoint(&embedder),
        questions_file: qpath.display().to_string(),
        planner_model: a.planner_model.clone(),
        reader_model: a.reader_model.clone(),
        provider: provider.to_string(),
        plans_file: a.plans.as_ref().map(|p| p.display().to_string()),
        plans_model: plans_file.as_ref().and_then(|f| f.model.clone()),
        plans_ontology: plans_file.as_ref().and_then(|f| f.ontology.clone()),
        k: a.k,
        seed: a.bootstrap_seed,
        resamples: a.bootstrap_resamples,
        cache: cache.counts(),
    };
    let rep = report::Report {
        meta,
        run: &out,
        questions: &questions,
        reference: plans_file
            .as_ref()
            .and_then(|f| f.route_planned5_scores.as_ref()),
    };
    let outcome = Outcome {
        json: rep.to_json(),
        markdown: rep.render_markdown(),
        reader_failure: check_reader_errors(&out, a.allow_reader_errors).err(),
    };
    drop(guard);
    Ok(outcome)
}

async fn qmd_setup(a: &GraphQaArgs, runner: Option<Arc<dyn qmd::QmdRunner>>) -> QmdSetup {
    let fx = match fixture::load(&a.corpus) {
        Ok(f) => f,
        Err(e) => return QmdSetup::Skipped(format!("cannot load --corpus for qmd: {e}")),
    };
    let runner: Arc<dyn qmd::QmdRunner> = runner.unwrap_or_else(|| {
        Arc::new(qmd::ProcessRunner {
            bin: a.qmd_bin.clone(),
        })
    });
    let index = a
        .qmd_index
        .clone()
        .unwrap_or_else(|| crate::retrieval::default_qmd_index(&fx));
    match qmd::prepare(runner.as_ref(), &index, &fx.notes_dir()).await {
        Ok(_) => QmdSetup::Ready(QmdSource {
            runner,
            index,
            dates: fx
                .manifest
                .notes
                .iter()
                .filter_map(|n| n.reference_time.clone().map(|t| (n.id.clone(), t)))
                .collect(),
            notes: fx.notes.clone(),
        }),
        Err(qmd::Skipped(why)) => {
            eprintln!("lcg-eval graph-qa: qmd skipped: {why}");
            QmdSetup::Skipped(why)
        }
    }
}

/// Writes `graph-qa.md` and `graph-qa.json` to `dir`.
pub fn write_outputs(dir: &Path, o: &Outcome) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create --out {}: {e}", dir.display()))?;
    std::fs::write(dir.join("graph-qa.md"), &o.markdown)
        .map_err(|e| format!("cannot write graph-qa.md: {e}"))?;
    std::fs::write(
        dir.join("graph-qa.json"),
        serde_json::to_string_pretty(&o.json).expect("serializes") + "\n",
    )
    .map_err(|e| format!("cannot write graph-qa.json: {e}"))
}

pub async fn run_cli(args: &[String]) -> i32 {
    let a = match cli::parse_args(args) {
        Ok(Mode::Help) => {
            println!("{}", cli::usage());
            return 0;
        }
        Ok(Mode::Run(a)) => *a,
        Err(e) => {
            eprintln!("lcg-eval graph-qa: {e}");
            return EXIT_USAGE;
        }
    };
    let configured = provider_configured(&process_env, &a.anthropic);
    let client = if configured {
        match MessagesClient::from_env(&a.anthropic) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("lcg-eval graph-qa: {e}");
                return EXIT_USAGE;
            }
        }
    } else {
        None
    };
    let provider = client
        .as_ref()
        .map_or("none configured (cache only)".to_string(), |c| c.describe());
    let unconfigured = mock::Unconfigured;
    let reader: &dyn ModelClient = match &client {
        Some(c) => c,
        None => &unconfigured,
    };
    let planner: Option<&dyn ModelClient> = client.as_ref().map(|c| c as &dyn ModelClient);
    match run_on_workspace(&a, planner, reader, &provider, None).await {
        Ok(o) => {
            println!("{}", o.markdown);
            if let Some(dir) = &a.out {
                if let Err(e) = write_outputs(dir, &o) {
                    eprintln!("lcg-eval graph-qa: {e}");
                    return EXIT_FAILED;
                }
                eprintln!(
                    "lcg-eval graph-qa: wrote graph-qa.md and graph-qa.json to {}",
                    dir.display()
                );
            }
            match o.reader_failure {
                Some(f) => {
                    eprintln!("lcg-eval graph-qa: {}", f.message);
                    f.code
                }
                None => 0,
            }
        }
        Err(f) => {
            eprintln!("lcg-eval graph-qa: {}", f.message);
            f.code
        }
    }
}

/// The default questions path for a corpus.
pub fn default_questions(corpus: &Path) -> PathBuf {
    corpus.join(graph_questions::FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedder_addresses_compare_through_loopback_case_and_trailing_slash() {
        let same = [
            ("http://localhost:8080", "http://127.0.0.1:8080"),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
            (
                "HTTP://LocalHost:8080/embed/",
                "http://127.0.0.1:8080/embed",
            ),
            ("http://[::1]:8080", "http://localhost:8080"),
            (" /tmp/e.sock ", "/tmp/e.sock"),
            // The manifest stores the address without credentials; the run's must still match.
            (
                "http://user:tok@localhost:8080/e?key=s3cret",
                "http://127.0.0.1:8080/e",
            ),
        ];
        for (a, b) in same {
            assert_eq!(embedder_key(a), embedder_key(b), "{a} vs {b}");
        }
        let different = [
            ("http://127.0.0.1:8080", "http://127.0.0.1:8081"),
            ("http://127.0.0.1:8080/a", "http://127.0.0.1:8080/b"),
            ("http://10.0.0.2:8080", "http://127.0.0.1:8080"),
            ("/tmp/a.sock", "/tmp/b.sock"),
        ];
        for (a, b) in different {
            assert_ne!(embedder_key(a), embedder_key(b), "{a} vs {b}");
        }
    }

    #[test]
    fn an_embedder_mismatch_is_refused_unless_allowed() {
        let d = tempfile::tempdir().unwrap();
        let ws = d.path();
        std::fs::create_dir_all(ws.join(".lcg/db")).unwrap();
        std::fs::write(ws.join(".lcg/db/liminis.db"), b"").unwrap();
        let m = workspace::WorkspaceManifest {
            embedder: "http://localhost:8080".into(),
            ..Default::default()
        };
        workspace::write_manifest(ws, &m).unwrap();
        // An alias of the ingest address is accepted.
        inspect_workspace(ws, "http://127.0.0.1:8080/", false).unwrap();
        // A different address is refused, and the error names the override.
        let e = inspect_workspace(ws, "/tmp/other.sock", false).unwrap_err();
        assert_eq!(e.code, EXIT_WORKSPACE);
        assert!(
            e.message.contains("--allow-embedder-mismatch"),
            "{}",
            e.message
        );
        // The override lets it through.
        inspect_workspace(ws, "/tmp/other.sock", true).unwrap();
    }

    #[test]
    fn the_ingest_embedding_profile_is_inherited_and_a_different_one_refused() {
        let ws = Path::new("/w");
        let m = workspace::WorkspaceManifest {
            embedding_profile: Some("bge-v1.5".into()),
            ..Default::default()
        };
        let r = |m: Option<&workspace::WorkspaceManifest>, q: Option<&str>| {
            resolve_embedding_profile(ws, m, q)
        };
        assert_eq!(r(Some(&m), None).unwrap().as_deref(), Some("bge-v1.5"));
        assert_eq!(
            r(Some(&m), Some("bge-v1.5")).unwrap().as_deref(),
            Some("bge-v1.5")
        );
        let e = r(Some(&m), Some("none")).unwrap_err();
        assert_eq!(e.code, EXIT_WORKSPACE);
        assert!(e.message.contains("bge-v1.5"), "{}", e.message);
        // No recorded profile (or no manifest): whatever the run asks for.
        let free = workspace::WorkspaceManifest::default();
        assert_eq!(
            r(Some(&free), Some("none")).unwrap().as_deref(),
            Some("none")
        );
        assert_eq!(r(None, None).unwrap(), None);
    }
}
