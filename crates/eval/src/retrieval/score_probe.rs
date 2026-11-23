//! `lcg-eval retrieval-score-probe` (#863, eval-only): how do the scores `knowledge_search_passages`
//! returns separate answerable queries from queries that have no answer in the corpus?
//!
//! This is a measurement instrument for the #848 question "is an absolute score floor (#812) safe
//! in hybrid mode?". It sends `min_score = 0` in every mode (so nothing is floored away before it
//! is measured), records the top-k `score` / `similarity` per query as JSONL, and summarises, per
//! query set and mode, the distribution of the top-1 score and of the best vector-leg cosine, the
//! separation (AUC, best single threshold) against the answerable set, and what a given floor would
//! do (answerable queries lost, irrelevant queries admitted).
//!
//! It adds nothing to the service: it reads the existing response. Consequently it cannot see the
//! raw BM25 value, and "lexical-only" is read as `similarity == None` (ADR-0789 §6).
//!
//! The statistics are pure functions with unit tests; the runner reuses [`super::lcg`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use super::fixture::{self, Category, Fixture, Question};
use super::lcg::{self, PassageMode, RpcTransport};
use super::mapping::map_passages;

/// Query sets, in report order.
pub const SET_ANSWERABLE: &str = "answerable";
pub const SET_HARD_NEGATIVE: &str = "hard-negative";
pub const SET_OFFTOPIC: &str = "offtopic";

/// Categories measured as "answerable": one relevant note (or paragraph) holds the answer.
pub const ANSWERABLE_CATEGORIES: [Category; 4] = [
    Category::Direct,
    Category::Paraphrase,
    Category::Answer,
    Category::Passage,
];

/// A query to probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeQuery {
    pub set: &'static str,
    /// Sub-kind inside the set (question category, or the off-topic `kind`).
    pub kind: String,
    pub id: String,
    pub text: String,
    /// Relevant note ids (empty for the irrelevant sets).
    pub relevant: Vec<String>,
}

/// One passage hit as the service returned it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub uuid: String,
    pub note: Option<String>,
    pub score: f64,
    pub similarity: Option<f64>,
}

/// What one (query, mode) returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub query: ProbeQuery,
    pub mode: String,
    pub hits: Vec<Hit>,
}

impl Observation {
    /// Score of the top-ranked hit; `None` when nothing came back.
    pub fn top1_score(&self) -> Option<f64> {
        self.hits.first().map(|h| h.score)
    }
    /// Best vector-leg cosine over the returned hits (the quantity #812's floor acts on).
    /// In vector mode `score` *is* the cosine and `similarity` is absent, so fall back to it.
    pub fn max_cosine(&self, vector_mode: bool) -> Option<f64> {
        self.hits
            .iter()
            .filter_map(|h| {
                if vector_mode {
                    Some(h.score)
                } else {
                    h.similarity
                }
            })
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
    }
    /// Hits with no vector-leg match: lexical-only, so never floored by #812 (hybrid mode only).
    pub fn lexical_only(&self) -> usize {
        self.hits.iter().filter(|h| h.similarity.is_none()).count()
    }
    /// Whether the top-1 hit's note is one of the query's relevant notes.
    pub fn top1_relevant(&self) -> Option<bool> {
        if self.query.relevant.is_empty() {
            return None;
        }
        Some(
            self.hits
                .first()
                .and_then(|h| h.note.as_ref())
                .is_some_and(|n| self.query.relevant.contains(n)),
        )
    }
}

// ── pure statistics ─────────────────────────────────────────────────────────────────────────

/// Linear-interpolated quantile of `xs` (`q` in 0..=1). `None` for an empty slice.
pub fn quantile(xs: &[f64], q: f64) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let pos = q.clamp(0.0, 1.0) * (v.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    Some(v[lo] + (v[hi] - v[lo]) * (pos - lo as f64))
}

/// Five-number-ish summary of a score sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Dist {
    pub n: usize,
    pub min: f64,
    pub p10: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

pub fn dist(xs: &[f64]) -> Option<Dist> {
    Some(Dist {
        n: xs.len(),
        min: quantile(xs, 0.0)?,
        p10: quantile(xs, 0.1)?,
        median: quantile(xs, 0.5)?,
        p90: quantile(xs, 0.9)?,
        max: quantile(xs, 1.0)?,
    })
}

