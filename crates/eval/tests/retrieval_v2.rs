//! Network-free coverage of the retrieval-v2 fixture (#788): loading and shape (SC-001), the
//! generator's determinism and drift check (SC-002), the human-review record (SC-003), temporal
//! ground truth without recorded supersession, origin submission at ingest, and an end-to-end run
//! that exercises confidence intervals and the per-origin report. Nothing here needs qmd, a model
//! or a network.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use lcg_eval::retrieval::evidence_judge::{CachedEvidenceJudge, EvidenceJudge};
use lcg_eval::retrieval::fixture::{self, Category, Fixture};
use lcg_eval::retrieval::generator::questions::{
    content_words, overlap, PARAPHRASE_MAX_NOTE_OVERLAP,
};
use lcg_eval::retrieval::generator::{self, Scenario};
use lcg_eval::retrieval::lcg::{self, LcgPassages, RpcTransport};
use lcg_eval::retrieval::mapping::chunk_markdown;
use lcg_eval::retrieval::report::{Bootstrap, IngestCost, Report, RunMeta, Status};
use lcg_eval::retrieval::{evaluate, resolve, RankedResult, Retriever, Slot};

const K: usize = 10;

/// (id, text, words) of an indexed chunk or note.
type Doc = (String, String, BTreeSet<String>);

fn v2_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval-v2")
}

fn load() -> Fixture {
    fixture::load(&v2_dir()).expect("retrieval-v2 must load with zero validation errors")
}

fn count_by_category(fx: &Fixture) -> HashMap<Category, usize> {
    let mut by = HashMap::new();
    for q in &fx.questions {
        *by.entry(q.category).or_default() += 1;
    }
    by
}

// ── SC-001: shape ────────────────────────────────────────────────────────────────

#[test]
fn v2_loads_and_has_the_required_scale_and_mix() {
    let fx = load();
    let st = fx.stats();
    assert!((300..=500).contains(&st.notes), "notes: {}", st.notes);
    assert!(
        fx.questions.len() >= 200,
        "questions: {}",
        fx.questions.len()
    );
    assert!(fx.questions.iter().filter(|q| q.negative).count() >= 30);

    // Size mix: many short notes, a substantial 500–2,000-word group, a few over 5,000 words.
    assert!(st.size_bands[0] >= 150, "short notes: {:?}", st.size_bands);
    assert!(
        st.size_bands[2] >= 50,
        "500-2000 word notes: {:?}",
        st.size_bands
    );
    assert!(st.over_5000_words >= 3, "{}", st.over_5000_words);
    // Long notes really span several chunks, so passage choice is exercised.
    assert!(
        st.multi_chunk_notes >= 50,
        "multi-chunk notes: {}",
        st.multi_chunk_notes
    );

    // Every category is present with a documented minimum of 30 questions.
    let by = count_by_category(&fx);
    for c in Category::EVERY {
        assert!(
            by.get(&c).copied().unwrap_or(0) >= 30,
            "{} has fewer than 30 questions: {by:?}",
            c.label()
        );
    }
    assert_eq!(fx.manifest.corpus_version, "meridian-notebook-2");
    assert_eq!(fx.questions_version, "meridian-questions-2");
}

#[test]
fn v2_has_all_four_origins_primary_largest_and_derived_pages_cite_primary_notes() {
    let fx = load();
    let st = fx.stats();
    for o in fixture::ORIGINS {
        assert!(
            st.origins.contains_key(o),
            "missing origin {o}: {:?}",
            st.origins
        );
    }
    let primary = st.origins["primary"];
    assert!(
        st.origins.values().all(|n| *n <= primary),
        "{:?}",
        st.origins
    );
    assert!(
        st.derived_citing_primary >= 30,
        "{}",
        st.derived_citing_primary
    );
    let derived: Vec<_> = fx
        .manifest
        .notes
        .iter()
        .filter(|n| n.origin.as_deref() == Some("derived"))
        .collect();
    assert!(derived.iter().all(|n| !n.cites.is_empty()));
    // Filed answers cite the notes they answer from; clippings and primary notes cite nothing.
    for n in &fx.manifest.notes {
        match n.origin.as_deref() {
            Some("primary") | Some("external") => assert!(n.cites.is_empty(), "{}", n.id),
            _ => {}
        }
    }
}

