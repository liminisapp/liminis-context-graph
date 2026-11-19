//! Network-free coverage of the retrieval-v2 fixture (#788): loading and shape (SC-001), the
//! generator's determinism and drift check (SC-002), the human-review record (SC-003), temporal
//! ground truth without recorded supersession, origin submission at ingest, and an end-to-end run
//! that exercises confidence intervals and the per-origin report. Nothing here needs qmd, a model
//! or a network.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use lcg_eval::retrieval::evidence_judge::{CachedEvidenceJudge, EvidenceJudge};
use lcg_eval::retrieval::fixture::{self, Category, Fixture};
use lcg_eval::retrieval::generator::check::{self, has_value, Rendered};
use lcg_eval::retrieval::generator::questions::{
    content_words, overlap, PARAPHRASE_MAX_NOTE_OVERLAP,
};
use lcg_eval::retrieval::generator::render;
use lcg_eval::retrieval::generator::skeleton::{skeleton_hash, SkNote, Skeleton};
use lcg_eval::retrieval::generator::{self, RenderMeta, Scenario};
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

fn scenario() -> Scenario {
    generator::load_scenario(&v2_dir().join("scenario.json")).unwrap()
}

fn skeleton_text() -> String {
    std::fs::read_to_string(v2_dir().join("skeleton.json")).expect("skeleton.json is checked in")
}

fn rendered_exists() -> bool {
    v2_dir().join("render.json").exists()
}

fn meta_for(sk: &Skeleton, text: &str, model: &str) -> RenderMeta {
    RenderMeta {
        render_model: model.into(),
        prompt_version: render::PROMPT_VERSION.into(),
        rendered_on: "2026-11-18".into(),
        skeleton_hash: skeleton_hash(text),
        corpus_version: sk.corpus_version.clone(),
        questions_version: sk.questions_version.clone(),
        model_calls: 0,
    }
}

/// The skeleton rendered with the template **test double** into a scratch directory. This lets the
/// whole pipeline be tested with no model. It is never the checked-in fixture.
fn template_dir() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let g = generator::generate(&scenario()).unwrap();
        let text = g.skeleton.to_json();
        let d = tempfile::tempdir().unwrap();
        generator::write_fixture(
            d.path(),
            &g.skeleton,
            &g.template,
            &meta_for(&g.skeleton, &text, "template-test-double"),
        )
        .unwrap();
        std::fs::write(d.path().join("skeleton.json"), &text).unwrap();
        d
    })
    .path()
}

fn load_template() -> Fixture {
    fixture::load(template_dir()).expect("the template render of the skeleton must load")
}

/// Fixtures the shape tests run on: the template test double always, and the checked-in rendered
/// fixture once a maintainer has rendered it.
fn fixtures_under_test() -> Vec<(&'static str, Fixture)> {
    let mut v = vec![("template test double", load_template())];
    if rendered_exists() {
        v.push((
            "checked-in render",
            fixture::load(&v2_dir())
                .expect("the rendered retrieval-v2 must load with zero validation errors"),
        ));
    }
    v
}

fn count_by_category(fx: &Fixture) -> HashMap<Category, usize> {
    let mut by = HashMap::new();
    for q in &fx.questions {
        *by.entry(q.category).or_default() += 1;
    }
    by
}

// ── scope change: the checked-in fixture is skeleton + LLM prose, never template prose ──────

#[test]
fn the_checked_in_fixture_is_either_unrendered_or_llm_rendered_never_template_prose() {
    let d = v2_dir();
    assert!(d.join("skeleton.json").exists() && d.join("scenario.json").exists());
    if rendered_exists() {
        let m: RenderMeta =
            serde_json::from_str(&std::fs::read_to_string(d.join("render.json")).unwrap()).unwrap();
        assert_ne!(
            m.render_model, "template-test-double",
            "template prose must not be checked in"
        );
        assert!(m.render_model.starts_with("claude"), "{}", m.render_model);
        assert_eq!(
            m.prompt_version,
            render::PROMPT_VERSION,
            "re-render after changing the prompts"
        );
        assert_eq!(m.skeleton_hash, skeleton_hash(&skeleton_text()));
    } else {
        // Not rendered yet: no prose of any kind is checked in, and the loader says what to do.
        for f in [
            "manifest.json",
            "questions.jsonl",
            "questions.meta.json",
            "notes",
        ] {
            assert!(
                !d.join(f).exists(),
                "{f} is checked in but retrieval-v2 has not been rendered"
            );
        }
        let e = fixture::load(&d).unwrap_err();
        assert!(
            e.contains("not been rendered") && e.contains("retrieval-render"),
            "{e}"
        );
    }
}

#[test]
fn skeleton_equals_the_generator_output_for_the_checked_in_scenario() {
    let g = generator::generate(&scenario()).unwrap();
    assert!(
        g.skeleton.to_json() == skeleton_text(),
        "skeleton.json drifted from scenario.json; run `lcg-eval retrieval-gen --spec crates/eval/fixtures/retrieval-v2/scenario.json --out crates/eval/fixtures/retrieval-v2`"
    );
}

#[test]
fn the_rendered_fixture_if_present_matches_the_skeleton_and_passes_every_check() {
    if !rendered_exists() {
        return; // pending maintainer render; the test above pins that state
    }
    let text = skeleton_text();
    let sk = Skeleton::from_json(&text).unwrap();
    let f = generator::check_dir(&v2_dir(), &sk, &text).unwrap();
    assert!(
        f.errors.is_empty(),
        "rendered retrieval-v2 fails its checks:\n{}",
        check::describe(&f)
    );
}

// ── render-step robustness ──────────────────────────────────────────────────────

#[test]
fn resume_sees_notes_rendered_before_a_crash_even_without_questions_jsonl() {
    // A crashed first render has notes/*.md (written as each note finishes) but no questions.jsonl
    // (written only at the very end); --resume must still keep those notes.
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("notes")).unwrap();
    std::fs::write(d.path().join("notes/alpha.md"), "# Alpha\n").unwrap();
    assert!(check::load_rendered(d.path()).is_err());
    let r = check::load_rendered_partial(d.path());
    assert_eq!(r.notes.len(), 1);
    assert!(r.notes.contains_key("alpha") && r.questions.is_empty());
    // Nothing rendered at all is an empty start, not an error.
    let empty = tempfile::tempdir().unwrap();
    let r = check::load_rendered_partial(empty.path());
    assert!(r.notes.is_empty() && r.questions.is_empty());
}

#[test]
fn render_checks_report_a_corpus_shape_shortfall_as_an_error() {
    let ok = load_template();
    assert!(
        ok.v2_shape_errors().is_empty(),
        "{:?}",
        ok.v2_shape_errors()
    );
    // Prose that came out far too short: every note collapses to a few words.
    let mut short = load_template();
    for body in short.notes.values_mut() {
        *body = "Brief note.".to_string();
    }
    let errs = short.v2_shape_errors();
    assert!(
        errs.iter().any(|e| e.contains("500-2,000")) && errs.iter().any(|e| e.contains("5,000")),
        "{errs:?}"
    );
}

