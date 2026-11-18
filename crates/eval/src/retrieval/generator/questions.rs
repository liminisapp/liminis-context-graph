//! Question generation for retrieval-v2 (#788): ground truth is read straight off the fact model.
//!
//! Automatic checks enforced here (not left to review):
//! * paraphrase questions share at most [`PARAPHRASE_MAX_FACT_OVERLAP`] of their content words
//!   with the fact sentence and [`PARAPHRASE_MAX_NOTE_OVERLAP`] with the whole note;
//! * a negative's fake entity appears in no note, and a real entity asked about an attribute it
//!   never has never co-occurs with that attribute's marker words in any note;
//! * a passage question's fact sits in a chunk after the first of a multi-chunk note.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::notes::Corpus;
use super::vocab::*;
use super::world::*;
use super::Scenario;
use crate::retrieval::mapping::chunk_markdown;

/// Maximum share of a paraphrase question's content words found in the fact sentence.
pub const PARAPHRASE_MAX_FACT_OVERLAP: f64 = 0.25;
/// Maximum share found in the whole relevant note.
pub const PARAPHRASE_MAX_NOTE_OVERLAP: f64 = 0.5;

#[derive(Debug, Serialize)]
pub struct RelOut {
    pub note: String,
    pub gain: u32,
}
#[derive(Debug, Serialize)]
pub struct EvOut {
    pub note: String,
    pub key_fact: String,
}
#[derive(Debug, Serialize)]
pub struct QOut {
    pub id: String,
    pub category: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relevant_notes: Vec<RelOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub current: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub negative: bool,
}

const STOP: [&str; 40] = [
    "what", "which", "who", "whom", "when", "where", "how", "does", "that", "with", "deals",
    "works", "this", "from", "have", "been", "they", "their", "there", "about", "into", "over",
    "much", "many", "each", "every", "also", "other", "than", "then", "them", "were", "will",
    "would", "could", "should", "does", "work", "come", "call",
];

/// Lowercase alphanumeric words longer than three letters, minus glue words.
pub fn content_words(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3 && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}

/// Share of `q`'s content words that also appear in `doc`.
pub fn overlap(q: &str, doc: &str) -> f64 {
    let qw = content_words(q);
    if qw.is_empty() {
        return 0.0;
    }
    let dw = content_words(doc);
    qw.intersection(&dw).count() as f64 / qw.len() as f64
}

struct Ctx<'a> {
    w: &'a World,
    corpus: &'a Corpus,
    bodies: &'a BTreeMap<String, String>,
    out: Vec<QOut>,
    counters: BTreeMap<&'static str, usize>,
}

impl Ctx<'_> {
    fn next_id(&mut self, prefix: &'static str) -> String {
        let n = self.counters.entry(prefix).or_default();
        *n += 1;
        format!("{prefix}-{:02}", *n)
    }

    fn add(
        &mut self,
        prefix: &'static str,
        category: &'static str,
        text: String,
        rel: Vec<(String, u32)>,
    ) -> usize {
        let id = self.next_id(prefix);
        self.out.push(QOut {
            id,
            category,
            text,
            relevant_notes: rel
                .into_iter()
                .map(|(note, gain)| RelOut { note, gain })
                .collect(),
            evidence: vec![],
            current: vec![],
            stale: vec![],
            negative: false,
        });
        self.out.len() - 1
    }

    fn name(&self, ei: usize) -> &str {
        &self.w.entities[ei].name
    }
}

fn mentions(bodies: &BTreeMap<String, String>, needle: &str) -> bool {
    let n = needle.to_lowercase();
    bodies.values().any(|b| b.to_lowercase().contains(&n))
}

pub fn build(
    sc: &Scenario,
    w: &World,
    corpus: &Corpus,
    bodies: &BTreeMap<String, String>,
    root: &Rng,
) -> Result<Vec<QOut>, String> {
    let mut c = Ctx {
        w,
        corpus,
        bodies,
        out: Vec::new(),
        counters: BTreeMap::new(),
    };
    direct(&mut c, sc, root)?;
    paraphrase(&mut c, sc)?;
    temporal(&mut c, sc, root)?;
    answer(&mut c, sc)?;
    multi_hop(&mut c, sc, root)?;
    passage(&mut c, sc, root)?;
    negative(&mut c, sc)?;
    Ok(c.out)
}