#[test]
fn near_duplicates_exist_and_ground_truth_names_which_note_is_relevant() {
    let fx = load();
    // A recap (primary near-duplicate) is relevant at gain 1 beside its source at gain 2; the
    // derived restatements of the same fact are never relevant.
    let mut recap_questions = 0;
    for q in &fx.questions {
        for r in &q.relevant {
            let origin = fx
                .manifest
                .notes
                .iter()
                .find(|n| n.id == r.note)
                .and_then(|n| n.origin.as_deref());
            assert_ne!(
                origin,
                Some("derived"),
                "{}: a derived page is relevant",
                q.id
            );
            if r.note.starts_with("recap-") {
                recap_questions += 1;
                assert_eq!(r.gain, 1, "{}", q.id);
                assert!(q.relevant.iter().any(|o| o.gain == 2), "{}", q.id);
            }
        }
    }
    assert!(recap_questions >= 10, "{recap_questions}");
}

// ── temporal realism (US4, FR-006, FR-013) ───────────────────────────────────────

#[test]
fn decisions_are_revised_three_or_more_times_with_explicit_and_implicit_steps() {
    let fx = load();
    let mut chains: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in &fx.manifest.notes {
        if let Some(rest) = n.id.strip_prefix("decision-") {
            let (chain, _step) = rest.rsplit_once('-').unwrap();
            chains
                .entry(chain.to_string())
                .or_default()
                .push(n.id.clone());
        }
    }
    assert!(chains.len() >= 20, "{}", chains.len());
    assert!(
        chains.values().all(|c| c.len() >= 3),
        "every chain is revised ≥ 3 times"
    );
    assert!(chains.values().any(|c| c.len() >= 5));

    let explicit: BTreeSet<(&str, &str)> = fx
        .manifest
        .supersedes
        .iter()
        .map(|s| (s.old_note.as_str(), s.new_note.as_str()))
        .collect();
    // Consecutive steps with no recorded supersession are the implicit contradictions.
    let mut implicit_pairs = 0;
    let mut explicit_pairs = 0;
    for steps in chains.values() {
        for w in steps.windows(2) {
            if explicit.contains(&(w[0].as_str(), w[1].as_str())) {
                explicit_pairs += 1;
            } else {
                implicit_pairs += 1;
            }
        }
    }
    assert!(
        explicit_pairs >= 20 && implicit_pairs >= 20,
        "{explicit_pairs}/{implicit_pairs}"
    );

    // Effective dates differ from recording dates for some revisions (FR-006, US4 scenario 3).
    let rt: HashMap<&str, &str> = fx
        .manifest
        .notes
        .iter()
        .filter_map(|n| n.reference_time.as_deref().map(|t| (n.id.as_str(), t)))
        .collect();
    let differing = fx
        .manifest
        .supersedes
        .iter()
        .filter(|s| rt.get(s.new_note.as_str()) != Some(&s.effective_at.as_str()))
        .count();
    assert!(
        differing >= 10,
        "{differing} supersessions with a distinct effective date"
    );
}

#[test]
fn temporal_questions_declare_current_and_stale_without_needing_a_supersession() {
    let fx = load();
    let explicit: BTreeSet<(&str, &str)> = fx
        .manifest
        .supersedes
        .iter()
        .map(|s| (s.old_note.as_str(), s.new_note.as_str()))
        .collect();
    let (mut with, mut without) = (0, 0);
    for q in fx
        .questions
        .iter()
        .filter(|q| q.category == Category::Temporal)
    {
        assert!(!q.current.is_empty() && !q.stale.is_empty(), "{}", q.id);
        for c in &q.current {
            assert!(q.relevant.iter().any(|r| &r.note == c), "{}", q.id);
            // Each stale note is an earlier step of the same decision chain.
            let prefix = &c[..c.rfind('-').unwrap()];
            for s in &q.stale {
                assert!(s.starts_with(prefix) && s != c, "{}: {s} vs {c}", q.id);
                if explicit.contains(&(s.as_str(), c.as_str())) {
                    with += 1;
                } else {
                    without += 1;
                }
            }
        }
    }
    assert!(
        with > 0 && without > 0,
        "explicit {with}, implicit {without}"
    );
}

// ── question rules (FR-010, FR-011, FR-012) ──────────────────────────────────────