#[test]
fn render_checks_enforce_the_negative_and_paraphrase_floors_the_tests_assert() {
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let mut r = check::load_rendered(template_dir()).unwrap();
    let base = check::check_rendered(&sk, &r);
    assert!(base.errors.is_empty(), "{}", check::describe(&base));
    let neg = sk.questions.iter().find(|q| q.negative).unwrap().id.clone();
    let para = sk
        .questions
        .iter()
        .find(|q| q.category == "paraphrase")
        .unwrap()
        .id
        .clone();
    r.questions
        .insert(neg.clone(), "zzqx wvvu plorg frimble snarkle?".into());
    r.questions.insert(para.clone(), "What is it?".into());
    let f = check::check_rendered(&sk, &r);
    assert!(
        f.errors
            .iter()
            .any(|e| e.contains(&neg) && e.contains("not a near miss")),
        "{}",
        check::describe(&f)
    );
    assert!(
        f.errors
            .iter()
            .any(|e| e.contains(&para) && e.contains("content words")),
        "{}",
        check::describe(&f)
    );
}

// ── relevance labels are complete (#788 maintainer review) ─────────────────────────

/// Skeleton notes that carry one of `q`'s basis facts, by identical statement or by the same
/// entity, attribute and value, other than `derived` and stale notes and those already relevant.
fn unmarked_fact_carriers(
    sk: &Skeleton,
    q: &lcg_eval::retrieval::generator::skeleton::SkQuestion,
) -> Vec<String> {
    let mut out = Vec::new();
    for b in &q.basis {
        let Some(t) = sk
            .notes
            .iter()
            .flat_map(|n| n.facts.iter())
            .find(|f| &f.statement == b)
        else {
            continue;
        };
        for n in &sk.notes {
            if n.origin == "derived"
                || q.stale.contains(&n.id)
                || q.relevant_notes.iter().any(|r| r.note == n.id)
            {
                continue;
            }
            if n.facts.iter().any(|f| {
                &f.statement == b
                    || (f.entity == t.entity && f.attribute == t.attribute && f.value == t.value)
            }) {
                out.push(format!("{} <- {}", q.id, n.id));
            }
        }
    }
    out
}

#[test]
fn every_note_stating_a_basis_fact_is_relevant_except_derived_and_stale_notes() {
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let mut missing = Vec::new();
    for q in sk.questions.iter().filter(|q| !q.negative) {
        missing.extend(unmarked_fact_carriers(&sk, q));
        for r in &q.relevant_notes {
            let n = sk.note(&r.note).unwrap();
            assert_ne!(n.origin, "derived", "{}: '{}' is derived", q.id, r.note);
            assert!(!q.stale.contains(&r.note), "{}: stale note relevant", q.id);
        }
    }
    assert!(missing.is_empty(), "unmarked carriers: {missing:?}");
    // The maintainer's multi-hop findings: a filed answer or recap stating a hop fact is gain 1
    // beside the gain-2 hop notes, and a gain-2 note is never downgraded.
    let q = sk.questions.iter().find(|q| q.id == "multihop-22").unwrap();
    let gain = |id: &str| {
        q.relevant_notes
            .iter()
            .find(|r| r.note == id)
            .map(|r| r.gain)
    };
    assert_eq!(gain("recap-04-orbit-core"), Some(1));
    assert_eq!(gain("project-tern-core-1"), Some(2));
    let q = sk.questions.iter().find(|q| q.id == "multihop-25").unwrap();
    assert!(q
        .relevant_notes
        .iter()
        .any(|r| r.note == "answer-13-cedar-core-budget" && r.gain == 1));
}

#[test]
fn the_relevance_check_flags_an_unlisted_note_that_states_a_basis_fact() {
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let mut r = check::load_rendered(template_dir()).unwrap();
    let base = check::check_rendered(&sk, &r);
    assert!(base.errors.is_empty(), "{}", check::describe(&base));
    // A direct question whose basis fact we then also state in an unrelated primary note that
    // names the same entity.
    let q = sk.questions.iter().find(|q| q.id == "direct-01").unwrap();
    let statement = &q.basis[0];
    let fact = sk
        .notes
        .iter()
        .flat_map(|n| n.facts.iter())
        .find(|f| &f.statement == statement)
        .unwrap();
    let carrier = sk
        .notes
        .iter()
        .find(|n| {
            n.origin == "primary"
                && n.kind == "journal"
                && !q.relevant_notes.iter().any(|x| x.note == n.id)
                && n.entities.iter().any(|e| e.name == fact.entity)
        })
        .or_else(|| {
            sk.notes.iter().find(|n| {
                n.origin == "primary"
                    && !q.relevant_notes.iter().any(|x| x.note == n.id)
                    && n.entities.iter().any(|e| e.name == fact.entity)
            })
        })
        .expect("a primary note about the same entity outside the relevant set");
    let derived = sk
        .notes
        .iter()
        .find(|n| n.origin == "derived" && n.entities.iter().any(|e| e.name == fact.entity));
    let original = r.notes.get(&carrier.id).unwrap().clone();
    r.notes
        .insert(carrier.id.clone(), format!("{original}\n\n{statement}\n"));
    let f = check::check_rendered(&sk, &r);
    assert!(
        f.errors
            .iter()
            .any(|e| e.contains(&q.id) && e.contains(&carrier.id) && e.contains("relevant_notes")),
        "{}",
        check::describe(&f)
    );
    // The same sentence in a derived note is a restatement, which is scored separately.
    r.notes.insert(carrier.id.clone(), original);
    if let Some(d) = derived {
        let body = r.notes.get(&d.id).unwrap().clone();
        r.notes
            .insert(d.id.clone(), format!("{body}\n\n{statement}\n"));
        let f = check::check_rendered(&sk, &r);
        assert!(
            !f.errors.iter().any(|e| e.contains("not in relevant_notes")),
            "{}",
            check::describe(&f)
        );
    }
}

#[test]
fn a_shared_value_in_another_context_is_not_a_stated_fact() {
    // `9` in "9 December 2027" must not make a note that mentions the entity and the word "team"
    // state "The Helix Core team consists of 9 engineers."
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let mut r = check::load_rendered(template_dir()).unwrap();
    let q = sk.questions.iter().find(|q| q.id == "direct-05").unwrap();
    let carrier = sk
        .notes
        .iter()
        .find(|n| {
            n.origin == "primary"
                && !q.relevant_notes.iter().any(|x| x.note == n.id)
                && n.entities.iter().any(|e| e.name == "Helix Core")
        })
        .unwrap();
    let body = r.notes.get(&carrier.id).unwrap().clone();
    r.notes.insert(
        carrier.id.clone(),
        format!("{body}\n\nHelix Core held its quarterly retrospective on 9 December 2027, and the notes are in the team folder.\n"),
    );
    let f = check::check_rendered(&sk, &r);
    assert!(
        !f.errors.iter().any(|e| e.contains("not in relevant_notes")),
        "{}",
        check::describe(&f)
    );
}