fn direct(c: &mut Ctx, sc: &Scenario, root: &Rng) -> Result<(), String> {
    let mut r = root.fork("q-direct");
    let n_internal = sc.questions.direct.saturating_sub(sc.external_direct);
    for fi in c.w.pools.direct.iter().copied().take(n_internal) {
        let f = &c.w.facts[fi];
        let a = c.w.attr(f);
        let text = fill(a.qd[r.below(a.qd.len())], &[("e", c.name(f.entity))]);
        let mut rel = vec![(f.note.clone(), 2)];
        if let Some(recap) = c.corpus.recaps.get(&fi) {
            // A near-duplicate restatement is an acceptable (weaker) answer; the derived pages
            // that also restate this fact are deliberately not listed.
            rel.push((recap.clone(), 1));
        }
        c.add("direct", "direct", text, rel);
    }
    for x in c.w.ext_facts.iter().take(sc.external_direct) {
        if x.note.is_empty() {
            return Err("an external fact has no note; raise external_notes".into());
        }
        let text = format!(
            "What standard rate did the trade press report for {}?",
            c.w.entities[x.entity].name
        );
        c.add("direct", "direct", text, vec![(x.note.clone(), 2)]);
    }
    Ok(())
}

fn paraphrase(c: &mut Ctx, sc: &Scenario) -> Result<(), String> {
    let mut made = 0;
    let candidates: Vec<usize> =
        c.w.pools
            .paraphrase
            .iter()
            .chain(c.w.pools.reserve.iter())
            .copied()
            .collect();
    for fi in candidates {
        if made == sc.questions.paraphrase {
            break;
        }
        let f = &c.w.facts[fi];
        let e = &c.w.entities[f.entity];
        let a = c.w.attr(f);
        let text = fill(
            a.qp[0],
            &[("kq", KINDS[e.kind].q), ("dq", DOMAINS[e.domain].q)],
        );
        let note = &c.bodies[&f.note];
        if overlap(&text, &f.sentence) > PARAPHRASE_MAX_FACT_OVERLAP
            || overlap(&text, note) > PARAPHRASE_MAX_NOTE_OVERLAP
        {
            continue; // re-draw from the reserve
        }
        c.add("paraphrase", "paraphrase", text, vec![(f.note.clone(), 2)]);
        made += 1;
    }
    if made < sc.questions.paraphrase {
        return Err(format!(
            "only {made} of {} paraphrase questions satisfy the lexical-overlap bound",
            sc.questions.paraphrase
        ));
    }
    Ok(())
}

fn temporal(c: &mut Ctx, sc: &Scenario, root: &Rng) -> Result<(), String> {
    let _ = root;
    let total = sc.questions.temporal;
    let chains = c.w.chains.len();
    if chains == 0 && total > 0 {
        return Err("temporal questions need decision chains".into());
    }
    for i in 0..total {
        let ci = i % chains;
        let chain = &c.w.chains[ci];
        let topic = &CHAIN_TOPICS[chain.topic];
        let p = c.name(chain.project).to_string();
        let text = if i < chains {
            if ci.is_multiple_of(2) {
                format!("Which {} does {p} use now?", topic.label)
            } else {
                format!("What is the current {} for {p}?", topic.label)
            }
        } else {
            format!("As of the latest notes, which {} is {p} on?", topic.label)
        };
        let last = chain
            .steps
            .last()
            .expect("chains are non-empty")
            .note
            .clone();
        let stale: Vec<String> = chain.steps[..chain.steps.len() - 1]
            .iter()
            .map(|s| s.note.clone())
            .collect();
        let idx = c.add("temporal", "temporal", text, vec![(last.clone(), 2)]);
        c.out[idx].current = vec![last];
        c.out[idx].stale = stale;
    }
    Ok(())
}

