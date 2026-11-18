//! Question skeletons for retrieval-v2 (#788): ground truth is read straight off the fact model.
//! The wording is rendered by an LLM later; here every question gets a plain `text_hint`, the
//! names its wording must keep, and what the check step must be able to verify.
//!
//! Checks made while the skeleton is built (they depend only on the fact model, never on prose):
//! * a paraphrase question's hint shares at most [`PARAPHRASE_MAX_FACT_OVERLAP`] of its content
//!   words with the canonical fact statement (the rendered wording is re-checked, with the note
//!   text, by [`super::check`]);
//! * a negative's fake entity matches no entity and appears in no fact statement; a real entity is
//!   only asked about an attribute its kind never has;
//! * a passage question's fact sits in a section after the first of a multi-section note, so its
//!   chunk is never the note's first.

use std::collections::BTreeSet;

use super::notes::Corpus;
use super::skeleton::*;
use super::vocab::*;
use super::world::*;
use super::Scenario;

/// Maximum share of a paraphrase question's content words found in the fact sentence.
pub const PARAPHRASE_MAX_FACT_OVERLAP: f64 = 0.25;
/// Maximum share found in the whole relevant note.
pub const PARAPHRASE_MAX_NOTE_OVERLAP: f64 = 0.5;

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
    out: Vec<SkQuestion>,
    counters: std::collections::BTreeMap<&'static str, usize>,
}

impl Ctx<'_> {
    fn add(
        &mut self,
        prefix: &'static str,
        category: &'static str,
        intent: String,
        text_hint: String,
        rel: Vec<(String, u32)>,
    ) -> usize {
        let n = self.counters.entry(prefix).or_default();
        *n += 1;
        let id = format!("{prefix}-{:02}", *n);
        self.out.push(SkQuestion {
            id,
            category: category.to_string(),
            intent,
            text_hint,
            relevant_notes: rel
                .into_iter()
                .map(|(note, gain)| SkRel { note, gain })
                .collect(),
            evidence: vec![],
            current: vec![],
            stale: vec![],
            negative: false,
            names: vec![],
            descriptor: None,
            basis: vec![],
            markers: vec![],
        });
        self.out.len() - 1
    }

    fn name(&self, ei: usize) -> &str {
        &self.w.entities[ei].name
    }
}

/// How a paraphrase question refers to an entity without naming it.
pub fn descriptor(kind: usize, domain_question: &str) -> String {
    match kind {
        0 => format!("the initiative that deals with {domain_question}"),
        1 => format!("the outside provider that deals with {domain_question}"),
        2 => format!("the employee who works on {domain_question}"),
        _ => format!("the rule governing {domain_question}"),
    }
}