#[test]
fn evidence_is_stated_in_its_note_and_passage_facts_sit_beyond_the_first_chunk() {
    let fx = load();
    let max = fx.manifest.max_chunk_tokens;
    let mut passages = 0;
    for q in &fx.questions {
        for e in &q.evidence {
            assert!(
                fx.notes[&e.note].contains(&e.key_fact),
                "{}: key fact not verbatim in {}",
                q.id,
                e.note
            );
        }
        match q.category {
            Category::Passage => {
                passages += 1;
                assert_eq!(q.evidence.len(), 1, "{}", q.id);
                let body = &fx.notes[&q.evidence[0].note];
                let chunks = chunk_markdown(body, max);
                assert!(chunks.len() >= 2, "{}: note fits in one chunk", q.id);
                let at = chunks
                    .iter()
                    .position(|c| c.contains(&q.evidence[0].key_fact))
                    .expect("fact is inside one chunk");
                assert!(at >= 1, "{}: the answer is in the first chunk", q.id);
            }
            Category::MultiHop => assert!(q.relevant.len() >= 2, "{}", q.id),
            Category::Answer => assert!(!q.evidence.is_empty(), "{}", q.id),
            _ => {}
        }
    }
    assert!(passages >= 30);
}

#[test]
fn paraphrase_questions_stay_below_the_lexical_overlap_bound() {
    let fx = load();
    let mut n = 0;
    for q in fx
        .questions
        .iter()
        .filter(|q| q.category == Category::Paraphrase)
    {
        n += 1;
        for r in &q.relevant {
            let o = overlap(&q.text, &fx.notes[&r.note]);
            assert!(
                o <= PARAPHRASE_MAX_NOTE_OVERLAP,
                "{}: {o:.2} of its content words appear in {}",
                q.id,
                r.note
            );
        }
        assert!(content_words(&q.text).len() >= 3, "{}", q.id);
    }
    assert!(n >= 30);
}

#[test]
fn every_negative_has_a_near_miss_distractor_and_no_relevant_note() {
    let fx = load();
    let mut n = 0;
    for q in fx.questions.iter().filter(|q| q.negative) {
        n += 1;
        assert_eq!(q.category, Category::Negative, "{}", q.id);
        assert!(q.relevant.is_empty() && q.evidence.is_empty(), "{}", q.id);
        let best = fx
            .notes
            .values()
            .map(|b| overlap(&q.text, b))
            .fold(0.0_f64, f64::max);
        assert!(
            best >= 0.33,
            "{}: no near-miss note (best overlap {best:.2})",
            q.id
        );
    }
    assert!(n >= 30);
}

// ── SC-002: generator determinism and drift ──────────────────────────────────────

fn scenario() -> Scenario {
    generator::load_scenario(&v2_dir().join("scenario.json")).unwrap()
}

#[test]
fn checked_in_fixture_equals_the_generator_output() {
    let out = generator::generate(&scenario()).unwrap();
    let drift = generator::check_output(&v2_dir(), &out);
    assert!(
        drift.is_empty(),
        "retrieval-v2 drifted from scenario.json; run `lcg-eval retrieval-gen --spec crates/eval/fixtures/retrieval-v2/scenario.json --out crates/eval/fixtures/retrieval-v2`:\n{}",
        drift.join("\n")
    );
}

#[test]
fn generation_is_byte_identical_for_the_same_seed_and_differs_for_another() {
    let sc = scenario();
    let a = generator::generate(&sc).unwrap();
    let b = generator::generate(&sc).unwrap();
    assert_eq!(a.files, b.files);
    assert_eq!(a.review, b.review);
    let other = Scenario {
        seed: sc.seed + 1,
        ..sc
    };
    // A different seed still yields a valid fixture (generate re-validates through the loader).
    let c = generator::generate(&other).unwrap();
    assert_ne!(a.files["questions.jsonl"], c.files["questions.jsonl"]);
    assert_ne!(a.files["manifest.json"], c.files["manifest.json"]);
    assert!((300..=500).contains(&c.notes) && c.questions >= 200);
}