fn answer(c: &mut Ctx, sc: &Scenario) -> Result<(), String> {
    for fi in c.w.pools.answer.iter().copied().take(sc.questions.answer) {
        let f = &c.w.facts[fi];
        let a = c.w.attr(f);
        let text = fill(a.qd[0], &[("e", c.name(f.entity))]);
        let mut rel = vec![(f.note.clone(), 2)];
        if let Some(ans) = c.corpus.answer_notes.get(&fi) {
            rel.push((ans.clone(), 1));
        }
        let idx = c.add("answer", "answer", text, rel);
        c.out[idx].evidence = vec![EvOut {
            note: f.note.clone(),
            key_fact: f.sentence.clone(),
        }];
    }
    Ok(())
}

fn multi_hop(c: &mut Ctx, sc: &Scenario, root: &Rng) -> Result<(), String> {
    let mut r = root.fork("q-multihop");
    let m = sc.questions.multi_hop;
    let n_a = m.div_ceil(2);
    // A: project → supplier (link fact) → supplier attribute.
    let vendor_attrs: Vec<usize> = (0..ATTRS.len()).filter(|i| ATTRS[*i].kind == 1).collect();
    let mut combos: Vec<(usize, usize)> = (0..12)
        .flat_map(|p| vendor_attrs.iter().map(move |a| (p, *a)))
        .collect();
    r.shuffle(&mut combos);
    for (p, ai) in combos.into_iter().take(n_a) {
        let pe = entity_index(0, p);
        let dep =
            c.w.fact_of(pe, "dependency")
                .ok_or("project without dependency")?;
        let vendor_name = c.w.facts[dep].value.clone();
        let ve =
            c.w.entities
                .iter()
                .position(|e| e.kind == 1 && e.name == vendor_name)
                .ok_or("dependency names an unknown supplier")?;
        let vf =
            c.w.fact_of(ve, ATTRS[ai].key)
                .ok_or("supplier without attribute")?;
        let text = fill(ATTRS[ai].qhop, &[("p", c.name(pe))]);
        let rel = vec![
            (c.w.facts[dep].note.clone(), 2),
            (c.w.facts[vf].note.clone(), 2),
        ];
        c.add("multihop", "multi-hop", text, rel);
    }
    // B: compare the monthly budgets of two projects.
    let mut pairs: Vec<(usize, usize)> = (0..12)
        .flat_map(|a| ((a + 1)..12).map(move |b| (a, b)))
        .collect();
    r.shuffle(&mut pairs);
    let mut made = 0;
    for (a, b) in pairs {
        if made == m - n_a {
            break;
        }
        let (ea, eb) = (entity_index(0, a), entity_index(0, b));
        let fa = c.w.fact_of(ea, "budget").ok_or("project without budget")?;
        let fb = c.w.fact_of(eb, "budget").ok_or("project without budget")?;
        if c.w.facts[fa].value == c.w.facts[fb].value {
            continue; // an answerless comparison
        }
        let text = if made % 2 == 0 {
            format!(
                "Which has the larger monthly budget, {} or {}?",
                c.name(ea),
                c.name(eb)
            )
        } else {
            format!(
                "Between {} and {}, which one gets the bigger monthly budget?",
                c.name(eb),
                c.name(ea)
            )
        };
        let rel = vec![
            (c.w.facts[fa].note.clone(), 2),
            (c.w.facts[fb].note.clone(), 2),
        ];
        c.add("multihop", "multi-hop", text, rel);
        made += 1;
    }
    if made < m - n_a {
        return Err("not enough distinct project pairs for the comparison questions".into());
    }
    Ok(())
}

/// Chunk index (0-based) holding `sentence`, with the note's chunk count.
pub fn chunk_of(body: &str, sentence: &str, max_tokens: usize) -> Option<(usize, usize)> {
    let chunks = chunk_markdown(body, max_tokens);
    let idx = chunks.iter().position(|c| c.contains(sentence))?;
    Some((idx, chunks.len()))
}