pub fn build(
    sc: &Scenario,
    w: &World,
    corpus: &Corpus,
    root: &Rng,
) -> Result<Vec<SkQuestion>, String> {
    let mut c = Ctx {
        w,
        corpus,
        out: Vec::new(),
        counters: Default::default(),
    };
    direct(&mut c, sc, root)?;
    paraphrase(&mut c, sc)?;
    temporal(&mut c, sc)?;
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
        let name = c.name(f.entity).to_string();
        let text = fill(a.qd[r.below(a.qd.len())], &[("e", &name)]);
        let mut rel = vec![(f.note.clone(), 2)];
        if let Some(recap) = c.corpus.recaps.get(&fi) {
            // A near-duplicate restatement is an acceptable (weaker) answer; the derived pages
            // that also restate this fact are deliberately not listed.
            rel.push((recap.clone(), 1));
        }
        let idx = c.add(
            "direct",
            "direct",
            format!("Ask for the {} of {name}.", a.label.to_lowercase()),
            text,
            rel,
        );
        c.out[idx].names = vec![name];
        c.out[idx].basis = vec![f.sentence.clone()];
    }
    for x in c.w.ext_facts.iter().take(sc.external_direct) {
        if x.note.is_empty() {
            return Err("an external fact has no note; raise external_notes".into());
        }
        let name = c.w.entities[x.entity].name.clone();
        let text = format!("What standard rate did the trade press report for {name}?");
        let idx = c.add(
            "direct",
            "direct",
            format!("Ask what standard rate the trade press reported for {name} (a clipped article's figure)."),
            text,
            vec![(x.note.clone(), 2)],
        );
        c.out[idx].names = vec![name];
        c.out[idx].basis = vec![x.sentence.clone()];
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
        if overlap(&text, &f.sentence) > PARAPHRASE_MAX_FACT_OVERLAP {
            continue; // re-draw from the reserve
        }
        let desc = descriptor(e.kind, DOMAINS[e.domain].q);
        let idx = c.add(
            "paraphrase",
            "paraphrase",
            format!(
                "Ask for the {} of {desc} without naming it and without reusing the wording of the statement.",
                a.label.to_lowercase()
            ),
            text,
            vec![(f.note.clone(), 2)],
        );
        c.out[idx].descriptor = Some(desc);
        c.out[idx].basis = vec![f.sentence.clone()];
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

fn temporal(c: &mut Ctx, sc: &Scenario) -> Result<(), String> {
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
        let last = chain.steps.last().expect("chains are non-empty");
        let stale: Vec<String> = chain.steps[..chain.steps.len() - 1]
            .iter()
            .map(|s| s.note.clone())
            .collect();
        let last_note = last.note.clone();
        let basis = format!("{p} now uses {} as its {}.", last.value, topic.label);
        let idx = c.add(
            "temporal",
            "temporal",
            format!(
                "Ask which {} {p} uses now; the decision was revised {} times, so older notes are stale.",
                topic.label,
                chain.steps.len() - 1
            ),
            text,
            vec![(last_note.clone(), 2)],
        );
        c.out[idx].current = vec![last_note];
        c.out[idx].stale = stale;
        c.out[idx].names = vec![p];
        c.out[idx].basis = vec![basis];
    }
    Ok(())
}

