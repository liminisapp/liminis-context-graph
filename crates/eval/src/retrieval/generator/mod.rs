//! Generator for the retrieval-v2 fixture (#788, ADR-0788).
//!
//! The fixture is built in two steps so that structure is reproducible and prose is natural:
//!
//! 1. **Skeleton (deterministic, free).** `scenario.json` (quotas, seed) plus the fact model in
//!    code produce `skeleton.json`: entities and facts, which note carries which facts, each
//!    note's origin, reference time, size and citations, the explicit supersessions, and the
//!    question ground truth. Same scenario and seed ⇒ byte-identical skeleton. No network, no model.
//! 2. **Render (explicit, paid, once).** `lcg-eval retrieval-render` has an LLM write each note and
//!    each question's wording from the skeleton ([`render`]). The rendered notes and questions are
//!    **checked in**; CI and ordinary eval runs never call a model. [`check`] verifies the
//!    rendered text against the skeleton and detects drift.
//!
//! Template prose ([`notes`] builds it) exists only as a test double for the LLM so the pipeline
//! can be exercised without one; it is never written into the checked-in fixture.

pub mod check;
pub mod graph_questions;
pub mod notes;
pub mod pyrandom;
pub mod questions;
pub mod render;
pub mod skeleton;
pub mod variants;
pub mod vocab;
pub mod world;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use self::check::Rendered;
use self::skeleton::*;
use self::world::{rfc3339, Rng, World};
use super::fixture;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionQuotas {
    pub direct: usize,
    pub paraphrase: usize,
    pub temporal: usize,
    pub answer: usize,
    pub multi_hop: usize,
    pub passage: usize,
    pub negative: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LongNotes {
    /// Notes of 500–2,000 words.
    pub mid: usize,
    /// Notes over 5,000 words.
    pub huge: usize,
}

/// The checked-in description of the corpus to generate.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub seed: u64,
    /// Bump when the prose is re-rendered: re-rendering changes the corpus.
    pub corpus_version: String,
    pub questions_version: String,
    pub description: String,
    #[serde(default = "default_chunk")]
    pub chunk_max_tokens: usize,
    /// Decision chains of 3–5 revisions each.
    pub chains: usize,
    /// Near-duplicate primary notes restating a fact.
    pub recaps: usize,
    pub long_notes: LongNotes,
    pub external_notes: usize,
    /// Of the direct questions, how many are about an external clipping's fact.
    pub external_direct: usize,
    /// Share (percent) of an entity's facts that its derived page restates.
    pub derived_restate_percent: usize,
    /// Derived pages over decision chains (every second one lags and restates a stale value).
    pub derived_chain_pages: usize,
    pub answer_notes: usize,
    pub questions: QuestionQuotas,
    /// Questions per category in the human-review sample.
    #[serde(default = "default_review")]
    pub review_per_category: usize,
}

fn default_chunk() -> usize {
    fixture::DEFAULT_MAX_CHUNK_TOKENS
}
fn default_review() -> usize {
    6
}