fn passage(c: &mut Ctx, sc: &Scenario, root: &Rng) -> Result<(), String> {
    let mut order: Vec<usize> = (0..c.w.log_facts.len()).collect();
    root.fork("q-passage").shuffle(&mut order);
    let mut made = 0;
    for li in order {
        if made == sc.questions.passage {
            break;
        }
        let lf = &c.w.log_facts[li];
        if lf.note.is_empty() {
            continue;
        }
        let Some((idx, n)) = chunk_of(&c.bodies[&lf.note], &lf.sentence, sc.chunk_max_tokens)
        else {
            continue;
        };
        if n < 2 || idx < 1 {
            continue; // the answer must need passage choice inside a multi-chunk note
        }
        let text = fill(LOG_ATTRS[lf.attr].qd, &[("e", c.name(lf.entity))]);
        let idx_q = c.add("passage", "passage", text, vec![(lf.note.clone(), 2)]);
        c.out[idx_q].evidence = vec![EvOut {
            note: lf.note.clone(),
            key_fact: lf.sentence.clone(),
        }];
        made += 1;
    }
    if made < sc.questions.passage {
        return Err(format!(
            "only {made} of {} passage questions have their fact beyond the first chunk of a multi-chunk note",
            sc.questions.passage
        ));
    }
    Ok(())
}

const FAKE_SUFFIX: [&str; 3] = ["Labs", "Works", "Holdings"];

fn negative(c: &mut Ctx, sc: &Scenario) -> Result<(), String> {
    let n = sc.questions.negative;
    let n_fake = n / 2;
    let mut texts: BTreeSet<String> = BTreeSet::new();
    let mut made = 0;
    // (a) an entity that does not exist, named like ones that do.
    let mut i = 0;
    while made < n_fake && i < 400 {
        let a = &ATTRS[(i * 7 + 3) % ATTRS.len()];
        let fake = format!("{} {}", WORDS[(i * 5 + 1) % 12], FAKE_SUFFIX[i % 3]);
        i += 1;
        if mentions(c.bodies, &fake) {
            continue;
        }
        let text = fill(a.qd[0], &[("e", &fake)]);
        if !texts.insert(text.clone()) {
            continue;
        }
        let idx = c.add("negative", "negative", text, vec![]);
        c.out[idx].negative = true;
        made += 1;
    }
    // (b) a real entity asked about an attribute its kind never has.
    let mut j = 0;
    while made < n && j < 2000 {
        let ei = (j * 7 + 3) % c.w.entities.len();
        let ai = (j * 5 + 1) % ATTRS.len();
        j += 1;
        let (e, a) = (&c.w.entities[ei], &ATTRS[ai]);
        if a.kind == e.kind {
            continue;
        }
        let body_hits = c.bodies.values().any(|b| {
            let lb = b.to_lowercase();
            lb.contains(&e.name.to_lowercase()) && a.neg.iter().any(|w| lb.contains(w))
        });
        if body_hits {
            continue;
        }
        let text = fill(a.qd[0], &[("e", &e.name)]);
        if !texts.insert(text.clone()) {
            continue;
        }
        let idx = c.add("negative", "negative", text, vec![]);
        c.out[idx].negative = true;
        made += 1;
    }
    if made < n {
        return Err(format!("could only build {made} of {n} verified negatives"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_words_drop_short_and_glue_words() {
        let w = content_words("What is the monthly budget of Cedar Core?");
        assert!(w.contains("monthly") && w.contains("budget") && w.contains("cedar"));
        assert!(!w.contains("what") && !w.contains("the"));
    }

    #[test]
    fn overlap_is_the_share_of_question_words_found_in_the_document() {
        assert_eq!(
            overlap("budget ceiling initiative", "The budget is large."),
            1.0 / 3.0
        );
        assert_eq!(overlap("???", "anything"), 0.0);
        assert_eq!(overlap("alpha beta", "gamma"), 0.0);
    }

    #[test]
    fn chunk_of_locates_a_sentence_after_the_first_chunk() {
        let body = format!(
            "# T\n\n{}\n\n## B\n\nTHE FACT SENTENCE.\n",
            "word ".repeat(1200)
        );
        let (idx, n) = chunk_of(&body, "THE FACT SENTENCE.", 100).unwrap();
        assert!(n >= 2 && idx >= 1);
        assert!(chunk_of(&body, "absent", 100).is_none());
    }
}