#[test]
fn completing_relevance_leaves_the_corpus_hash_and_so_the_cassette_alone() {
    if !rendered_exists() {
        return;
    }
    // Only questions changed: the notes and manifest, and so the extraction cassette's inputs, are
    // the ones it was recorded against (README: corpus hash da41ea92487ba4fc).
    let fx = fixture::load(&v2_dir()).unwrap();
    assert_eq!(fx.corpus_hash, "da41ea92487ba4fc");
}

// ── SC-001: shape (checked on every fixture under test) ──────────────────────────

#[test]
fn v2_loads_and_has_the_required_scale_and_mix() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
        assert_eq!(fx.questions_version, "meridian-questions-3");
    }
}

#[test]
fn v2_has_all_four_origins_primary_largest_and_derived_pages_cite_primary_notes() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
}

#[test]
fn near_duplicates_exist_and_ground_truth_names_which_note_is_relevant() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
}

// ── temporal realism (US4, FR-006, FR-013) ───────────────────────────────────────

#[test]
fn decisions_are_revised_three_or_more_times_with_explicit_and_implicit_steps() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
}

#[test]
fn temporal_questions_declare_current_and_stale_without_needing_a_supersession() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
}

// ── question rules (FR-010, FR-011, FR-012) ──────────────────────────────────────