#[test]
fn scenario_and_review_files_do_not_change_the_fixture_hashes() {
    let fx = load();
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("notes")).unwrap();
    std::fs::write(
        d.path().join("manifest.json"),
        r#"{"corpus_version":"v","notes":[{"id":"a"}]}"#,
    )
    .unwrap();
    std::fs::write(d.path().join("notes/a.md"), "# A\nbody").unwrap();
    std::fs::write(
        d.path().join("questions.jsonl"),
        r#"{"id":"q","category":"direct","text":"t","relevant_notes":[{"note":"a"}]}"#,
    )
    .unwrap();
    let before = fixture::load(d.path()).unwrap();
    std::fs::write(d.path().join("scenario.json"), "{}").unwrap();
    std::fs::write(d.path().join("review.json"), "{}").unwrap();
    let after = fixture::load(d.path()).unwrap();
    assert_eq!(before.corpus_hash, after.corpus_hash);
    assert_eq!(before.questions_hash, after.questions_hash);
    // And the checked-in v2 hashes are what the review record names (next test).
    assert_eq!(fx.questions_hash.len(), 16);
}

// ── SC-003: human-review record ──────────────────────────────────────────────────

#[test]
fn review_record_is_consistent_with_the_question_set() {
    let fx = load();
    let text =
        std::fs::read_to_string(v2_dir().join("review.json")).expect("review.json is checked in");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        r["questions_hash"], fx.questions_hash,
        "questions changed since the review record was written: re-review and update it"
    );
    assert_eq!(r["questions_version"], fx.questions_version.as_str());
    let status = r["status"].as_str().unwrap();
    assert!(status == "pending" || status == "reviewed", "{status}");

    let sample = r["sample"].as_array().unwrap();
    let ids: BTreeSet<&str> = fx.questions.iter().map(|q| q.id.as_str()).collect();
    let mut per_cat: HashMap<&str, usize> = HashMap::new();
    for s in sample {
        let id = s["id"].as_str().unwrap();
        assert!(ids.contains(id), "sampled question {id} does not exist");
        let q = fx.questions.iter().find(|q| q.id == id).unwrap();
        assert_eq!(s["category"], q.category.label());
        *per_cat.entry(q.category.label()).or_default() += 1;
    }
    for c in Category::EVERY {
        assert!(
            per_cat.get(c.label()).copied().unwrap_or(0) >= 5,
            "{} sample too small",
            c.label()
        );
    }
    assert!(
        sample.len() * 10 >= fx.questions.len(),
        "sample under 10% of the questions"
    );

    let verdicts = [
        "ground_truth_correct",
        "note_relevance_correct",
        "key_fact_correct",
    ];
    if status == "pending" {
        assert!(r["error_rate"].is_null() && r["reviewer"].is_null());
        for s in sample {
            assert!(
                verdicts.iter().all(|v| s[*v].is_null()),
                "pending record has verdicts"
            );
        }
    } else {
        assert!(r["reviewer"].is_string() && r["reviewed_on"].is_string());
        let wrong = sample
            .iter()
            .filter(|s| verdicts.iter().any(|v| s[*v] == json!(false)))
            .count();
        let rate = wrong as f64 / sample.len() as f64;
        assert!(
            (r["error_rate"].as_f64().unwrap() - rate).abs() < 1e-9,
            "error_rate disagrees with the verdicts"
        );
        assert!(
            rate <= r["error_rate_threshold"].as_f64().unwrap(),
            "error rate {rate} above threshold"
        );
    }
}

// ── origin submission at ingest (FR-005) ─────────────────────────────────────────

/// Records every call; answers just enough for `ingest`.
struct LogTransport {
    log: Mutex<Vec<(String, Value)>>,
    n: Mutex<usize>,
}
impl RpcTransport for LogTransport {
    fn call<'a>(&'a self, method: &'a str, p: Value) -> BoxFuture<'a, Result<Value, String>> {
        self.log.lock().unwrap().push((method.to_string(), p));
        let out = match method {
            "knowledge_process_chunk" => {
                let mut n = self.n.lock().unwrap();
                *n += 1;
                Ok(json!({"episode_uuid": format!("ep-{n}")}))
            }
            "knowledge_get_entities_by_source" => Ok(json!({"nodes": []})),
            _ => Ok(json!({})),
        };
        Box::pin(async move { out })
    }
}

