//! Deterministic generator for the retrieval-v2 fixture (#788, ADR-0788).
//!
//! `scenario.json` (quotas, seed) plus the vocabulary in [`vocab`] fully determine every byte of
//! `manifest.json`, `notes/*.md`, `questions.jsonl` and `questions.meta.json`: no network, no
//! model calls, no clock, no hash-map iteration. `lcg-eval retrieval-gen --check` regenerates in
//! memory and reports any drift from the checked-in files; a Rust test does the same.
//!
//! The generator builds a fact model first ([`world`]), renders notes from it ([`notes`]) and
//! derives questions and ground truth from the same facts ([`questions`]), so evidence, negatives
//! and currency are correct by construction. The output is then re-validated through the
//! production loader.

pub mod notes;
pub mod questions;
pub mod vocab;
pub mod world;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use self::notes::GenNote;
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

/// Everything the generator writes, keyed by path relative to the fixture directory.
#[derive(Debug, Clone)]
pub struct Output {
    pub files: BTreeMap<String, String>,
    /// Pending human-review record; written only when `review.json` does not exist yet.
    pub review: String,
    pub notes: usize,
    pub questions: usize,
}

#[derive(Serialize)]
struct ManNote<'a> {
    id: &'a str,
    reference_time: String,
    origin: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cites: &'a Vec<String>,
}
#[derive(Serialize)]
struct ManSup<'a> {
    old_note: &'a str,
    new_note: &'a str,
    effective_at: String,
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
#[derive(Serialize)]
struct Meta<'a> {
    questions_version: &'a str,
    generator: &'a str,
    seed: u64,
}
#[derive(Serialize)]
struct ReviewItem {
    id: String,
    category: String,
    ground_truth_correct: Option<bool>,
    note_relevance_correct: Option<bool>,
    key_fact_correct: Option<bool>,
}
#[derive(Serialize)]
struct Review<'a> {
    status: &'a str,
    questions_version: &'a str,
    questions_hash: String,
    sample_policy: String,
    reviewer: Option<String>,
    reviewed_on: Option<String>,
    error_rate_threshold: f64,
    error_rate: Option<f64>,
    sample: Vec<ReviewItem>,
}

pub fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read scenario {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn pretty<T: Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("serializable");
    s.push('\n');
    s
}

/// Generates the whole fixture in memory.
pub fn generate(sc: &Scenario) -> Result<Output, String> {
    let root = Rng::new(sc.seed);
    let mut w = World::build(sc, &root)?;
    let corpus = notes::build(sc, &mut w, &root)?;

    // Manifest order = ingest order: by recording date, creation order breaking ties. A derived
    // or answer note is always dated after the notes it cites, so citations always point earlier.
    let mut ordered: Vec<&GenNote> = corpus.notes.iter().collect();
    ordered.sort_by_key(|n| (n.day, n.seq));
    let bodies: BTreeMap<String, String> = corpus
        .notes
        .iter()
        .map(|n| (n.id.clone(), n.body.clone()))
        .collect();

    let manifest = Man {
        corpus_version: &sc.corpus_version,
        description: &sc.description,
        chunking: Chunking {
            max_tokens: sc.chunk_max_tokens,
        },
        notes: ordered
            .iter()
            .map(|n| ManNote {
                id: &n.id,
                reference_time: rfc3339(n.day),
                origin: n.origin,
                cites: &n.cites,
            })
            .collect(),
        supersedes: corpus
            .supersedes
            .iter()
            .map(|(old, new, day)| ManSup {
                old_note: old,
                new_note: new,
                effective_at: rfc3339(*day),
            })
            .collect(),
    };
    let manifest_text = pretty(&manifest);

    let qs = questions::build(sc, &w, &corpus, &bodies, &root)?;
    let mut qtext = String::new();
    for q in &qs {
        qtext.push_str(&serde_json::to_string(q).expect("serializable"));
        qtext.push('\n');
    }
    let meta_text = pretty(&Meta {
        questions_version: &sc.questions_version,
        generator: "lcg-eval retrieval-gen",
        seed: sc.seed,
    });

    let mut files = BTreeMap::new();
    files.insert("manifest.json".to_string(), manifest_text);
    files.insert("questions.jsonl".to_string(), qtext.clone());
    files.insert("questions.meta.json".to_string(), meta_text);
    for n in &corpus.notes {
        files.insert(format!("notes/{}.md", n.id), n.body.clone());
    }

    // Re-validate through the production loader, then the checks only the generator can make.
    let m = fixture::parse_manifest(files["manifest.json"].as_bytes())
        .map_err(|e| format!("generated manifest is invalid: {e}"))?;
    let known: HashSet<&str> = m.notes.iter().map(|n| n.id.as_str()).collect();
    fixture::parse_questions(&qtext, &known)
        .map_err(|e| format!("generated questions are invalid: {e}"))?;
    for q in &qs {
        for e in &q.evidence {
            if !bodies[&e.note].contains(&e.key_fact) {
                return Err(format!(
                    "question '{}': key fact is not stated verbatim in note '{}'",
                    q.id, e.note
                ));
            }
        }
    }

    let review = review_skeleton(sc, &qs, &fixture::questions_hash_of(&qtext), &root);
    Ok(Output {
        notes: corpus.notes.len(),
        questions: qs.len(),
        files,
        review,
    })
}