/// Probability that a random positive scores above a random negative (ties count half): 1.0 is
/// perfect separation, 0.5 is none. `None` if either side is empty.
pub fn auc(pos: &[f64], neg: &[f64]) -> Option<f64> {
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut wins = 0.0;
    for p in pos {
        for n in neg {
            wins += match p.total_cmp(n) {
                std::cmp::Ordering::Greater => 1.0,
                std::cmp::Ordering::Equal => 0.5,
                std::cmp::Ordering::Less => 0.0,
            };
        }
    }
    Some(wins / (pos.len() * neg.len()) as f64)
}

/// The single threshold `t` (predict "answerable" when `score >= t`) maximising balanced accuracy.
#[derive(Debug, Clone, PartialEq)]
pub struct BestThreshold {
    pub threshold: f64,
    /// Fraction of positives with `score >= t`.
    pub tpr: f64,
    /// Fraction of negatives with `score >= t`.
    pub fpr: f64,
    pub balanced_accuracy: f64,
}

pub fn best_threshold(pos: &[f64], neg: &[f64]) -> Option<BestThreshold> {
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut cands: Vec<f64> = pos.iter().chain(neg.iter()).copied().collect();
    cands.sort_by(|a, b| a.total_cmp(b));
    cands.dedup();
    let mut best: Option<BestThreshold> = None;
    for t in cands {
        let (tpr, fpr) = rates_at(pos, neg, t);
        let ba = (tpr + (1.0 - fpr)) / 2.0;
        if best.as_ref().is_none_or(|b| ba > b.balanced_accuracy) {
            best = Some(BestThreshold {
                threshold: t,
                tpr,
                fpr,
                balanced_accuracy: ba,
            });
        }
    }
    best
}

/// `(fraction of pos >= t, fraction of neg >= t)`.
pub fn rates_at(pos: &[f64], neg: &[f64], t: f64) -> (f64, f64) {
    let frac = |xs: &[f64]| {
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().filter(|x| **x >= t).count() as f64 / xs.len() as f64
        }
    };
    (frac(pos), frac(neg))
}

/// Fraction of `pos` that score below the highest `neg` (0.0 = fully separable by one threshold).
pub fn overlap_fraction(pos: &[f64], neg: &[f64]) -> Option<f64> {
    let max_neg = quantile(neg, 1.0)?;
    if pos.is_empty() {
        return None;
    }
    Some(pos.iter().filter(|p| **p <= max_neg).count() as f64 / pos.len() as f64)
}

// ── query sets ──────────────────────────────────────────────────────────────────────────────

/// Parse `offtopic-queries.jsonl`: one `{"id","kind","text"}` object per non-empty line.
pub fn parse_offtopic(text: &str) -> Result<Vec<ProbeQuery>, String> {
    let mut out = Vec::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(line).map_err(|e| format!("off-topic line {}: {e}", i + 1))?;
        let field = |k: &str| {
            v[k].as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("off-topic line {}: missing \"{k}\"", i + 1))
        };
        out.push(ProbeQuery {
            set: SET_OFFTOPIC,
            kind: field("kind")?,
            id: field("id")?,
            text: field("text")?,
            relevant: Vec::new(),
        });
    }
    Ok(out)
}

fn category_name(c: Category) -> String {
    format!("{c:?}").to_lowercase()
}

/// Answerable + hard-negative queries from the fixture's own questions, then the off-topic set.
pub fn build_queries(fx: &Fixture, offtopic: Vec<ProbeQuery>) -> Vec<ProbeQuery> {
    let mut out = Vec::new();
    for q in &fx.questions {
        let set = if q.negative || q.category == Category::Negative {
            SET_HARD_NEGATIVE
        } else if ANSWERABLE_CATEGORIES.contains(&q.category) {
            SET_ANSWERABLE
        } else {
            continue;
        };
        out.push(probe_query(set, q));
    }
    out.extend(offtopic);
    out
}

fn probe_query(set: &'static str, q: &Question) -> ProbeQuery {
    ProbeQuery {
        set,
        kind: category_name(q.category),
        id: q.id.clone(),
        text: q.text.clone(),
        relevant: q.relevant.iter().map(|r| r.note.clone()).collect(),
    }
}

// ── summary ─────────────────────────────────────────────────────────────────────────────────

fn scores_of<'a>(
    obs: &'a [Observation],
    mode: &str,
    set: &str,
    f: impl Fn(&Observation) -> Option<f64> + 'a,
) -> Vec<f64> {
    obs.iter()
        .filter(|o| o.mode == mode && o.query.set == set)
        .filter_map(f)
        .collect()
}

fn dist_json(d: Option<Dist>) -> Value {
    match d {
        Some(d) => {
            json!({"n": d.n, "min": d.min, "p10": d.p10, "median": d.median, "p90": d.p90, "max": d.max})
        }
        None => Value::Null,
    }
}