#[test]
fn evidence_is_stated_in_its_note_and_passage_facts_sit_beyond_the_first_chunk() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
        let max = fx.manifest.max_chunk_tokens;
        let mut passages = 0;
        // An LLM render may reword the sentence, so "stated in its note" means what check.rs
        // means: the fact's entity and value (from the skeleton) both appear. The verbatim
        // sentence is only guaranteed for the template test double.
        let sk = Skeleton::from_json(&skeleton_text()).unwrap();
        let anchor = |note: &str, key_fact: &str| -> (String, String) {
            let f = sk
                .note(note)
                .and_then(|n| n.facts.iter().find(|x| x.statement == key_fact))
                .unwrap_or_else(|| panic!("key fact of {note} is not a skeleton fact"));
            (f.entity.to_lowercase(), f.value.to_lowercase())
        };
        let anchored = |text: &str, (entity, value): &(String, String)| {
            let t = text.to_lowercase();
            t.contains(entity.as_str()) && t.contains(value.as_str())
        };
        for q in &fx.questions {
            for e in &q.evidence {
                assert!(
                    anchored(&fx.notes[&e.note], &anchor(&e.note, &e.key_fact)),
                    "{}: key fact (entity and value) not found in {}",
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
                    let a = anchor(&q.evidence[0].note, &q.evidence[0].key_fact);
                    // The chunk carrying the fact's sentence (as check.rs locates it), not merely
                    // the first one containing the value as a substring (`10` in `10 April 2025`).
                    let kf = &q.evidence[0].key_fact;
                    let at = chunks
                        .iter()
                        .position(|c| {
                            anchored(c, &a)
                                && lcg_eval::retrieval::generator::questions::overlap(kf, c) >= 0.6
                        })
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
}

#[test]
fn paraphrase_questions_stay_below_the_lexical_overlap_bound() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
}

#[test]
fn every_negative_has_a_near_miss_distractor_and_no_relevant_note() {
    for (label, fx) in fixtures_under_test() {
        let _ = label;
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
}

// ── SC-002: generator determinism and drift ──────────────────────────────────────

#[test]
fn generation_is_byte_identical_for_the_same_seed_and_differs_for_another() {
    let sc = scenario();
    let a = generator::generate(&sc).unwrap();
    let b = generator::generate(&sc).unwrap();
    assert_eq!(a.skeleton, b.skeleton);
    assert_eq!(a.skeleton.to_json(), b.skeleton.to_json());
    assert_eq!(a.template.notes, b.template.notes);
    let other = Scenario {
        seed: sc.seed + 1,
        ..sc
    };
    // A different seed still yields a valid skeleton (generate re-validates through the loader).
    let c = generator::generate(&other).unwrap();
    assert_ne!(a.skeleton.to_json(), c.skeleton.to_json());
    assert!((300..=500).contains(&c.skeleton.notes.len()) && c.skeleton.questions.len() >= 200);
}

#[test]
fn skeleton_counts_meet_the_spec_before_any_prose_exists() {
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    assert!((300..=500).contains(&sk.notes.len()), "{}", sk.notes.len());
    assert!(sk.questions.len() >= 200 && sk.questions.iter().filter(|q| q.negative).count() >= 30);
    let by = sk.category_counts();
    for c in [
        "direct",
        "paraphrase",
        "temporal",
        "answer",
        "multi-hop",
        "passage",
        "negative",
    ] {
        assert!(by.get(c).copied().unwrap_or(0) >= 30, "{c}: {by:?}");
    }
    let origins: BTreeSet<&str> = sk.notes.iter().map(|n| n.origin.as_str()).collect();
    assert_eq!(origins.len(), 4);
    // Target sizes: many short notes, a substantial 500-2,000 group, at least three over 5,000.
    let t = |lo: usize, hi: usize| {
        sk.notes
            .iter()
            .filter(|n| n.target_words >= lo && n.target_words < hi)
            .count()
    };
    assert!(t(0, 250) >= 150 && t(500, 2000) >= 50 && t(5000, usize::MAX) >= 3);
    // Every long note is sectioned so passage facts can be placed beyond the first chunk.
    assert!(sk.notes.iter().filter(|n| n.sections > 1).count() >= 50);
}

#[test]
fn the_skeleton_is_free_of_template_prose() {
    // The skeleton carries structure and plain hints, never a note body.
    let text = skeleton_text();
    assert!(!text.contains("Quick recap from the week"));
    assert!(!text.contains("Agent-maintained page"));
    let sk = Skeleton::from_json(&text).unwrap();
    assert!(sk.notes.iter().all(|n| n.title_hint.len() < 120));
}

#[test]
fn scenario_review_and_skeleton_files_do_not_change_the_fixture_hashes() {
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
    for f in [
        "scenario.json",
        "review.json",
        "skeleton.json",
        "render.json",
    ] {
        std::fs::write(d.path().join(f), "{}").unwrap();
    }
    let after = fixture::load(d.path()).unwrap();
    assert_eq!(before.corpus_hash, after.corpus_hash);
    assert_eq!(before.questions_hash, after.questions_hash);
}

// ── SC-003: human-review record ──────────────────────────────────────────────────

#[test]
fn review_record_is_a_pending_template_unless_a_human_filled_it() {
    let sk_text = skeleton_text();
    let sk = Skeleton::from_json(&sk_text).unwrap();
    let r: Value = serde_json::from_str(
        &std::fs::read_to_string(v2_dir().join("review.json")).expect("review.json is checked in"),
    )
    .unwrap();
    assert_eq!(
        r["skeleton_hash"],
        skeleton_hash(&sk_text).as_str(),
        "questions changed since the review template was written"
    );
    assert_eq!(r["questions_version"], sk.questions_version.as_str());
    let status = r["status"].as_str().unwrap();
    assert!(status == "pending" || status == "reviewed", "{status}");

    let sample = r["sample"].as_array().unwrap();
    let mut per_cat: HashMap<&str, usize> = HashMap::new();
    for s in sample {
        let id = s["id"].as_str().unwrap();
        let q = sk
            .questions
            .iter()
            .find(|q| q.id == id)
            .unwrap_or_else(|| panic!("sampled question {id} does not exist"));
        assert_eq!(s["category"], q.category.as_str());
        *per_cat.entry(q.category.as_str()).or_default() += 1;
    }
    for c in [
        "direct",
        "paraphrase",
        "temporal",
        "answer",
        "multi-hop",
        "passage",
        "negative",
    ] {
        assert!(
            per_cat.get(c).copied().unwrap_or(0) >= 5,
            "{c} sample too small"
        );
    }
    assert!(
        sample.len() * 10 >= sk.questions.len(),
        "sample under 10% of the questions"
    );

    let verdicts = [
        "ground_truth_correct",
        "note_relevance_correct",
        "key_fact_correct",
    ];
    if rendered_exists() {
        let fx = fixture::load(&v2_dir()).unwrap();
        // Once rendered, the record names the question set it is about.
        assert_eq!(
            r["questions_hash"],
            fx.questions_hash.as_str(),
            "re-render or re-review: the record is for different questions"
        );
    } else {
        assert!(
            r["questions_hash"].is_null(),
            "no questions exist to hash yet"
        );
    }
    if status == "pending" {
        // The implementer never writes verdicts: they are a human's.
        assert!(r["error_rate"].is_null() && r["reviewer"].is_null() && r["reviewed_on"].is_null());
        for s in sample {
            assert!(
                verdicts.iter().all(|v| s[*v].is_null()),
                "a pending record has verdicts in it"
            );
        }
    } else {
        assert!(
            rendered_exists(),
            "cannot be reviewed before it is rendered"
        );
        assert!(r["reviewer"].is_string() && r["reviewed_on"].is_string());
        assert!(
            sample
                .iter()
                .all(|s| verdicts.iter().all(|v| s[*v].is_boolean())),
            "every verdict must be filled"
        );
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

#[test]
fn update_review_fills_a_pending_record_and_never_touches_a_reviewed_one() {
    let d = tempfile::tempdir().unwrap();
    let qs = "{\"id\":\"direct-01\",\"category\":\"direct\",\"text\":\"Who owns X?\"}\n";
    let pending = json!({"status": "pending", "questions_hash": null, "sample": [{"id": "direct-01", "text": null, "ground_truth_correct": null}]});
    std::fs::write(d.path().join("review.json"), pending.to_string()).unwrap();
    render::update_review(d.path(), qs, "abc123", "q-9").unwrap();
    let r: Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join("review.json")).unwrap())
            .unwrap();
    assert_eq!(r["questions_hash"], fixture::questions_hash_of(qs).as_str());
    assert_eq!(r["sample"][0]["text"], "Who owns X?");
    assert_eq!(r["skeleton_hash"], "abc123");
    assert_eq!(r["questions_version"], "q-9");
    assert!(
        r["sample"][0]["ground_truth_correct"].is_null(),
        "verdicts stay a human's"
    );

    let reviewed = json!({"status": "reviewed", "questions_hash": "0000000000000000", "sample": [{"id": "direct-01", "ground_truth_correct": true}]});
    std::fs::write(d.path().join("review.json"), reviewed.to_string()).unwrap();
    let before = std::fs::read_to_string(d.path().join("review.json")).unwrap();
    let e = render::update_review(d.path(), qs, "abc123", "q-9").unwrap_err();
    assert!(e.contains("re-review"), "{e}");
    assert_eq!(
        std::fs::read_to_string(d.path().join("review.json")).unwrap(),
        before
    );
}

// ── origin submission at ingest (FR-005) ─────────────────────────────────────────

/// Records every call; answers just enough for `ingest`.
struct LogTransport {
    log: Mutex<Vec<(String, Value)>>,
    n: Mutex<usize>,
    episodes: Mutex<HashMap<String, Vec<String>>>,
}
impl RpcTransport for LogTransport {
    fn call<'a>(&'a self, method: &'a str, p: Value) -> BoxFuture<'a, Result<Value, String>> {
        self.log
            .lock()
            .unwrap()
            .push((method.to_string(), p.clone()));
        let out = match method {
            "knowledge_process_chunk" => {
                let mut n = self.n.lock().unwrap();
                *n += 1;
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
            "knowledge_get_entities_by_source" => Ok(json!({"nodes": []})),
            _ => Ok(json!({})),
        };
        Box::pin(async move { out })
    }
}

#[tokio::test]
async fn ingest_submits_every_note_with_its_declared_origin_and_every_supersession() {
    let fx = load_template();
    let t = LogTransport {
        log: Mutex::new(vec![]),
        n: Mutex::new(0),
        episodes: Mutex::new(HashMap::new()),
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
    episodes: Mutex<HashMap<String, Vec<String>>>,
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
            // Graph-aware search (#797): the fake has no graph, so it answers with the same
            // lexical passages, in the read's result shape.
            "knowledge_graph_search" => {
                let q = words(p["query"].as_str().unwrap());
                let chunks = self.chunks.lock().unwrap();
                let mut scored: Vec<(usize, &Doc)> = chunks
                    .iter()
                    .map(|c| (c.2.intersection(&q).count(), c))
                    .filter(|(s, _)| *s > 0)
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1 .0.cmp(&b.1 .0)));
                let results: Vec<Value> = scored
                    .into_iter()
                    .take(K)
                    .map(|(_, c)| {
                        json!({"kind": "passage",
                               "passage": {"source_description": c.0, "content": c.1}})
                    })
                    .collect();
                Ok(json!({"results": results, "stats": {"embed_calls": 1}}))
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
            service_calls: None,
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
        episodes: Mutex::new(HashMap::new()),
    });
    let dynt: Arc<dyn RpcTransport> = transport.clone();
    let map = lcg::ingest(dynt.as_ref(), fx).await.unwrap();
    let slots = vec![
        Slot::Run(Box::new(LcgPassages {
            t: dynt.clone(),
            mode: lcg::PassageMode::Default,
        })),
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
    let fx = load_template();
    let d = tempfile::tempdir().unwrap();
    let a = run_report(&fx, &d.path().join("j1.jsonl")).await;
    let b = run_report(&fx, &d.path().join("j2.jsonl")).await;

    let (ma, mb) = (a.render_markdown(), b.render_markdown());
    // The `## Cost` section holds wall-clock latency (p50/p95), which varies run to run under
    // host load; everything else in the markdown must be byte-identical for the same seed.
    let without_cost = |m: &str| {
        let (head, tail) = m
            .split_once("\n## Cost\n")
            .expect("report has a Cost section");
        let rest = tail
            .split_once("\n## Reproduction\n")
            .expect("report has a Reproduction section")
            .1;
        format!("{head}\n{rest}")
    };
    assert_eq!(
        without_cost(&ma),
        without_cost(&mb),
        "two runs with the same seed must produce the identical report (latency aside)"
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
        "synthetic (generated structure",
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

// ── the render step and its checks, with a model double (no network) ─────────────

/// Plays the LLM by returning the template prose for the note or questions named in the prompt.
struct TemplateModel {
    rendered: Rendered,
    calls: Mutex<usize>,
}
impl render::TextModel for TemplateModel {
    fn model(&self) -> &str {
        "template-test-double"
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        _max: u32,
    ) -> BoxFuture<'a, Result<String, String>> {
        *self.calls.lock().unwrap() += 1;
        let out = if system == render::QUESTION_SYSTEM {
            let ids: Vec<&str> = user
                .lines()
                .filter_map(|l| l.strip_prefix("id: "))
                .collect();
            let m: BTreeMap<&str, &String> = ids
                .iter()
                .map(|i| (*i, &self.rendered.questions[*i]))
                .collect();
            Ok(format!(
                "```json\n{}\n```",
                serde_json::to_string(&m).unwrap()
            ))
        } else {
            let id = user
                .lines()
                .find_map(|l| l.strip_prefix("NOTE ID: "))
                .unwrap();
            let body = &self.rendered.notes[id];
            let section = user
                .lines()
                .find_map(|l| l.strip_prefix("This is section "))
                .and_then(|l| l.split(' ').next())
                .and_then(|n| n.parse::<usize>().ok());
            match section {
                None => Ok(body.clone()),
                Some(k) => {
                    let parts: Vec<&str> = body.split("\n## ").collect();
                    Ok(if k == 1 {
                        parts[0].to_string()
                    } else {
                        format!("## {}", parts[k - 1])
                    })
                }
            }
        };
        Box::pin(async move { out })
    }
}

fn opts() -> render::Options {
    render::Options {
        concurrency: 4,
        resume: false,
        only: None,
    }
}

#[tokio::test]
async fn render_pipeline_produces_a_fixture_that_passes_every_check() {
    let g = generator::generate(&scenario()).unwrap();
    let model = TemplateModel {
        rendered: g.template.clone(),
        calls: Mutex::new(0),
    };
    let (nc, qc) = render::plan_calls(&g.skeleton, &Rendered::default(), &opts());
    let written = Mutex::new(0usize);
    let on_note = |_: &str, _: &str| *written.lock().unwrap() += 1;
    let (rendered, calls) =
        render::render_all(&g.skeleton, &model, Rendered::default(), &opts(), &on_note)
            .await
            .unwrap();
    assert_eq!(calls, nc + qc);
    assert_eq!(*model.calls.lock().unwrap(), nc + qc);
    assert_eq!(
        *written.lock().unwrap(),
        g.skeleton.notes.len(),
        "every note is persisted as it finishes"
    );
    for (id, body) in &g.template.notes {
        assert_eq!(rendered.notes[id].trim(), body.trim(), "note {id}");
    }
    assert_eq!(rendered.questions, g.template.questions);

    let d = tempfile::tempdir().unwrap();
    let text = g.skeleton.to_json();
    generator::write_fixture(
        d.path(),
        &g.skeleton,
        &rendered,
        &meta_for(&g.skeleton, &text, "template-test-double"),
    )
    .unwrap();
    let f = generator::check_dir(d.path(), &g.skeleton, &text).unwrap();
    assert!(f.errors.is_empty(), "{}", check::describe(&f));
    let fx = fixture::load(d.path()).unwrap();
    assert_eq!(fx.manifest.notes.len(), g.skeleton.notes.len());
    // The manifest is generated from the skeleton, not written by the model.
    assert_eq!(
        std::fs::read_to_string(d.path().join("manifest.json")).unwrap(),
        generator::manifest_json(&g.skeleton)
    );
    let meta = std::fs::read_to_string(d.path().join("questions.meta.json")).unwrap();
    assert!(
        meta.contains("prompt_version")
            && meta.contains("render_model")
            && meta.contains(&g.skeleton.questions_version)
    );
}

#[tokio::test]
async fn only_and_resume_rerender_just_what_they_select() {
    let g = generator::generate(&scenario()).unwrap();
    let model = TemplateModel {
        rendered: g.template.clone(),
        calls: Mutex::new(0),
    };
    let nothing = |_: &str, _: &str| {};
    let target = g
        .skeleton
        .notes
        .iter()
        .find(|n| n.sections == 1)
        .unwrap()
        .id
        .clone();
    let mut existing = g.template.clone();
    existing.notes.insert(target.clone(), "STALE".into());
    let only = render::Options {
        only: Some([target.clone()].into_iter().collect()),
        ..opts()
    };
    let (out, calls) = render::render_all(&g.skeleton, &model, existing.clone(), &only, &nothing)
        .await
        .unwrap();
    assert_eq!(calls, 1, "one note call and no question calls");
    assert_eq!(out.notes[&target].trim(), g.template.notes[&target].trim());
    // Resume keeps what is there (even stale text) and only renders the missing note.
    let mut partial = g.template.clone();
    let gone = g
        .skeleton
        .notes
        .iter()
        .find(|n| n.sections == 1 && n.id != target)
        .unwrap()
        .id
        .clone();
    partial.notes.remove(&gone);
    partial.notes.insert(target.clone(), "KEPT".into());
    let resume = render::Options {
        resume: true,
        ..opts()
    };
    let (out, _) = render::render_all(&g.skeleton, &model, partial, &resume, &nothing)
        .await
        .unwrap();
    assert_eq!(out.notes[&target], "KEPT");
    assert!(out.notes.contains_key(&gone));
}

#[test]
fn render_plan_counts_calls_per_section_and_per_question_batch() {
    let g = generator::generate(&scenario()).unwrap();
    let (nc, qc) = render::plan_calls(&g.skeleton, &Rendered::default(), &opts());
    assert_eq!(
        nc,
        g.skeleton
            .notes
            .iter()
            .map(|n| n.sections.max(1))
            .sum::<usize>()
    );
    assert!((7..=40).contains(&qc), "{qc}");
    // Resuming with every note present only has the question calls left.
    let (nc2, _) = render::plan_calls(
        &g.skeleton,
        &g.template,
        &render::Options {
            resume: true,
            ..opts()
        },
    );
    assert_eq!(nc2, 0);
}

fn first_of<'a>(sk: &'a Skeleton, pred: impl Fn(&&'a SkNote) -> bool) -> &'a SkNote {
    sk.notes.iter().find(pred).unwrap()
}

#[test]
fn prompts_carry_the_constraints_the_checks_enforce() {
    let g = generator::generate(&scenario()).unwrap();
    let sk = &g.skeleton;
    // An implicit revision must not be told to mention the earlier choice.
    let implicit = first_of(sk, |n| {
        n.kind == "decision" && n.explicit == Some(false) && !n.earlier_values.is_empty()
    });
    let (p, _) = render::note_prompt(implicit, None);
    assert!(
        p.contains("Do NOT say it replaces")
            && p.contains(&implicit.id)
            && p.contains(&implicit.facts[0].value)
    );
    // An explicit revision must state the effective date exactly.
    let explicit = first_of(sk, |n| n.explicit == Some(true));
    let (p, _) = render::note_prompt(explicit, None);
    assert!(
        p.contains("replaces the earlier choice")
            && p.contains(explicit.effective_text.as_ref().unwrap())
    );
    // A derived page is told which ids to cite.
    let wiki = first_of(sk, |n| n.kind == "wiki" && n.topic.is_none());
    let (p, _) = render::note_prompt(wiki, None);
    assert!(wiki.cites.iter().all(|c| p.contains(c.as_str())) && p.contains("Sources"));
    // A long note is rendered section by section, each with only its own facts.
    let long = first_of(sk, |n| {
        n.sections >= 3 && n.facts.iter().any(|f| f.section == Some(1))
    });
    let (p0, _) = render::note_prompt(long, Some(0));
    let (p1, _) = render::note_prompt(long, Some(1));
    assert!(p0.contains("section 1 of") && p1.contains("section 2 of") && p1.contains("`## `"));
    for f in long.facts.iter().filter(|f| f.section == Some(1)) {
        assert!(p1.contains(&f.value));
    }
    for f in long.facts.iter().filter(|f| f.section != Some(1)) {
        assert!(
            !p1.contains(&format!("value (verbatim): {} ", f.value)),
            "section 2 prompt leaks another section's fact"
        );
    }
    // A paraphrase prompt forbids the entity and the statement's words and gives a descriptor.
    let q = sk
        .questions
        .iter()
        .find(|q| q.category == "paraphrase")
        .unwrap();
    let (p, _) = render::questions_prompt(&[q]);
    assert!(
        p.contains("do NOT name the entity")
            && p.contains(q.descriptor.as_ref().unwrap())
            && p.contains("Do not use any of these words")
    );
    // Other questions must keep their names verbatim.
    let q = sk
        .questions
        .iter()
        .find(|q| q.category == "direct")
        .unwrap();
    let (p, _) = render::questions_prompt(&[q]);
    assert!(p.contains(&format!("\"{}\"", q.names[0])));
}

#[test]
fn model_replies_are_parsed_and_cleaned() {
    let m = render::parse_questions_reply(
        "Sure!\n```json\n{\"a-01\": \" Who owns X? \", \"a-02\": \"When?\"}\n```",
    )
    .unwrap();
    assert_eq!(m["a-01"], "Who owns X?");
    assert!(render::parse_questions_reply("no json here").is_err());
    assert_eq!(
        render::clean_note("```markdown\n# T\n\nbody\n```"),
        "# T\n\nbody"
    );
    assert_eq!(
        render::clean_note("Here is the note:\n\n# T\nbody"),
        "# T\nbody"
    );
    assert!(
        render::clean_note("_Clipped from the web. Source: x._\n\n# T").starts_with("_Clipped")
    );
}

fn template_rendered() -> (Skeleton, Rendered) {
    let g = generator::generate(&scenario()).unwrap();
    (g.skeleton, g.template)
}

#[test]
fn the_checks_accept_the_template_double_and_reject_each_kind_of_defect() {
    let (sk, ok) = template_rendered();
    let f = check::check_rendered(&sk, &ok);
    assert!(f.errors.is_empty(), "{}", check::describe(&f));

    // A fact's value missing from its note.
    let n = first_of(&sk, |n| n.kind == "fact");
    let mut r = ok.clone();
    let v = &n.facts[0].value;
    r.notes.insert(
        n.id.clone(),
        r.notes[&n.id].replace(v.as_str(), "[redacted]"),
    );
    let f = check::check_rendered(&sk, &r);
    assert!(
        f.errors
            .iter()
            .any(|e| e.contains(&n.id) && e.contains(v.as_str())),
        "{:?}",
        f.errors
    );

    // A note naming an entity it is not about.
    let other = sk
        .entities
        .iter()
        .find(|e| {
            !n.entities.iter().any(|x| x.name == e.name)
                && !n.facts.iter().any(|f| f.value.contains(&e.name))
        })
        .unwrap();
    let mut r = ok.clone();
    r.notes
        .get_mut(&n.id)
        .unwrap()
        .push_str(&format!("\nWe also spoke to {}.\n", other.name));
    let f = check::check_rendered(&sk, &r);
    assert!(
        f.errors
            .iter()
            .any(|e| e.contains(&n.id) && e.contains(&other.name) && e.contains("not about")),
        "{:?}",
        f.errors
    );

    // An implicit revision mentioning the earlier value; an explicit one missing its date.
    let imp = first_of(&sk, |n| {
        n.explicit == Some(false)
            && n.earlier_values
                .iter()
                .any(|v| v.chars().any(|c| c.is_uppercase()))
    });
    let earlier = imp
        .earlier_values
        .iter()
        .find(|v| v.chars().any(|c| c.is_uppercase()))
        .unwrap();
    let mut r = ok.clone();
    r.notes
        .get_mut(&imp.id)
        .unwrap()
        .push_str(&format!("\nThis moved on from {earlier}.\n"));
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&imp.id) && e.contains("earlier value")));
    let exp = first_of(&sk, |n| n.explicit == Some(true));
    let mut r = ok.clone();
    let eff = exp.effective_text.clone().unwrap();
    r.notes
        .insert(exp.id.clone(), r.notes[&exp.id].replace(&eff, "soon"));
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&exp.id) && e.contains("effective date")));

    // A derived page that does not cite its source.
    let wiki = first_of(&sk, |n| n.origin == "derived");
    let mut r = ok.clone();
    let cite = format!("[[{}]]", wiki.cites[0]);
    r.notes
        .insert(wiki.id.clone(), r.notes[&wiki.id].replace(&cite, ""));
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&wiki.id) && e.contains("cite")));

    // A paraphrase that names its entity, and one that copies the fact's sentence.
    let para = sk
        .questions
        .iter()
        .find(|q| q.category == "paraphrase")
        .unwrap();
    let rel = &para.relevant_notes[0].note;
    let ent = &first_of(&sk, |n| &n.id == rel).entities[0].name;
    let mut r = ok.clone();
    r.questions
        .insert(para.id.clone(), format!("What is the figure for {ent}?"));
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&para.id) && e.contains("must not name")));
    let mut r = ok.clone();
    r.questions.insert(
        para.id.clone(),
        para.basis[0].replace(ent.as_str(), "the thing"),
    );
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&para.id) && e.contains("content words")));

    // A non-paraphrase question that drops the name it must keep.
    let direct = sk
        .questions
        .iter()
        .find(|q| q.category == "direct")
        .unwrap();
    let mut r = ok.clone();
    r.questions
        .insert(direct.id.clone(), "What is the value?".into());
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&direct.id) && e.contains("must name")));

    // A negative made answerable: its fake entity appears in a note.
    let neg = sk
        .questions
        .iter()
        .find(|q| q.negative && q.markers.is_empty())
        .unwrap();
    let mut r = ok.clone();
    r.notes
        .get_mut(&n.id)
        .unwrap()
        .push_str(&format!("\n{} was mentioned.\n", neg.names[0]));
    assert!(check::check_rendered(&sk, &r)
        .errors
        .iter()
        .any(|e| e.contains(&neg.id) && e.contains("answerable")));

    // A passage fact pulled into the first chunk.
    let pas = sk
        .questions
        .iter()
        .find(|q| q.category == "passage")
        .unwrap();
    let ev = &pas.evidence[0];
    let mut r = ok.clone();
    let body = r.notes[&ev.note].clone();
    r.notes.insert(
        ev.note.clone(),
        body.replacen('\n', &format!("\n\n{}\n", ev.key_fact), 1),
    );
    // (the fact now also sits near the top, but the check looks at where its value first appears)
    let f = check::check_rendered(&sk, &r);
    assert!(
        f.errors.iter().any(|e| e.contains(&pas.id)),
        "{:?}",
        f.errors
    );

    // A missing note and a missing question wording.
    let mut r = ok.clone();
    r.notes.remove(&n.id);
    r.questions.remove(&direct.id);
    let f = check::check_rendered(&sk, &r);
    assert!(f
        .errors
        .iter()
        .any(|e| e.contains(&n.id) && e.contains("not rendered")));
    assert!(f
        .errors
        .iter()
        .any(|e| e.contains(&direct.id) && e.contains("not rendered")));
}