fn answer(c: &mut Ctx, sc: &Scenario) -> Result<(), String> {
    for fi in c.w.pools.answer.iter().copied().take(sc.questions.answer) {
        let f = &c.w.facts[fi];
        let a = c.w.attr(f);
        let name = c.name(f.entity).to_string();
        let text = fill(a.qd[0], &[("e", &name)]);
        let mut rel = vec![(f.note.clone(), 2)];
        if let Some(ans) = c.corpus.answer_notes.get(&fi) {
            rel.push((ans.clone(), 1));
        }
        let idx = c.add(
            "answer",
            "answer",
            format!(
                "Ask for the {} of {name}; a grounded answer must state the fact.",
                a.label.to_lowercase()
            ),
            text,
            rel,
        );
        c.out[idx].evidence = vec![SkEv {
            note: f.note.clone(),
            key_fact: f.sentence.clone(),
        }];
        c.out[idx].names = vec![name];
        c.out[idx].basis = vec![f.sentence.clone()];
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
        let pname = c.name(pe).to_string();
        let text = fill(ATTRS[ai].qhop, &[("p", &pname)]);
        let rel = vec![
            (c.w.facts[dep].note.clone(), 2),
            (c.w.facts[vf].note.clone(), 2),
        ];
        let basis = vec![
            c.w.facts[dep].sentence.clone(),
            c.w.facts[vf].sentence.clone(),
        ];
        let idx = c.add(
            "multihop",
            "multi-hop",
            format!(
                "Ask for the {} of the supplier that {pname} depends on; needs the note naming the supplier and the supplier's own note.",
                ATTRS[ai].label.to_lowercase()
            ),
            text,
            rel,
        );
        c.out[idx].names = vec![pname];
        c.out[idx].basis = basis;
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
        let (na, nb) = (c.name(ea).to_string(), c.name(eb).to_string());
        let text = if made % 2 == 0 {
            format!("Which has the larger monthly budget, {na} or {nb}?")
        } else {
            format!("Between {nb} and {na}, which one gets the bigger monthly budget?")
        };
        let rel = vec![
            (c.w.facts[fa].note.clone(), 2),
            (c.w.facts[fb].note.clone(), 2),
        ];
        let basis = vec![
            c.w.facts[fa].sentence.clone(),
            c.w.facts[fb].sentence.clone(),
        ];
        let idx = c.add(
            "multihop",
            "multi-hop",
            format!("Ask which of {na} and {nb} has the larger monthly budget; needs both budget notes."),
            text,
            rel,
        );
        c.out[idx].names = vec![na, nb];
        c.out[idx].basis = basis;
        made += 1;
    }
    if made < m - n_a {
        return Err("not enough distinct project pairs for the comparison questions".into());
    }
    Ok(())
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
        // A fact in the first section shares a chunk with the note's start; a later section is
        // rendered under its own heading and is therefore a later chunk.
        if lf.note.is_empty() || lf.section < 1 {
            continue;
        }
        let name = c.name(lf.entity).to_string();
        let text = fill(LOG_ATTRS[lf.attr].qd, &[("e", &name)]);
        let idx = c.add(
            "passage",
            "passage",
            format!(
                "Ask: \"{text}\" The answer is one paragraph (section {}) of a long note.",
                lf.section + 1
            ),
            text,
            vec![(lf.note.clone(), 2)],
        );
        c.out[idx].evidence = vec![SkEv {
            note: lf.note.clone(),
            key_fact: lf.sentence.clone(),
        }];
        c.out[idx].names = vec![name];
        c.out[idx].basis = vec![lf.sentence.clone()];
        made += 1;
    }
    if made < sc.questions.passage {
        return Err(format!(
            "only {made} of {} passage questions have their fact beyond the first section of a long note",
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
    let statements: Vec<String> = c
        .corpus
        .notes
        .iter()
        .flat_map(|n| n.sk.facts.iter().map(|f| f.statement.to_lowercase()))
        .collect();
    // Per note: its canonical statements and the names of its entities, lowercased.
    let note_texts: Vec<String> = c
        .corpus
        .notes
        .iter()
        .map(|n| {
            let mut t: Vec<String> =
                n.sk.facts
                    .iter()
                    .map(|f| f.statement.to_lowercase())
                    .collect();
            t.extend(n.sk.entities.iter().map(|e| e.name.to_lowercase()));
            t.join(" ")
        })
        .collect();
    let mut made = 0;
    // (a) an entity that does not exist, named like ones that do.
    let mut i = 0;
    while made < n_fake && i < 400 {
        let a = &ATTRS[(i * 7 + 3) % ATTRS.len()];
        let fake = format!("{} {}", WORDS[(i * 5 + 1) % 12], FAKE_SUFFIX[i % 3]);
        i += 1;
        let lf = fake.to_lowercase();
        if c.w.entities.iter().any(|e| e.name.to_lowercase() == lf)
            || statements.iter().any(|s| s.contains(&lf))
        {
            continue;
        }
        let text = fill(a.qd[0], &[("e", &fake)]);
        if !texts.insert(text.clone()) {
            continue;
        }
        let idx = c.add(
            "negative",
            "negative",
            format!(
                "Ask for the {} of '{fake}', which exists nowhere in the notebook (it is named like real entities).",
                a.label.to_lowercase()
            ),
            text,
            vec![],
        );
        c.out[idx].negative = true;
        c.out[idx].names = vec![fake];
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
        // The entity must never share a note with the attribute's marker words (checked against
        // the notes' canonical statements and entities, so it does not depend on any prose).
        let name_lc = e.name.to_lowercase();
        if note_texts
            .iter()
            .any(|t| t.contains(&name_lc) && a.neg.iter().any(|m| t.contains(m)))
        {
            continue;
        }
        let text = fill(a.qd[0], &[("e", &e.name)]);
        if !texts.insert(text.clone()) {
            continue;
        }
        let (name, label) = (e.name.clone(), a.label.to_lowercase());
        let markers: Vec<String> = a.neg.iter().map(|m| m.to_string()).collect();
        let idx = c.add(
            "negative",
            "negative",
            format!("Ask for the {label} of {name}, an attribute this kind of thing never has in the notebook."),
            text,
            vec![],
        );
        c.out[idx].negative = true;
        c.out[idx].names = vec![name];
        c.out[idx].markers = markers;
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
    fn descriptors_name_the_kind_and_the_domain_but_not_the_entity() {
        let d = descriptor(2, "stock tracking");
        assert!(d.contains("employee") && d.contains("stock tracking"));
    }
}