pub fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read scenario {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Result of the deterministic step.
#[derive(Debug, Clone)]
pub struct Generated {
    pub skeleton: Skeleton,
    /// Template prose, a test double for the LLM renderer. Never checked in.
    pub template: Rendered,
    pub review_per_category: usize,
}

/// Builds the skeleton (and the template test double) in memory.
pub fn generate(sc: &Scenario) -> Result<Generated, String> {
    let root = Rng::new(sc.seed);
    let mut w = World::build(sc, &root)?;
    let corpus = notes::build(sc, &mut w, &root)?;
    let mut qs = questions::build(sc, &w, &corpus, &root)?;

    // Manifest order = ingest order: by recording date, creation order breaking ties. A derived
    // or answer note is always dated after the notes it cites, so citations always point earlier.
    let mut ordered: Vec<&notes::GenNote> = corpus.notes.iter().collect();
    ordered.sort_by_key(|n| (n.day, n.seq));
    let sk_notes: Vec<SkNote> = ordered.iter().map(|n| n.sk.clone()).collect();
    questions::complete_relevance(&mut qs, &sk_notes);
    let skeleton = Skeleton {
        skeleton_version: SKELETON_VERSION,
        corpus_version: sc.corpus_version.clone(),
        questions_version: sc.questions_version.clone(),
        description: sc.description.clone(),
        seed: sc.seed,
        chunk_max_tokens: sc.chunk_max_tokens,
        entities: w.entities.iter().map(notes::entity_sk).collect(),
        notes: sk_notes,
        supersedes: corpus
            .supersedes
            .iter()
            .map(|(old, new, day)| SkSup {
                old_note: old.clone(),
                new_note: new.clone(),
                effective_at: rfc3339(*day),
            })
            .collect(),
        questions: qs,
    };

    let template = Rendered {
        notes: corpus
            .notes
            .iter()
            .map(|n| (n.sk.id.clone(), n.body.clone()))
            .collect(),
        questions: skeleton
            .questions
            .iter()
            .map(|q| (q.id.clone(), q.text_hint.clone()))
            .collect(),
    };

    // Re-validate the structure through the production loader, with the hints as wording.
    let man = manifest_json(&skeleton);
    let m = fixture::parse_manifest(man.as_bytes())
        .map_err(|e| format!("generated manifest is invalid: {e}"))?;
    let known: HashSet<&str> = m.notes.iter().map(|n| n.id.as_str()).collect();
    let qtext = questions_jsonl(&skeleton, &template.questions)?;
    fixture::parse_questions(&qtext, &known)
        .map_err(|e| format!("generated questions are invalid: {e}"))?;
    let findings = check::check_skeleton(&skeleton);
    if !findings.errors.is_empty() {
        return Err(format!(
            "generated skeleton is inconsistent:\n{}",
            check::describe(&findings)
        ));
    }
    Ok(Generated {
        skeleton,
        template,
        review_per_category: sc.review_per_category,
    })
}

// ── manifest and questions from the skeleton (no prose involved) ─────────────────

#[derive(Serialize)]
struct ManNote<'a> {
    id: &'a str,
    reference_time: &'a str,
    origin: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cites: &'a Vec<String>,
}
#[derive(Serialize)]
struct ManSup<'a> {
    old_note: &'a str,
    new_note: &'a str,
    effective_at: &'a str,
}
#[derive(Serialize)]
struct Chunking {
    max_tokens: usize,
}
#[derive(Serialize)]
struct Man<'a> {
    corpus_version: &'a str,
    description: &'a str,
    chunking: Chunking,
    notes: Vec<ManNote<'a>>,
    supersedes: Vec<ManSup<'a>>,
}

fn pretty<T: Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("serializable");
    s.push('\n');
    s
}

/// `manifest.json`, fully determined by the skeleton.
pub fn manifest_json(sk: &Skeleton) -> String {
    pretty(&Man {
        corpus_version: &sk.corpus_version,
        description: &sk.description,
        chunking: Chunking {
            max_tokens: sk.chunk_max_tokens,
        },
        notes: sk
            .notes
            .iter()
            .map(|n| ManNote {
                id: &n.id,
                reference_time: &n.reference_time,
                origin: &n.origin,
                cites: &n.cites,
            })
            .collect(),
        supersedes: sk
            .supersedes
            .iter()
            .map(|s| ManSup {
                old_note: &s.old_note,
                new_note: &s.new_note,
                effective_at: &s.effective_at,
            })
            .collect(),
    })
}

#[derive(Serialize)]
struct RelOut<'a> {
    note: &'a str,
    gain: u32,
}
#[derive(Serialize)]
struct EvOut<'a> {
    note: &'a str,
    key_fact: &'a str,
}
#[derive(Serialize)]
struct QOut<'a> {
    id: &'a str,
    category: &'a str,
    text: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relevant_notes: Vec<RelOut<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<EvOut<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    current: &'a Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stale: &'a Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    negative: bool,
}

/// `questions.jsonl`: the skeleton's ground truth with `texts` as wording. The ground truth is
/// fixed by the skeleton, so a hand edit of anything but the wording shows up as drift.
pub fn questions_jsonl(sk: &Skeleton, texts: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    for q in &sk.questions {
        let text = texts
            .get(&q.id)
            .ok_or_else(|| format!("question '{}' has no wording", q.id))?;
        let line = QOut {
            id: &q.id,
            category: &q.category,
            text,
            relevant_notes: q
                .relevant_notes
                .iter()
                .map(|r| RelOut {
                    note: &r.note,
                    gain: r.gain,
                })
                .collect(),
            evidence: q
                .evidence
                .iter()
                .map(|e| EvOut {
                    note: &e.note,
                    key_fact: &e.key_fact,
                })
                .collect(),
            current: &q.current,
            stale: &q.stale,
            negative: q.negative,
        };
        out.push_str(&serde_json::to_string(&line).expect("serializable"));
        out.push('\n');
    }
    Ok(out)
}

// ── fixture files ────────────────────────────────────────────────────────────────

/// Who rendered the prose and how; written to `render.json` and mirrored in
/// `questions.meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderMeta {
    pub render_model: String,
    pub prompt_version: String,
    pub rendered_on: String,
    pub skeleton_hash: String,
    pub corpus_version: String,
    pub questions_version: String,
    pub model_calls: usize,
}