fn sep_json(pos: &[f64], neg: &[f64], floors: &[f64]) -> Value {
    json!({
        "auc": auc(pos, neg),
        "overlap_fraction": overlap_fraction(pos, neg),
        "best_threshold": best_threshold(pos, neg).map(|b| json!({
            "threshold": b.threshold, "tpr": b.tpr, "fpr": b.fpr,
            "balanced_accuracy": b.balanced_accuracy})),
        "floors": floors.iter().map(|&t| {
            let (tpr, fpr) = rates_at(pos, neg, t);
            json!({"floor": t, "answerable_kept": tpr, "irrelevant_admitted": fpr})
        }).collect::<Vec<_>>(),
    })
}

/// A per-observation statistic; the flag says whether the mode is a vector mode.
type Stat = fn(&Observation, bool) -> Option<f64>;

/// The JSON summary: per mode, per statistic (`top1_score`, `max_cosine`), the distribution of each
/// set and its separation from the answerable set; plus lexical-leak counts and, in each mode, the
/// answerable top-1 correct-vs-wrong split.
pub fn summarize(obs: &[Observation], modes: &[String], floors: &[f64]) -> Value {
    let mut by_mode = serde_json::Map::new();
    for mode in modes {
        let vector_mode = mode.starts_with("vector");
        let stats: [(&str, Stat); 2] = [
            ("top1_score", |o, _| o.top1_score()),
            ("max_cosine", |o, vector| o.max_cosine(vector)),
        ];
        let mut per_stat = serde_json::Map::new();
        for (name, stat) in stats {
            let f = move |o: &Observation| stat(o, vector_mode);
            let pos = scores_of(obs, mode, SET_ANSWERABLE, f);
            let mut sets = serde_json::Map::new();
            for set in [SET_ANSWERABLE, SET_HARD_NEGATIVE, SET_OFFTOPIC] {
                let xs = scores_of(obs, mode, set, f);
                let mut entry = json!({"dist": dist_json(dist(&xs))});
                if set != SET_ANSWERABLE {
                    entry["vs_answerable"] = sep_json(&pos, &xs, floors);
                }
                sets.insert(set.to_string(), entry);
            }
            // Off-topic broken down by kind.
            let mut kinds = serde_json::Map::new();
            let mut ks: Vec<&str> = obs
                .iter()
                .filter(|o| o.mode == *mode && o.query.set == SET_OFFTOPIC)
                .map(|o| o.query.kind.as_str())
                .collect();
            ks.sort();
            ks.dedup();
            for k in ks {
                let xs: Vec<f64> = obs
                    .iter()
                    .filter(|o| o.mode == *mode && o.query.set == SET_OFFTOPIC && o.query.kind == k)
                    .filter_map(f)
                    .collect();
                kinds.insert(
                    k.to_string(),
                    json!({"dist": dist_json(dist(&xs)), "vs_answerable": sep_json(&pos, &xs, floors)}),
                );
            }
            sets.insert("offtopic_by_kind".into(), Value::Object(kinds));
            per_stat.insert(name.to_string(), Value::Object(sets));
        }
        // Answerable top-1 right vs wrong, on the top-1 score.
        let (mut right, mut wrong) = (Vec::new(), Vec::new());
        for o in obs
            .iter()
            .filter(|o| o.mode == *mode && o.query.set == SET_ANSWERABLE)
        {
            if let (Some(s), Some(r)) = (o.top1_score(), o.top1_relevant()) {
                if r {
                    right.push(s)
                } else {
                    wrong.push(s)
                }
            }
        }
        per_stat.insert(
            "answerable_top1_right_vs_wrong".into(),
            json!({"right": dist_json(dist(&right)), "wrong": dist_json(dist(&wrong)),
                   "auc_right_over_wrong": auc(&right, &wrong)}),
        );
        // BM25 leakage: lexical-only hits per query (hybrid modes; meaningless in vector mode).
        let mut leak = serde_json::Map::new();
        for set in [SET_ANSWERABLE, SET_HARD_NEGATIVE, SET_OFFTOPIC] {
            let rows: Vec<&Observation> = obs
                .iter()
                .filter(|o| o.mode == *mode && o.query.set == set)
                .collect();
            let n = rows.len().max(1) as f64;
            leak.insert(
                set.to_string(),
                json!({
                    "queries": rows.len(),
                    "queries_with_lexical_only_hit":
                        rows.iter().filter(|o| o.lexical_only() > 0).count(),
                    "mean_lexical_only_hits": rows.iter().map(|o| o.lexical_only()).sum::<usize>() as f64 / n,
                    "queries_returning_nothing": rows.iter().filter(|o| o.hits.is_empty()).count(),
                }),
            );
        }
        // `similarity` is never set in vector mode, so the count only means something in hybrid.
        per_stat.insert(
            "lexical_only_hits".into(),
            if vector_mode {
                Value::Null
            } else {
                Value::Object(leak)
            },
        );
        by_mode.insert(mode.clone(), Value::Object(per_stat));
    }
    Value::Object(by_mode)
}