#[test]
fn check_dir_pins_the_structure_to_the_skeleton_and_flags_a_stale_render() {
    let g = generator::generate(&scenario()).unwrap();
    let text = g.skeleton.to_json();
    let d = tempfile::tempdir().unwrap();
    generator::write_fixture(
        d.path(),
        &g.skeleton,
        &g.template,
        &meta_for(&g.skeleton, &text, "m"),
    )
    .unwrap();
    assert!(generator::check_dir(d.path(), &g.skeleton, &text)
        .unwrap()
        .errors
        .is_empty());
    // Editing ground truth by hand is drift: only wording may change.
    let q = std::fs::read_to_string(d.path().join("questions.jsonl")).unwrap();
    std::fs::write(
        d.path().join("questions.jsonl"),
        q.replacen("\"gain\":2", "\"gain\":1", 1),
    )
    .unwrap();
    let f = generator::check_dir(d.path(), &g.skeleton, &text).unwrap();
    assert!(
        f.errors
            .iter()
            .any(|e| e.contains("questions.jsonl differs")),
        "{:?}",
        f.errors
    );
    std::fs::write(d.path().join("questions.jsonl"), &q).unwrap();
    // A render from an older skeleton is reported as stale.
    let f = generator::check_dir(d.path(), &g.skeleton, &(text.clone() + " ")).unwrap();
    assert!(
        f.errors.iter().any(|e| e.contains("re-render")),
        "{:?}",
        f.errors
    );
    // A hand-edited manifest is drift too.
    std::fs::write(d.path().join("manifest.json"), "{}").unwrap();
    let f = generator::check_dir(d.path(), &g.skeleton, &text).unwrap();
    assert!(f.errors.iter().any(|e| e.contains("manifest.json differs")));
}