#[tokio::test]
async fn ingest_submits_every_note_with_its_declared_origin_and_every_supersession() {
    let fx = load();
    let t = LogTransport {
        log: Mutex::new(vec![]),
        n: Mutex::new(0),
    };
    let map = lcg::ingest(&t, &fx).await.unwrap();
    let declared: HashMap<&str, &str> = fx
        .manifest
        .notes
        .iter()
        .map(|n| (n.id.as_str(), n.origin.as_deref().unwrap()))
        .collect();
    let log = t.log.lock().unwrap();
    let mut chunks = 0;
    let mut seen_origins = BTreeSet::new();
    for (m, p) in log.iter().filter(|(m, _)| m == "knowledge_process_chunk") {
        assert_eq!(m, "knowledge_process_chunk");
        chunks += 1;
        let doc = p["doc_id"].as_str().unwrap();
        assert_eq!(p["origin"], declared[doc], "{doc}");
        seen_origins.insert(p["origin"].as_str().unwrap().to_string());
    }
    assert_eq!(seen_origins.len(), 4);
    assert_eq!(chunks, map.chunks);
    assert_eq!(chunks, fx.stats().chunks);
    let sups: Vec<&Value> = log
        .iter()
        .filter(|(m, _)| m == "knowledge_supersede_document")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(sups.len(), fx.manifest.supersedes.len());
    // The recorded effective date is the one sent, not the new note's recording date.
    let s0 = &fx.manifest.supersedes[0];
    assert!(sups
        .iter()
        .any(|p| p["superseding_doc_id"] == s0.new_note.as_str()
            && p["effective_at"] == s0.effective_at.as_str()));
}

// ── end to end: CIs and per-origin report on v2 ──────────────────────────────────

fn words(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_string)
        .collect()
}

/// Lexical stand-in for the LCG service: indexes chunks at ingest, ranks by word overlap.
struct FakeLcg {
    chunks: Mutex<Vec<Doc>>,
    n: Mutex<usize>,
}
impl RpcTransport for FakeLcg {
    fn call<'a>(&'a self, method: &'a str, p: Value) -> BoxFuture<'a, Result<Value, String>> {
        let out = match method {
            "knowledge_process_chunk" => {
                let mut n = self.n.lock().unwrap();
                *n += 1;
                let text = p["chunk_text"].as_str().unwrap().to_string();
                self.chunks.lock().unwrap().push((
                    format!(
                        "{}:{}",
                        p["source_file"].as_str().unwrap(),
                        p["chunk_id"].as_str().unwrap()
                    ),
                    text.clone(),
                    words(&text),
                ));
                Ok(json!({"episode_uuid": format!("ep-{n}")}))
            }
            "knowledge_get_entities_by_source" => Ok(json!({"nodes": []})),
            "knowledge_search_passages" => {
                let q = words(p["query"].as_str().unwrap());
                let chunks = self.chunks.lock().unwrap();
                let mut scored: Vec<(usize, &Doc)> = chunks
                    .iter()
                    .map(|c| (c.2.intersection(&q).count(), c))
                    .filter(|(s, _)| *s > 0)
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1 .0.cmp(&b.1 .0)));
                let passages: Vec<Value> = scored
                    .into_iter()
                    .take(K)
                    .map(|(_, c)| json!({"source_description": c.0, "content": c.1}))
                    .collect();
                Ok(json!({"passages": passages}))
            }
            _ => Ok(json!({})),
        };
        Box::pin(async move { out })
    }
}

/// Whole-note lexical ranking standing in for qmd.
struct NoteLexical {
    notes: Vec<Doc>,
}
impl Retriever for NoteLexical {
    fn id(&self) -> &str {
        "qmd"
    }
    fn description(&self) -> String {
        "whole-note lexical".into()
    }
    fn retrieve<'a>(&'a self, q: &'a str, k: usize) -> BoxFuture<'a, Result<RankedResult, String>> {
        let qw = words(q);
        let mut scored: Vec<(usize, &Doc)> = self
            .notes
            .iter()
            .map(|n| (n.2.intersection(&qw).count(), n))
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1 .0.cmp(&b.1 .0)));
        let top: Vec<_> = scored.into_iter().take(k).collect();
        let res = RankedResult {
            notes: top.iter().map(|(_, n)| n.0.clone()).collect(),
            evidence: top.iter().map(|(_, n)| n.1.clone()).collect(),
            model_calls: None,
            lookup_calls: 0,
            evidence_gaps: 0,
            latency: Duration::from_millis(1),
        };
        Box::pin(async move { Ok(res) })
    }
}