#[derive(Serialize)]
struct MetaFile<'a> {
    questions_version: &'a str,
    render_model: &'a str,
    prompt_version: &'a str,
    skeleton_hash: &'a str,
    /// Hash of `prose-facts.json` when the fixture has one (#907); `retrieval-gen --check` fails
    /// when the file changes without this being refreshed alongside a `questions_version` bump.
    #[serde(skip_serializing_if = "Option::is_none")]
    prose_facts_hash: Option<String>,
}

/// Writes manifest, notes, questions, `questions.meta.json` and `render.json` for `rendered`.
/// Removes notes that are no longer in the skeleton. Does not touch `skeleton.json`,
/// `scenario.json` or `review.json`.
pub fn write_fixture(
    dir: &Path,
    sk: &Skeleton,
    rendered: &Rendered,
    meta: &RenderMeta,
) -> Result<(), String> {
    let notes_dir = dir.join("notes");
    std::fs::create_dir_all(&notes_dir).map_err(|e| format!("{}: {e}", notes_dir.display()))?;
    if let Ok(rd) = std::fs::read_dir(&notes_dir) {
        for e in rd.flatten() {
            let id = e
                .path()
                .file_stem()
                .map(|s| s.to_string_lossy().to_string());
            if e.path().extension().is_some_and(|x| x == "md")
                && id.is_some_and(|i| sk.note(&i).is_none())
            {
                std::fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    let w = |name: &str, text: &str| -> Result<(), String> {
        let p = dir.join(name);
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    };
    for n in &sk.notes {
        let body = rendered
            .notes
            .get(&n.id)
            .ok_or_else(|| format!("note '{}' has no rendered text", n.id))?;
        let mut body = body.trim_end().to_string();
        body.push('\n');
        w(&format!("notes/{}.md", n.id), &body)?;
    }
    w("manifest.json", &manifest_json(sk))?;
    w(
        "questions.jsonl",
        &questions_jsonl(sk, &rendered.questions)?,
    )?;
    w(
        "questions.meta.json",
        &pretty(&MetaFile {
            questions_version: &sk.questions_version,
            render_model: &meta.render_model,
            prompt_version: &meta.prompt_version,
            skeleton_hash: &meta.skeleton_hash,
            prose_facts_hash: std::fs::read_to_string(dir.join(graph_questions::PROSE_FACTS_FILE))
                .ok()
                .map(|t| graph_questions::prose_facts_hash(&t)),
        }),
    )?;
    w("render.json", &pretty(meta))
}

/// Checks a rendered fixture directory against `sk`: structure pinned to the skeleton, prose
/// checked by [`check::check_rendered`], and the loader run over the result.
pub fn check_dir(
    dir: &Path,
    sk: &Skeleton,
    skeleton_text: &str,
) -> Result<check::Findings, String> {
    let rendered = check::load_rendered(dir)?;
    let mut f = check::check_rendered(sk, &rendered);
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default();
    if read("manifest.json") != manifest_json(sk) {
        f.errors.push(
            "manifest.json differs from the skeleton; it is generated, not hand-edited".into(),
        );
    }
    match questions_jsonl(sk, &rendered.questions) {
        Ok(expect) if expect == read("questions.jsonl") => {}
        Ok(_) => f.errors.push(
            "questions.jsonl differs from the skeleton's ground truth (only the wording may change)"
                .into(),
        ),
        Err(e) => f.errors.push(e),
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("render.json")) {
        match serde_json::from_str::<RenderMeta>(&text) {
            Ok(m) => {
                if m.skeleton_hash != skeleton_hash(skeleton_text) {
                    f.errors.push(format!(
                        "render.json was rendered from skeleton {} but skeleton.json is {}: re-render",
                        m.skeleton_hash,
                        skeleton_hash(skeleton_text)
                    ));
                }
                if m.corpus_version != sk.corpus_version
                    || m.questions_version != sk.questions_version
                {
                    f.errors
                        .push("render.json versions differ from the skeleton's".into());
                }
            }
            Err(e) => f.errors.push(format!("render.json: {e}")),
        }
    } else {
        f.errors.push(
            "render.json is missing (the render model and prompt version are recorded there)"
                .into(),
        );
    }
    match fixture::load(dir) {
        Ok(fx) => {
            for e in fx.v2_shape_errors() {
                f.errors.push(format!(
                    "corpus shape: {e} (the rendered prose came out shorter or longer than the skeleton's targets; re-render the affected notes with --only)"
                ));
            }
        }
        Err(e) => f
            .errors
            .push(format!("the loader rejects the fixture: {e}")),
    }
    Ok(f)
}

// ── review record ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ReviewItem {
    id: String,
    category: String,
    /// Filled by `retrieval-render` so the reviewer sees the question.
    text: Option<String>,
    ground_truth_correct: Option<bool>,
    note_relevance_correct: Option<bool>,
    key_fact_correct: Option<bool>,
}
#[derive(Serialize)]
struct Review<'a> {
    status: &'a str,
    skeleton_hash: String,
    questions_version: &'a str,
    /// Set by `retrieval-render` once there is a questions.jsonl to hash.
    questions_hash: Option<String>,
    sample_policy: String,
    reviewer: Option<String>,
    reviewed_on: Option<String>,
    error_rate_threshold: f64,
    error_rate: Option<f64>,
    sample: Vec<ReviewItem>,
}

/// A pending review template with a seeded, stratified sample and **empty verdicts**. Verdicts are
/// a human's to write; neither the generator nor the renderer ever fills them.
pub fn review_template(g: &Generated, skeleton_text: &str, root_seed: u64) -> String {
    let root = Rng::new(root_seed);
    let mut by_cat: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for q in &g.skeleton.questions {
        by_cat.entry(q.category.as_str()).or_default().push(&q.id);
    }
    let mut sample = Vec::new();
    for (cat, mut ids) in by_cat {
        root.fork(&format!("review|{cat}")).shuffle(&mut ids);
        let mut picked: Vec<&str> = ids.into_iter().take(g.review_per_category).collect();
        picked.sort();
        for id in picked {
            sample.push(ReviewItem {
                id: id.to_string(),
                category: cat.to_string(),
                text: None,
                ground_truth_correct: None,
                note_relevance_correct: None,
                key_fact_correct: None,
            });
        }
    }
    pretty(&Review {
        status: "pending",
        skeleton_hash: skeleton_hash(skeleton_text),
        questions_version: &g.skeleton.questions_version,
        questions_hash: None,
        sample_policy: format!(
            "stratified: {} questions per category (seeded sample, at least 10% of all questions). A maintainer fills every verdict (true/false) after reading the rendered question, its relevant notes and its key fact, then sets status to \"reviewed\" and records reviewer, reviewed_on and error_rate. Nobody else writes verdicts.",
            g.review_per_category
        ),
        reviewer: None,
        reviewed_on: None,
        error_rate_threshold: 0.05,
        error_rate: None,
        sample,
    })
}

// ── CLI: retrieval-gen ───────────────────────────────────────────────────────────

pub const USAGE: &str = "lcg-eval retrieval-gen — regenerate (or check) the retrieval-v2 skeleton

USAGE:
    lcg-eval retrieval-gen --spec <scenario.json> --out <DIR> [--check] [--seed <N>]

The skeleton (skeleton.json) is everything about the fixture except its prose: entities, facts,
which note carries which fact, origins, citations, sizes, supersessions and the question ground
truth. It is deterministic: the same scenario and seed give byte-identical output, with no network
access or model calls. The prose is written separately, once, by `lcg-eval retrieval-render`.

OPTIONS:
    --spec <FILE>   Scenario (quotas and seed), e.g. crates/eval/fixtures/retrieval-v2/scenario.json
    --out <DIR>     Fixture directory (writes skeleton.json and graph-questions.jsonl; review.json
                    only when absent). Reads prose-facts.json from it when present: prose-introduced
                    facts folded into the graph-question gold answers (stated) or listed as
                    optional_answers (ambiguous). An invalid file is an error.
    --check         Regenerate in memory and exit 1 if skeleton.json or graph-questions.jsonl
                    differs. If the fixture has been
                    rendered (render.json exists) also check the rendered notes and questions
                    against the skeleton. Writes nothing.
    --seed <N>      Override the scenario seed (write to a scratch --out)
    -h, --help      Print this help
";

fn run_gen(args: &[String]) -> Result<i32, String> {
    let mut spec = None;
    let mut out = None;
    let mut check = false;
    let mut seed = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(0);
            }
            "--check" => check = true,
            flag @ ("--spec" | "--out" | "--seed") => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                match flag {
                    "--spec" => spec = Some(v.clone()),
                    "--out" => out = Some(v.clone()),
                    _ => {
                        seed = Some(
                            v.parse::<u64>()
                                .map_err(|_| "--seed must be an unsigned integer".to_string())?,
                        )
                    }
                }
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
        i += 1;
    }
    let (Some(spec), Some(out)) = (spec, out) else {
        return Err(format!("--spec and --out are required\n\n{USAGE}"));
    };
    let mut sc = load_scenario(Path::new(&spec))?;
    if let Some(s) = seed {
        sc.seed = s;
    }
    let g = generate(&sc)?;
    let text = g.skeleton.to_json();
    let dir = Path::new(&out);
    let summary = format!(
        "{} notes, {} questions, seed {}, skeleton {}",
        g.skeleton.notes.len(),
        g.skeleton.questions.len(),
        sc.seed,
        skeleton_hash(&text)
    );
    // #821 FR-008: the graph question set is generated from the same skeleton, plus the
    // hand-maintained prose overlay (#907): absent means none, present-but-invalid is an error.
    let prose_path = Path::new(&out).join(graph_questions::PROSE_FACTS_FILE);
    let prose_text = match std::fs::read_to_string(&prose_path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", prose_path.display())),
    };
    let prose = match &prose_text {
        Some(t) => graph_questions::parse_prose_facts(t, &g.skeleton)?,
        None => Vec::new(),
    };
    // Once the notes are rendered, each cited note must actually name the entity and the value.
    let notes_dir = dir.join("notes");
    if notes_dir.is_dir() {
        graph_questions::check_prose_citations(&prose, &notes_dir)?;
    }
    let graph_text = graph_questions::to_jsonl(&graph_questions::generate(&g.skeleton, &prose)?);
    if check {
        let on_disk = std::fs::read_to_string(dir.join("skeleton.json")).unwrap_or_default();
        let mut failed = false;
        if on_disk != text {
            eprintln!("  drift: skeleton.json differs from the generator output for the scenario and seed");
            failed = true;
        }
        let graph_on_disk =
            std::fs::read_to_string(dir.join(graph_questions::FILE)).unwrap_or_default();
        if let Some(d) = first_line_diff(&graph_on_disk, &graph_text) {
            eprintln!(
                "  drift: {} differs from the generator output ({d})",
                graph_questions::FILE
            );
            failed = true;
        }
        // #907: the gold depends on prose-facts.json, so an edit must be a deliberate, versioned one.
        if let Ok(meta) = std::fs::read_to_string(dir.join("questions.meta.json")) {
            let recorded = serde_json::from_str::<serde_json::Value>(&meta)
                .ok()
                .and_then(|v| v["prose_facts_hash"].as_str().map(str::to_string));
            let current = prose_text.as_deref().map(graph_questions::prose_facts_hash);
            if recorded != current {
                eprintln!(
                    "  drift: {} ({}) does not match the prose_facts_hash in questions.meta.json ({}): it changes the graph-question gold, so bump `questions_version` in scenario.json, regenerate, and record the new hash",
                    graph_questions::PROSE_FACTS_FILE,
                    current.as_deref().unwrap_or("absent"),
                    recorded.as_deref().unwrap_or("none")
                );
                failed = true;
            }
        }
        if dir.join("render.json").exists() {
            let sk = Skeleton::from_json(&on_disk).unwrap_or_else(|_| g.skeleton.clone());
            let f = check_dir(dir, &sk, &on_disk)?;
            eprint!("{}", check::describe(&f));
            failed |= !f.errors.is_empty();
        } else {
            println!(
                "retrieval-gen --check: not rendered yet (no render.json); prose checks skipped"
            );
        }
        if failed {
            return Err("the fixture does not match the generator; see above".into());
        }
        println!(
            "retrieval-gen --check: {} matches ({summary})",
            dir.display()
        );
        return Ok(0);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(dir.join("skeleton.json"), &text).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(graph_questions::FILE), &graph_text).map_err(|e| e.to_string())?;
    let review = dir.join("review.json");
    if !review.exists() {
        std::fs::write(&review, review_template(&g, &text, sc.seed)).map_err(|e| e.to_string())?;
    }
    println!(
        "retrieval-gen: wrote skeleton.json and {} to {} ({summary})",
        graph_questions::FILE,
        dir.display()
    );
    Ok(0)
}

/// `None` when equal, else where the first difference is ("line N").
fn first_line_diff(a: &str, b: &str) -> Option<String> {
    if a == b {
        return None;
    }
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let n = la
        .iter()
        .zip(&lb)
        .position(|(x, y)| x != y)
        .unwrap_or(la.len().min(lb.len()));
    Some(format!(
        "first difference at line {}; {} lines on disk, {} generated",
        n + 1,
        la.len(),
        lb.len()
    ))
}

pub fn run_cli(args: &[String]) -> i32 {
    match run_gen(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lcg-eval retrieval-gen: {e}");
            if e.starts_with("unknown") || e.contains("requires a value") || e.contains("must be") {
                2
            } else {
                1
            }
        }
    }
}