/// One JSONL row per observation.
pub fn observation_json(o: &Observation) -> Value {
    json!({
        "set": o.query.set, "kind": o.query.kind, "id": o.query.id, "query": o.query.text,
        "mode": o.mode, "top1_relevant": o.top1_relevant(),
        "hits": o.hits.iter().map(|h| json!({
            "uuid": h.uuid, "note": h.note, "score": h.score, "similarity": h.similarity,
        })).collect::<Vec<_>>(),
    })
}

/// Parse a `knowledge_search_passages` response into hits.
pub fn parse_hits(resp: &Value) -> Vec<Hit> {
    let passages = resp["passages"].as_array().cloned().unwrap_or_default();
    passages
        .iter()
        .map(|p| {
            let (notes, _) = map_passages(std::slice::from_ref(p));
            Hit {
                uuid: p["uuid"].as_str().unwrap_or("").to_string(),
                note: notes.into_iter().next(),
                score: p["score"].as_f64().unwrap_or(f64::NAN),
                similarity: p["similarity"].as_f64(),
            }
        })
        .collect()
}

/// A mode name, optionally with a `@<floor>` suffix: `hybrid@0.5` runs hybrid with `min_score`
/// 0.5, i.e. what #812's default does to that mode.
fn mode_of(name: &str) -> Result<(PassageMode, Option<f64>), String> {
    let (base, floor) = match name.split_once('@') {
        Some((b, f)) => (
            b,
            Some(
                f.parse::<f64>()
                    .map_err(|e| format!("bad floor in '{name}': {e}"))?,
            ),
        ),
        None => (name, None),
    };
    let m = match base {
        "vector" => PassageMode::Raw,
        "hybrid" => PassageMode::Hybrid,
        "hybrid-belief" => PassageMode::HybridBelief,
        other => {
            return Err(format!(
                "unknown mode '{other}' (vector, hybrid, hybrid-belief; optionally @<floor>)"
            ))
        }
    };
    Ok((m, floor))
}

/// The `knowledge_search_passages` request for a mode name.
fn request_for(mode: &str, q: &str, k: usize) -> Result<Value, String> {
    let (m, floor) = mode_of(mode)?;
    let mut r = m.request(q, k);
    if let Some(f) = floor {
        r["min_score"] = json!(f);
    }
    Ok(r)
}

// ── CLI ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeArgs {
    pub corpus: PathBuf,
    pub service_bin: Option<PathBuf>,
    pub embedder_http: Option<String>,
    pub embedder_uds: Option<String>,
    /// Extraction endpoint. Passage search reads only Episodic nodes, so a stub that extracts
    /// nothing is enough; when set, no cassette is replayed by default.
    pub extractor_http: Option<String>,
    /// `LCG_EMBEDDING_PROFILE` (the record's standard setup uses `bge-v1.5`); `None` leaves the
    /// service default.
    pub embedding_profile: Option<String>,
    pub replay_cassette: Option<String>,
    /// `--ingest-concurrency N` (#900), default 1. Only takes effect with `--extractor-http`:
    /// a replayed cassette (explicit or the corpus default) is always ingested sequentially.
    pub ingest_concurrency: usize,
    pub offtopic: Option<PathBuf>,
    pub k: usize,
    pub modes: Vec<String>,
    pub floors: Vec<f64>,
    pub out: Option<PathBuf>,
    /// `<term>,<nonsense>` for the FTS any-term check (see `--fts-check`).
    pub fts_check: Option<(String, String)>,
}

pub fn usage() -> &'static str {
    "lcg-eval retrieval-score-probe — (#863, eval-only) score separation of answerable vs irrelevant queries

USAGE:
    lcg-eval retrieval-score-probe --embedder-uds <SOCK> | --embedder-http <URL> [OPTIONS]