/// A pending review record with a seeded, stratified sample (never overwrites a filled record).
fn review_skeleton(sc: &Scenario, qs: &[questions::QOut], hash: &str, root: &Rng) -> String {
    let mut by_cat: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for q in qs {
        by_cat.entry(q.category).or_default().push(&q.id);
    }
    let mut sample = Vec::new();
    for (cat, mut ids) in by_cat {
        root.fork(&format!("review|{cat}")).shuffle(&mut ids);
        let mut picked: Vec<&str> = ids.into_iter().take(sc.review_per_category).collect();
        picked.sort();
        for id in picked {
            sample.push(ReviewItem {
                id: id.to_string(),
                category: cat.to_string(),
                ground_truth_correct: None,
                note_relevance_correct: None,
                key_fact_correct: None,
            });
        }
    }
    pretty(&Review {
        status: "pending",
        questions_version: &sc.questions_version,
        questions_hash: hash.to_string(),
        sample_policy: format!(
            "stratified: {} questions per category (seeded sample, at least 10% of all questions); a maintainer fills every verdict and sets status to \"reviewed\"",
            sc.review_per_category
        ),
        reviewer: None,
        reviewed_on: None,
        error_rate_threshold: 0.05,
        error_rate: None,
        sample,
    })
}

/// Writes `out` into `dir`, removing notes the generator no longer produces. `review.json` is
/// written only if absent.
pub fn write_output(dir: &Path, out: &Output) -> Result<(), String> {
    let notes_dir = dir.join("notes");
    std::fs::create_dir_all(&notes_dir).map_err(|e| format!("{}: {e}", notes_dir.display()))?;
    let wanted: BTreeSet<&str> = out.files.keys().map(String::as_str).collect();
    if let Ok(rd) = std::fs::read_dir(&notes_dir) {
        for e in rd.flatten() {
            let rel = format!("notes/{}", e.file_name().to_string_lossy());
            if e.path().extension().is_some_and(|x| x == "md") && !wanted.contains(rel.as_str()) {
                std::fs::remove_file(e.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    for (rel, text) in &out.files {
        let p = dir.join(rel);
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let review = dir.join("review.json");
    if !review.exists() {
        std::fs::write(&review, &out.review).map_err(|e| format!("{}: {e}", review.display()))?;
    }
    Ok(())
}

/// Differences between `out` and what is on disk in `dir` (empty = no drift).
pub fn check_output(dir: &Path, out: &Output) -> Vec<String> {
    let mut drift = Vec::new();
    for (rel, text) in &out.files {
        match std::fs::read_to_string(dir.join(rel)) {
            Ok(on_disk) if &on_disk == text => {}
            Ok(_) => drift.push(format!("{rel}: differs from the generator output")),
            Err(_) => drift.push(format!("{rel}: missing on disk")),
        }
    }
    if let Ok(rd) = std::fs::read_dir(dir.join("notes")) {
        for e in rd.flatten() {
            let rel = format!("notes/{}", e.file_name().to_string_lossy());
            if !out.files.contains_key(&rel) {
                drift.push(format!("{rel}: on disk but not generated"));
            }
        }
    }
    drift.sort();
    drift
}

pub const USAGE: &str = "lcg-eval retrieval-gen — regenerate (or check) the retrieval-v2 fixture

USAGE:
    lcg-eval retrieval-gen --spec <scenario.json> --out <DIR> [--check] [--seed <N>]

OPTIONS:
    --spec <FILE>   Scenario (quotas and seed), e.g. crates/eval/fixtures/retrieval-v2/scenario.json
    --out <DIR>     Fixture directory to write (or compare against with --check)
    --check         Regenerate in memory and exit 1 if the files in --out differ (writes nothing)
    --seed <N>      Override the scenario seed (e.g. to produce a different valid fixture elsewhere)
    -h, --help      Print this help

Generation is deterministic: the same scenario and seed always produce byte-identical output, with
no network access or model calls. review.json is written only when absent and is never overwritten.
";

pub fn run_cli(args: &[String]) -> i32 {
    let mut spec = None;
    let mut out = None;
    let mut check = false;
    let mut seed = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            "--check" => check = true,
            flag @ ("--spec" | "--out" | "--seed") => {
                i += 1;
                let Some(v) = args.get(i) else {
                    eprintln!("lcg-eval retrieval-gen: {flag} requires a value");
                    return 2;
                };
                match flag {
                    "--spec" => spec = Some(v.clone()),
                    "--out" => out = Some(v.clone()),
                    _ => match v.parse::<u64>() {
                        Ok(n) => seed = Some(n),
                        Err(_) => {
                            eprintln!("lcg-eval retrieval-gen: --seed must be an unsigned integer");
                            return 2;
                        }
                    },
                }
            }
            other => {
                eprintln!("lcg-eval retrieval-gen: unknown argument '{other}'");
                return 2;
            }
        }
        i += 1;
    }
    let (Some(spec), Some(out)) = (spec, out) else {
        eprintln!("lcg-eval retrieval-gen: --spec and --out are required\n\n{USAGE}");
        return 2;
    };
    let result = (|| -> Result<(), String> {
        let mut sc = load_scenario(Path::new(&spec))?;
        if let Some(s) = seed {
            sc.seed = s;
        }
        let o = generate(&sc)?;
        let dir = Path::new(&out);
        if check {
            let drift = check_output(dir, &o);
            if drift.is_empty() {
                println!(
                    "retrieval-gen --check: {} matches the generator ({} notes, {} questions, seed {})",
                    dir.display(),
                    o.notes,
                    o.questions,
                    sc.seed
                );
                return Ok(());
            }
            for d in drift.iter().take(25) {
                eprintln!("  drift: {d}");
            }
            return Err(format!(
                "{} file(s) differ from the generator output; rerun without --check to regenerate",
                drift.len()
            ));
        }
        write_output(dir, &o)?;
        println!(
            "retrieval-gen: wrote {} notes and {} questions to {} (seed {})",
            o.notes,
            o.questions,
            dir.display(),
            sc.seed
        );
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lcg-eval retrieval-gen: {e}");
            1
        }
    }
}