#[test]
fn partial_render_runs_keep_the_recorded_model_and_prompt() {
    let g = generator::generate(&scenario()).unwrap();
    let text = g.skeleton.to_json();
    let prev = meta_for(&g.skeleton, &text, "model-a");
    // Same model and prompt version: fine. Different model: refused, naming both.
    let same = prev;
    let h = same.skeleton_hash.clone();
    assert!(render::check_partial_run_provenance(&same, "model-a", &h).is_ok());
    let e = render::check_partial_run_provenance(&same, "model-b", &h).unwrap_err();
    assert!(e.contains("model-a") && e.contains("model-b"), "{e}");
    let old_prompt = RenderMeta {
        prompt_version: "old-prompt".into(),
        ..same.clone()
    };
    let e = render::check_partial_run_provenance(&old_prompt, "model-a", &h).unwrap_err();
    assert!(e.contains("old-prompt"), "{e}");
    // A regenerated skeleton: the notes a partial run keeps would come from the old one.
    let e = render::check_partial_run_provenance(&same, "model-a", "0123456789abcdef").unwrap_err();
    assert!(
        e.contains("skeleton") && e.contains("without --resume"),
        "{e}"
    );
}

#[test]
fn resume_skips_questions_already_worded_and_only_rejects_unknown_ids() {
    let g = generator::generate(&scenario()).unwrap();
    // A resume with everything already rendered has nothing left, questions included.
    let (nc, qc) = render::plan_calls(
        &g.skeleton,
        &g.template,
        &render::Options {
            resume: true,
            ..opts()
        },
    );
    assert_eq!((nc, qc), (0, 0));
    // With only the notes present (a crash before questions were written) the question calls remain.
    let notes_only = Rendered {
        notes: g.template.notes.clone(),
        questions: Default::default(),
    };
    let (nc, qc) = render::plan_calls(
        &g.skeleton,
        &notes_only,
        &render::Options {
            resume: true,
            ..opts()
        },
    );
    assert_eq!(nc, 0);
    assert!(qc > 0);
    // --only: a known id is fine, a typo is an error naming it.
    let ok: BTreeSet<String> = [g.skeleton.notes[0].id.clone()].into_iter().collect();
    assert!(render::check_only_ids(&g.skeleton, &ok).is_ok());
    let bad: BTreeSet<String> = ["no-such-note".to_string()].into_iter().collect();
    let e = render::check_only_ids(&g.skeleton, &bad).unwrap_err();
    assert!(e.contains("no-such-note"), "{e}");
}