struct SubstringJudge;
impl EvidenceJudge for SubstringJudge {
    fn model(&self) -> &str {
        "test-judge"
    }
    fn supports<'a>(&'a self, k: &'a str, e: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        let kw = words(k);
        let hit = kw.intersection(&words(e)).count();
        Box::pin(async move { Ok(hit * 10 >= kw.len() * 8) })
    }
}

async fn run_report(fx: &Fixture, judge_path: &Path) -> Report {
    let transport: Arc<FakeLcg> = Arc::new(FakeLcg {
        chunks: Mutex::new(vec![]),
        n: Mutex::new(0),
    });
    let dynt: Arc<dyn RpcTransport> = transport.clone();
    let map = lcg::ingest(dynt.as_ref(), fx).await.unwrap();
    let slots = vec![
        Slot::Run(Box::new(LcgPassages { t: dynt.clone() })),
        Slot::Run(Box::new(NoteLexical {
            notes: fx
                .notes
                .iter()
                .map(|(k, v)| (k.clone(), v.clone(), words(v)))
                .collect(),
        })),
    ];
    let inner = SubstringJudge;
    let judge = CachedEvidenceJudge::load(&inner, judge_path).unwrap();
    let configs = evaluate(fx, K, slots, Some(&judge), "no judge").await;
    let args = match lcg_eval::retrieval::cli::parse_args(&[]).unwrap() {
        lcg_eval::retrieval::cli::RetrievalMode::Run(a) => *a,
        _ => unreachable!(),
    };
    Report {
        meta: RunMeta {
            corpus_version: fx.manifest.corpus_version.clone(),
            corpus_hash: fx.corpus_hash.clone(),
            questions_version: fx.questions_version.clone(),
            questions_hash: fx.questions_hash.clone(),
            k: K,
            category_counts: resolve(&args, fx).category_counts,
            bootstrap: Bootstrap {
                seed: 788,
                resamples: 200,
            },
            ..Default::default()
        },
        ingest: IngestCost {
            lcg_chunks: map.chunks,
            ..Default::default()
        },
        configs,
    }
}

#[tokio::test]
async fn v2_report_has_intervals_origin_breakdown_and_is_reproducible() {
    let fx = load();
    let d = tempfile::tempdir().unwrap();
    let a = run_report(&fx, &d.path().join("j1.jsonl")).await;
    let b = run_report(&fx, &d.path().join("j2.jsonl")).await;

    let (ma, mb) = (a.render_markdown(), b.render_markdown());
    assert_eq!(
        ma, mb,
        "two runs with the same seed must produce the identical report"
    );
    // Everything except wall-clock latency is exactly reproducible.
    let (ja, jb) = (a.to_json(), b.to_json());
    assert_eq!(ja["comparisons"], jb["comparisons"]);
    assert_eq!(ja["bootstrap"], jb["bootstrap"]);
    for (ca, cb) in ja["configs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(jb["configs"].as_array().unwrap())
    {
        for key in ["overall", "by_category", "ci", "origins", "questions"] {
            assert_eq!(ca[key], cb[key], "{} {key}", ca["id"]);
        }
    }

    for needle in [
        "### multi-hop (n = 30)",
        "### passage (n = 30)",
        "### negative (n = 34)",
        "Paired nDCG@k comparison",
        "## By origin of returned notes",
        "synthetic and template-generated",
    ] {
        assert!(ma.contains(needle), "missing {needle:?}");
    }
    let j = a.to_json();
    // Every headline metric of every ran configuration carries an interval where it is defined.
    for cfg in j["configs"].as_array().unwrap() {
        for m in [
            "recall",
            "mrr",
            "ndcg",
            "grounded_recall",
            "stale_above_current",
            "false_positive",
        ] {
            assert!(
                cfg["ci"]["overall"][m].is_object(),
                "{} {m}: {}",
                cfg["id"],
                cfg["ci"]["overall"][m]
            );
        }
        // Direct questions: shown with an interval (SC-005), whether or not they are at ceiling.
        assert!(cfg["ci"]["by_category"]["direct"]["ndcg"].is_object());
        assert!(cfg["origins"]["eligible_questions"].as_u64().unwrap() >= 20);
    }
    assert!(j["comparisons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["scope"] == "direct"));
    // Per-question granularity: one question moves recall by at most 0.0125 in aggregate.
    let scored = a.configs.iter().find_map(|c| match &c.status {
        Status::Ran(r) => Some(r.scores.len()),
        _ => None,
    });
    assert!(scored.unwrap() >= 200);
}