OPTIONS:
    --corpus <DIR>           Fixture directory (default crates/eval/fixtures/retrieval-v2).
    --service-bin <PATH>     liminis-context-graph binary (default: next to this executable).
    --embedder-uds/-http     Real embedder for the spawned service (mock/hash are refused).
    --extractor-http <URL>   Extraction endpoint (OpenAI-compatible). Passage search needs no
                             entities, so an endpoint that extracts nothing is enough; giving one
                             disables the default cassette.
    --embedding-profile <P>  Embedding instruction profile (the record's standard setup: bge-v1.5).
    --replay-cassette <F>    Extraction cassette (default <corpus>/cassette.jsonl when present and no
                             --extractor-http). Recorded cassettes go stale when the extraction
                             prompt changes; a miss aborts the ingest.
    --ingest-concurrency <N> Keep up to N knowledge_process_chunk calls in flight (default 1).
                             Forced to 1 whenever a cassette is replayed, so it matters only
                             with --extractor-http.
    --offtopic <FILE>        Off-topic queries (default <corpus>/offtopic-queries.jsonl).
    --k <N>                  Hits per query (default 10).
    --modes <LIST>           Subset of vector,hybrid,hybrid-belief (default: all three). A mode may
                             carry a floor, e.g. hybrid@0.5 sends min_score=0.5 (what #812's default
                             does), to see what the floor leaves behind.
    --floors <LIST>          Floors to evaluate, comma-separated (default 0.5, the #812 default).
    --out <FILE>             Write every observation as JSONL (the summary goes to stdout).
    --fts-check <A>,<B>      Also count the FTS index's matches for 'A', 'B' and 'A B', with and
                             without `conjunctive := true` (does the FTS leg match any term or all
                             terms?). A is a term in the corpus, B a nonsense token."
}

pub fn parse_args(args: &[String]) -> Result<Option<ProbeArgs>, String> {
    let mut a = ProbeArgs {
        corpus: PathBuf::from("crates/eval/fixtures/retrieval-v2"),
        service_bin: None,
        embedder_http: None,
        embedder_uds: None,
        extractor_http: None,
        embedding_profile: None,
        replay_cassette: None,
        ingest_concurrency: 1,
        offtopic: None,
        k: 10,
        modes: vec!["vector".into(), "hybrid".into(), "hybrid-belief".into()],
        floors: vec![0.5],
        out: None,
        fts_check: None,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag == "-h" || flag == "--help" {
            return Ok(None);
        }
        let mut val = |f: &str| -> Result<String, String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{f} needs a value"))
        };
        match flag.as_str() {
            "--corpus" => a.corpus = PathBuf::from(val("--corpus")?),
            "--service-bin" => a.service_bin = Some(PathBuf::from(val("--service-bin")?)),
            "--embedder-http" => a.embedder_http = Some(val("--embedder-http")?),
            "--embedder-uds" => a.embedder_uds = Some(val("--embedder-uds")?),
            "--extractor-http" => a.extractor_http = Some(val("--extractor-http")?),
            "--embedding-profile" => a.embedding_profile = Some(val("--embedding-profile")?),
            "--replay-cassette" => a.replay_cassette = Some(val("--replay-cassette")?),
            "--ingest-concurrency" => {
                a.ingest_concurrency = lcg::parse_ingest_concurrency(&val("--ingest-concurrency")?)?
            }
            "--offtopic" => a.offtopic = Some(PathBuf::from(val("--offtopic")?)),
            "--k" => {
                a.k = val("--k")?
                    .parse()
                    .ok()
                    .filter(|k| (1..=100).contains(k))
                    .ok_or("--k must be 1..=100")?
            }
            "--modes" => {
                a.modes = val("--modes")?.split(',').map(str::to_string).collect();
                for m in &a.modes {
                    mode_of(m)?;
                }
            }
            "--floors" => {
                a.floors = val("--floors")?
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<f64>()
                            .map_err(|e| format!("--floors: {e}"))
                    })
                    .collect::<Result<_, _>>()?
            }
            "--out" => a.out = Some(PathBuf::from(val("--out")?)),
            "--fts-check" => {
                let v = val("--fts-check")?;
                let (x, y) = v
                    .split_once(',')
                    .ok_or("--fts-check wants <term>,<nonsense>")?;
                a.fts_check = Some((x.to_string(), y.to_string()));
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(Some(a))
}

pub async fn run_cli(args: &[String]) -> i32 {
    match parse_args(args) {
        Ok(None) => {
            println!("{}", usage());
            0
        }
        Ok(Some(a)) => match run(a).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("lcg-eval retrieval-score-probe: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("lcg-eval retrieval-score-probe: {e}");
            2
        }
    }
}

async fn run(a: ProbeArgs) -> Result<(), String> {
    let fx = fixture::load(&a.corpus)?;
    let off_path = a
        .offtopic
        .clone()
        .unwrap_or_else(|| a.corpus.join("offtopic-queries.jsonl"));
    let offtopic = match std::fs::read_to_string(&off_path) {
        Ok(t) => parse_offtopic(&t)?,
        Err(e) => {
            return Err(format!(
                "cannot read the off-topic query set {}: {e}",
                off_path.display()
            ))
        }
    };
    let queries = build_queries(&fx, offtopic);
    // The service runs in a temp workspace, so every path it is handed must be absolute.
    let absolute = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let cassette = a
        .replay_cassette
        .as_deref()
        .map(|c| absolute(Path::new(c)))
        .or_else(|| {
            let p = a.corpus.join("cassette.jsonl");
            (a.extractor_http.is_none() && p.exists()).then(|| absolute(&p))
        })
        .map(|p| p.display().to_string());
    let opts = lcg::ServiceOpts {
        bin: a
            .service_bin
            .as_deref()
            .map(absolute)
            .unwrap_or_else(super::default_service_bin),
        embedder_http: a.embedder_http.clone(),
        embedder_uds: a.embedder_uds.clone(),
        extractor_http: a.extractor_http.clone(),
        embedding_profile: a.embedding_profile.clone(),
        replay_cassette: cassette,
        ..Default::default()
    };
    eprintln!(
        "retrieval-score-probe: starting service {} ({} queries x {} modes)",
        opts.bin.display(),
        queries.len(),
        a.modes.len()
    );
    // #900: a cassette recorded at ingest concurrency > 1 is order-dependent; say so before the
    // service starts (replay itself always runs at concurrency 1).
    if let Some(c) = &opts.replay_cassette {
        // An unreadable sidecar is reported, not fatal: the probe never read it before.
        match super::workspace::order_dependent_cassette_warning(Path::new(c)) {
            Ok(Some(w)) => eprintln!("retrieval-score-probe: WARNING {w}"),
            Ok(None) => {}
            Err(e) => eprintln!("retrieval-score-probe: WARNING {e}"),
        }
    }
    let mut guard = lcg::spawn_service(&opts).await?;
    let t: Arc<dyn RpcTransport> = guard.client.clone();
    eprintln!("retrieval-score-probe: ingesting {} notes", fx.notes.len());
    let (concurrency, note) =
        lcg::effective_ingest_concurrency(a.ingest_concurrency, opts.replay_cassette.is_some());
    if let Some(note) = note {
        eprintln!("retrieval-score-probe: WARNING {note}");
    }
    let ingest_t = guard.ingest_transport(concurrency);
    let ingested = lcg::ingest_with(ingest_t.as_ref(), &fx, concurrency).await;
    drop(ingest_t);
    if let Err(e) = ingested {
        // #900: let the service finish any commit already started rather than SIGKILL it.
        guard.stop_gracefully().await;
        return Err(e);
    }

    let mut obs = Vec::new();
    for q in &queries {
        for m in &a.modes {
            let resp = t
                .call("knowledge_search_passages", request_for(m, &q.text, a.k)?)
                .await?;
            obs.push(Observation {
                query: q.clone(),
                mode: m.clone(),
                hits: parse_hits(&resp),
            });
        }
    }
    if let Some(out) = &a.out {
        write_jsonl(out, &obs)?;
    }
    let mut summary = json!({
        "service": guard.version,
        "embedder": a.embedder_http.clone().or(a.embedder_uds.clone()),
        "embedding_profile": a.embedding_profile,
        "k": a.k,
        "corpus": a.corpus.display().to_string(),
        "corpus_hash": fx.corpus_hash,
        "questions_hash": fx.questions_hash,
        "sets": {
            SET_ANSWERABLE: queries.iter().filter(|q| q.set == SET_ANSWERABLE).count(),
            SET_HARD_NEGATIVE: queries.iter().filter(|q| q.set == SET_HARD_NEGATIVE).count(),
            SET_OFFTOPIC: queries.iter().filter(|q| q.set == SET_OFFTOPIC).count(),
        },
        "floors": a.floors,
        "modes": summarize(&obs, &a.modes, &a.floors),
    });
    if let Some((term, junk)) = &a.fts_check {
        // Straight at the FTS index through the cypher escape hatch (read-only, throwaway
        // workspace), so the vector leg cannot mask the answer. Default call = what
        // `fts_search_episodic` sends; `conjunctive := true` is the explicit all-terms form.
        let mut rows = Vec::new();
        for q in [term.clone(), junk.clone(), format!("{term} {junk}")] {
            for conj in [false, true] {
                let opt = if conj { ", conjunctive := true" } else { "" };
                let cypher = format!(
                    "CALL QUERY_FTS_INDEX('Episodic', 'episode_content', '{}'{opt}) RETURN count(*)",
                    q.replace('\'', "")
                );
                let r = t
                    .call("knowledge_query_cypher", json!({ "query": cypher }))
                    .await;
                rows.push(json!({
                    "query": q, "conjunctive": conj,
                    "result": r.unwrap_or_else(|e| json!({"error": e})),
                }));
            }
        }
        summary["fts_check"] = Value::Array(rows);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn write_jsonl(path: &Path, obs: &[Observation]) -> Result<(), String> {
    let mut s = String::new();
    for o in obs {
        s.push_str(&serde_json::to_string(&observation_json(o)).map_err(|e| e.to_string())?);
        s.push('\n');
    }
    std::fs::write(path, s).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(set: &'static str, kind: &str, relevant: &[&str]) -> ProbeQuery {
        ProbeQuery {
            set,
            kind: kind.into(),
            id: "x".into(),
            text: "t".into(),
            relevant: relevant.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn hit(note: &str, score: f64, sim: Option<f64>) -> Hit {
        Hit {
            uuid: "u".into(),
            note: Some(note.into()),
            score,
            similarity: sim,
        }
    }

    #[test]
    fn quantile_interpolates() {
        assert_eq!(quantile(&[], 0.5), None);
        assert_eq!(quantile(&[4.0], 0.9), Some(4.0));
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 0.5), Some(2.5));
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 0.0), Some(1.0));
        assert_eq!(quantile(&[3.0, 1.0, 2.0], 1.0), Some(3.0));
    }

    #[test]
    fn auc_perfect_none_and_ties() {
        assert_eq!(auc(&[0.9, 0.8], &[0.1, 0.2]), Some(1.0));
        assert_eq!(auc(&[0.1], &[0.9]), Some(0.0));
        assert_eq!(auc(&[0.5], &[0.5]), Some(0.5));
        assert_eq!(auc(&[], &[0.5]), None);
    }

    #[test]
    fn best_threshold_separates_and_reports_rates() {
        let b = best_threshold(&[0.7, 0.8, 0.9], &[0.1, 0.2, 0.3]).unwrap();
        assert!((b.balanced_accuracy - 1.0).abs() < 1e-12);
        assert!(b.threshold > 0.3 && b.threshold <= 0.7);
        assert_eq!((b.tpr, b.fpr), (1.0, 0.0));
        // Heavy overlap: no threshold does better than chance-ish.
        let o = best_threshold(&[0.4, 0.6], &[0.5, 0.7]).unwrap();
        assert!(o.balanced_accuracy < 0.8);
    }

    #[test]
    fn overlap_fraction_counts_positives_at_or_below_the_max_negative() {
        assert_eq!(overlap_fraction(&[0.9, 0.8], &[0.1, 0.5]), Some(0.0));
        assert_eq!(overlap_fraction(&[0.4, 0.8], &[0.1, 0.5]), Some(0.5));
        assert_eq!(overlap_fraction(&[], &[0.5]), None);
    }

    #[test]
    fn rates_at_is_inclusive() {
        assert_eq!(rates_at(&[0.5, 0.6], &[0.5, 0.1], 0.5), (1.0, 0.5));
    }

    #[test]
    fn parse_offtopic_reads_jsonl_and_rejects_missing_fields() {
        let ok = "{\"id\":\"a\",\"kind\":\"gibberish\",\"text\":\"zz\"}\n\n";
        let v = parse_offtopic(ok).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].set, v[0].kind.as_str()), (SET_OFFTOPIC, "gibberish"));
        assert!(parse_offtopic("{\"id\":\"a\",\"text\":\"zz\"}").is_err());
        assert!(parse_offtopic("not json").is_err());
    }

    #[test]
    fn shipped_offtopic_fixture_parses_with_unique_ids() {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/retrieval-v2/offtopic-queries.jsonl"
        );
        let v = parse_offtopic(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert!(v.len() >= 30);
        let mut ids: Vec<_> = v.iter().map(|q| q.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), v.len());
    }

    #[test]
    fn observation_accessors() {
        let o = Observation {
            query: q(SET_ANSWERABLE, "direct", &["n1"]),
            mode: "hybrid".into(),
            hits: vec![hit("n1", 0.02, Some(0.8)), hit("n2", 0.01, None)],
        };
        assert_eq!(o.top1_score(), Some(0.02));
        assert_eq!(o.max_cosine(false), Some(0.8));
        assert_eq!(o.max_cosine(true), Some(0.02));
        assert_eq!(o.lexical_only(), 1);
        assert_eq!(o.top1_relevant(), Some(true));
        let irrelevant = Observation {
            query: q(SET_OFFTOPIC, "gibberish", &[]),
            ..o.clone()
        };
        assert_eq!(irrelevant.top1_relevant(), None);
        let empty = Observation { hits: vec![], ..o };
        assert_eq!((empty.top1_score(), empty.max_cosine(false)), (None, None));
        assert_eq!(empty.top1_relevant(), Some(false));
    }

    #[test]
    fn parse_hits_maps_scores_similarity_and_note() {
        let resp = json!({"passages": [
            {"uuid": "a", "content": "c", "source_description": "alpha:chunk-1", "score": 0.7},
            {"uuid": "b", "content": "c", "source_description": "", "score": 0.02, "similarity": 0.61}
        ]});
        let h = parse_hits(&resp);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].score, h[0].similarity), (0.7, None));
        assert_eq!(h[0].note.as_deref(), Some("alpha"));
        assert_eq!(h[1].similarity, Some(0.61));
        assert_eq!(parse_hits(&json!({})).len(), 0);
    }

    #[test]
    fn summarize_separates_sets_and_reports_floor_effects() {
        let mk = |set: &'static str, kind: &str, rel: &[&str], top: &str, s: f64| Observation {
            query: q(set, kind, rel),
            mode: "vector".into(),
            hits: vec![hit(top, s, None)],
        };
        let obs = vec![
            mk(SET_ANSWERABLE, "direct", &["a"], "a", 0.8),
            mk(SET_ANSWERABLE, "direct", &["a"], "b", 0.6),
            mk(SET_HARD_NEGATIVE, "negative", &[], "a", 0.55),
            mk(SET_OFFTOPIC, "gibberish", &[], "a", 0.3),
            mk(SET_OFFTOPIC, "common-words", &[], "a", 0.4),
        ];
        let s = summarize(&obs, &["vector".to_string()], &[0.5]);
        let off = &s["vector"]["top1_score"]["offtopic"]["vs_answerable"];
        assert_eq!(off["auc"], json!(1.0));
        assert_eq!(off["overlap_fraction"], json!(0.0));
        assert_eq!(off["floors"][0]["answerable_kept"], json!(1.0));
        assert_eq!(off["floors"][0]["irrelevant_admitted"], json!(0.0));
        let hard = &s["vector"]["top1_score"]["hard-negative"]["vs_answerable"];
        assert_eq!(hard["floors"][0]["irrelevant_admitted"], json!(1.0));
        assert!(s["vector"]["top1_score"]["offtopic_by_kind"]["gibberish"].is_object());
        assert_eq!(
            s["vector"]["answerable_top1_right_vs_wrong"]["right"]["n"],
            json!(1)
        );
    }

    #[test]
    fn parse_args_defaults_and_errors() {
        let a = parse_args(&[]).unwrap().unwrap();
        assert_eq!((a.k, a.modes.len(), a.floors.clone()), (10, 3, vec![0.5]));
        assert!(parse_args(&["--help".into()]).unwrap().is_none());
        assert!(parse_args(&["--modes".into(), "nope".into()]).is_err());
        assert!(parse_args(&["--k".into(), "0".into()]).is_err());
        assert!(parse_args(&["--bogus".into()]).is_err());
        assert_eq!(a.ingest_concurrency, 1);
        let c = parse_args(&["--ingest-concurrency".into(), "4".into()])
            .unwrap()
            .unwrap();
        assert_eq!(c.ingest_concurrency, 4);
        for bad in ["0", "-2", "x"] {
            assert!(parse_args(&["--ingest-concurrency".into(), bad.into()])
                .unwrap_err()
                .contains("--ingest-concurrency"));
        }
        let f = parse_args(&["--fts-check".into(), "cedar,zzqx".into()])
            .unwrap()
            .unwrap();
        assert_eq!(f.fts_check, Some(("cedar".into(), "zzqx".into())));
    }

    #[test]
    fn mode_names_carry_an_optional_floor() {
        assert_eq!(mode_of("hybrid").unwrap(), (PassageMode::Hybrid, None));
        assert_eq!(
            mode_of("vector@0.5").unwrap(),
            (PassageMode::Raw, Some(0.5))
        );
        assert!(mode_of("hybrid@x").is_err());
        assert!(mode_of("rerank").is_err());
        let r = request_for("hybrid@0.5", "q", 7).unwrap();
        assert_eq!(
            (r["min_score"].as_f64(), r["mode"].as_str()),
            (Some(0.5), Some("hybrid"))
        );
        assert_eq!(
            request_for("hybrid", "q", 7).unwrap()["min_score"],
            json!(0.0)
        );
    }
}