#[test]
fn a_decimal_value_stays_in_one_sentence_for_the_paraphrase_check() {
    // A rendered note that copies a decimal-valued fact sentence must still be caught.
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let Some((q, n, x)) = sk.questions.iter().find_map(|q| {
        if q.category != "paraphrase" {
            return None;
        }
        let rel = q.relevant_notes.first()?;
        let n = sk.note(&rel.note)?;
        let x = q
            .basis
            .first()
            .and_then(|b| n.facts.iter().find(|x| &x.statement == b))?;
        x.value.contains('.').then_some((q, n, x))
    }) else {
        return; // no decimal-valued paraphrase fact in this scenario
    };
    let mut r = check::load_rendered(template_dir()).unwrap();
    let body = r.notes.get(&n.id).unwrap().clone();
    r.notes
        .insert(n.id.clone(), format!("{body}\n\n{}\n", x.statement));
    r.questions.insert(q.id.clone(), x.statement.clone());
    let f = check::check_rendered(&sk, &r);
    assert!(
        f.errors.iter().any(|e| e.contains(&q.id)
            && (e.contains("sentence carrying the fact") || e.contains("content words"))),
        "{}",
        check::describe(&f)
    );
}

#[test]
fn a_bare_number_value_is_not_satisfied_by_an_unrelated_number() {
    // 7 is not in 2027, 7.4, 1,700, 17 or 3-7 style joins; it is in "7 engineers" and "(7)."
    assert!(!has_value("The budget year is 2027.", "7"));
    assert!(!has_value("It scored 7.4 overall.", "7"));
    assert!(!has_value("About 1,700 requests.", "7"));
    assert!(!has_value("Room 17 is booked.", "7"));
    assert!(!has_value("Tickets 3-7 are open.", "7"));
    assert!(has_value("The team has 7 engineers.", "7"));
    assert!(has_value("Rota size (7).", "7"));
    assert!(has_value("Ends with 7.", "7"));
    assert!(has_value("Score: 7.4 out of 10", "7.4"));
    assert!(has_value("A budget of $1,200 a month", "$1,200"));
    assert!(!has_value("A budget of $1,2000 a month", "$1,200"));
    assert!(has_value("Cadence is Weekly.", "weekly"));
    // The fact check uses it: a note that drops the value but carries 2027 is flagged.
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let mut r = check::load_rendered(template_dir()).unwrap();
    let (n, f) = sk
        .notes
        .iter()
        .find_map(|n| {
            n.facts
                .iter()
                .find(|f| f.value.len() == 1 && f.value.chars().all(|c| c.is_ascii_digit()))
                .map(|f| (n, f))
        })
        .expect("the skeleton has a single-digit value");
    let body = r.notes[&n.id].clone();
    let tweaked: String = body.replace(&f.value, "#");
    r.notes.insert(
        n.id.clone(),
        format!("{tweaked}\n\nThe plan runs into 2027 and 2037.\n"),
    );
    let found = check::check_rendered(&sk, &r);
    assert!(
        found
            .errors
            .iter()
            .any(|e| e.contains(&n.id) && e.contains("does not appear")),
        "{}",
        check::describe(&found)
    );
}

#[tokio::test]
async fn resume_after_a_crash_needs_the_recorded_model_and_skeleton_not_just_notes() {
    let g = generator::generate(&scenario()).unwrap();
    let text = g.skeleton.to_json();
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("skeleton.json"), &text).unwrap();
    std::fs::create_dir(d.path().join("notes")).unwrap();
    let id = &g.skeleton.notes[0].id;
    std::fs::write(d.path().join(format!("notes/{id}.md")), "# kept\n").unwrap();
    let dir = d.path().to_str().unwrap().to_string();
    let args = |model: &str| -> Vec<String> {
        ["--dir", &dir, "--resume", "--dry-run", "--model", model]
            .map(String::from)
            .to_vec()
    };
    // Notes exist but nothing records what produced them: refused.
    assert_eq!(render::run_cli(&args("model-a")).await, 1);
    // A crashed run leaves render.progress.json behind; the same model may resume...
    let progress = |model: &str, hash: &str| {
        json!({"render_model": model, "prompt_version": render::PROMPT_VERSION, "skeleton_hash": hash})
            .to_string()
    };
    let hash = skeleton_hash(&text);
    std::fs::write(
        d.path().join(render::PROGRESS_FILE),
        progress("model-a", &hash),
    )
    .unwrap();
    assert_eq!(render::run_cli(&args("model-a")).await, 0);
    // ...a different model may not, nor may a resume against a changed skeleton.
    assert_eq!(render::run_cli(&args("model-b")).await, 1);
    std::fs::write(
        d.path().join(render::PROGRESS_FILE),
        progress("model-a", "0123456789abcdef"),
    )
    .unwrap();
    assert_eq!(render::run_cli(&args("model-a")).await, 1);
}

#[test]
fn a_full_render_clears_earlier_notes_and_questions_and_a_short_journal_only_warns() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("notes")).unwrap();
    std::fs::write(d.path().join("notes/old.md"), "# old skeleton\n").unwrap();
    std::fs::write(d.path().join("notes/keep.txt"), "not a note").unwrap();
    std::fs::write(d.path().join("questions.jsonl"), "{}\n").unwrap();
    render::reset_for_full_run(d.path()).unwrap();
    assert!(!d.path().join("notes/old.md").exists() && !d.path().join("questions.jsonl").exists());
    assert!(d.path().join("notes/keep.txt").exists());
    // Idempotent on a clean directory.
    render::reset_for_full_run(d.path()).unwrap();
    render::reset_for_full_run(tempfile::tempdir().unwrap().path()).unwrap();

    // A journal that undershoots 5,000 words is a warning, not an error.
    let sk = Skeleton::from_json(&skeleton_text()).unwrap();
    let mut r = check::load_rendered(template_dir()).unwrap();
    let j = sk.notes.iter().find(|n| n.kind == "journal").unwrap();
    let body: String = r.notes[&j.id]
        .split_whitespace()
        .take(4900)
        .collect::<Vec<_>>()
        .join(" ");
    r.notes.insert(j.id.clone(), body);
    let f = check::check_rendered(&sk, &r);
    assert!(f
        .warnings
        .iter()
        .any(|w| w.contains(&j.id) && w.contains("5,000")));
    assert!(
        !f.errors
            .iter()
            .any(|e| e.contains(&j.id) && e.contains("5,000")),
        "{}",
        check::describe(&f)
    );
}
