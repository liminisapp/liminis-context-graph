//! Note skeletons for retrieval-v2 (#788). Every note is described from the fact model in
//! [`World`]: which facts it carries, its origin, citations, size and reference time.
//!
//! Each note is also given a *template body*. That prose is a **test double** for the LLM renderer:
//! it lets the whole pipeline (checks, loader, ingest, report) run in tests with no model, and it
//! is never written into the checked-in fixture. The checked-in notes are rendered by an LLM
//! ([`super::render`]).

use std::collections::BTreeMap;

use super::skeleton::*;
use super::vocab::*;
use super::world::*;
use super::Scenario;

#[derive(Debug, Clone)]
pub struct GenNote {
    pub sk: SkNote,
    pub day: Day,
    pub seq: usize,
    /// Template prose (test double only).
    pub body: String,
}

#[derive(Debug, Default)]
pub struct Corpus {
    pub notes: Vec<GenNote>,
    /// `(old note, new note, effective day)` — explicit supersessions only.
    pub supersedes: Vec<(String, String, Day)>,
    /// fact index → recap (near-duplicate primary) note id.
    pub recaps: BTreeMap<usize, String>,
    /// fact index → filed-answer note id.
    pub answer_notes: BTreeMap<usize, String>,
    /// entity index → derived page id.
    pub wiki_pages: BTreeMap<usize, String>,
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

pub fn entity_sk(e: &Entity) -> SkEntity {
    SkEntity {
        name: e.name.clone(),
        kind: KINDS[e.kind].slug.to_string(),
        kind_note: KINDS[e.kind].note.to_string(),
        kind_question: KINDS[e.kind].q.to_string(),
        domain_note: DOMAINS[e.domain].note.to_string(),
        domain_question: DOMAINS[e.domain].q.to_string(),
    }
}

fn fact_sk(w: &World, fi: usize, statement: String) -> SkFact {
    let f = &w.facts[fi];
    SkFact {
        entity: w.entities[f.entity].name.clone(),
        attribute: ATTRS[f.attr].label.to_string(),
        value: f.value.clone(),
        statement,
        section: None,
    }
}

struct Builder<'a> {
    notes: Vec<GenNote>,
    days: BTreeMap<String, Day>,
    ids: std::collections::BTreeSet<String>,
    root: &'a Rng,
}

/// Target word count for a note whose template body has `words` words: rounded to ten, never
/// below 40 (the LLM renderer aims for it within 25%).
fn target_for(words: usize) -> usize {
    (words.div_ceil(10) * 10).max(40)
}

/// Skeleton fields every note shares; the caller fills the rest.
fn base(id: &str, kind: &str, origin: &str, title: String, target_words: usize) -> SkNote {
    SkNote {
        id: id.to_string(),
        kind: kind.to_string(),
        origin: origin.to_string(),
        reference_time: String::new(),
        cites: vec![],
        target_words,
        sections: 1,
        title_hint: title,
        entities: vec![],
        facts: vec![],
        topic: None,
        explicit: None,
        effective_text: None,
        earlier_values: vec![],
    }
}

impl Builder<'_> {
    fn push(&mut self, mut sk: SkNote, day: Day, body: String) -> Result<String, String> {
        if !crate::retrieval::fixture::valid_note_id(&sk.id) {
            return Err(format!(
                "generated note id '{}' is not a lowercase slug",
                sk.id
            ));
        }
        if !self.ids.insert(sk.id.clone()) {
            return Err(format!("generated duplicate note id '{}'", sk.id));
        }
        sk.reference_time = rfc3339(day);
        let id = sk.id.clone();
        self.days.insert(id.clone(), day);
        let seq = self.notes.len();
        self.notes.push(GenNote { sk, day, seq, body });
        Ok(id)
    }

    fn day_of(&self, id: &str) -> Day {
        self.days.get(id).copied().unwrap_or(0)
    }
}

pub fn filler_sentence(r: &mut Rng) -> String {
    let t = *r.pick(&FILLER);
    fill(
        t,
        &[
            ("day", *r.pick(&DAYS)),
            ("adj", *r.pick(&ADJS)),
            ("noun", *r.pick(&NOUNS)),
            ("verb", *r.pick(&VERBS)),
            ("topic", *r.pick(&TOPICS)),
        ],
    )
}

pub fn paragraph(r: &mut Rng, lo: usize, hi: usize) -> String {
    let n = r.range(lo, hi);
    (0..n)
        .map(|_| filler_sentence(r))
        .collect::<Vec<_>>()
        .join(" ")
}

fn sorted_unique(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// Words per section of a long note (each section is rendered, and chunked, on its own).
pub const SECTION_WORDS: usize = 400;

pub fn build(sc: &Scenario, w: &mut World, root: &Rng) -> Result<Corpus, String> {
    let mut b = Builder {
        notes: Vec::new(),
        days: BTreeMap::new(),
        ids: Default::default(),
        root,
    };
    let mut corpus = Corpus::default();

    // ── primary fact notes: two per entity ────────────────────────────────────────
    for ei in 0..w.entities.len() {
        let e = w.entities[ei].clone();
        let mut r = b.root.fork(&format!("factnote|{}", e.name));
        let fis = w.by_entity[ei].clone();
        let split = fis.len().div_ceil(2);
        let groups = [fis[..split].to_vec(), fis[split..].to_vec()];
        let mut day = (14 + ei * 5 + r.below(5)) as Day;
        for (gi, group) in groups.iter().enumerate() {
            if group.is_empty() {
                continue;
            }
            let titles = [
                format!("# {}: working notes", e.name),
                format!("# Notes on {}", e.name),
                format!("# {} — status", e.name),
                format!("# {} — what we know", e.name),
            ];
            let intro = format!(
                "{} is our {} for {}.",
                e.name, KINDS[e.kind].note, DOMAINS[e.domain].note
            );
            let title = r.pick(&titles).clone();
            let mut body = format!("{title}\n\n{} {}\n\n", intro, filler_sentence(&mut r));
            let mut para = Vec::new();
            for (i, fi) in group.iter().enumerate() {
                para.push(w.facts[*fi].sentence.clone());
                if i + 1 < group.len() {
                    para.push(filler_sentence(&mut r));
                }
            }
            body.push_str(&para.join(" "));
            body.push_str("\n\n");
            body.push_str(&paragraph(&mut r, 1, 3));
            body.push('\n');
            let id = format!("{}-{}-{}", KINDS[e.kind].slug, e.slug, gi + 1);
            let mut sk = base(
                &id,
                "fact",
                "primary",
                title.trim_start_matches("# ").to_string(),
                target_for(word_count(&body).max(90)),
            );
            sk.entities = vec![entity_sk(&e)];
            sk.facts = group
                .iter()
                .map(|fi| fact_sk(w, *fi, w.facts[*fi].sentence.clone()))
                .collect();
            b.push(sk, day, body)?;
            for fi in group {
                w.facts[*fi].note = id.clone();
            }
            day += r.range(3, 30) as Day;
        }
    }

    // ── decision chains ───────────────────────────────────────────────────────────
    for ci in 0..w.chains.len() {
        let chain = w.chains[ci].clone();
        let topic = &CHAIN_TOPICS[chain.topic];
        let p = w.entities[chain.project].clone();
        let mut r = b.root.fork(&format!("chainnote|{ci}"));
        let mut prev: Option<String> = None;
        for (si, step) in chain.steps.iter().enumerate() {
            let reason = r.pick(&REASONS);
            let (title, opening, statement) = if si == 0 {
                (
                    format!("# Decision: {} for {}", topic.label, p.name),
                    format!(
                        "We have chosen {} as the {} for {}. {reason}",
                        step.value, topic.label, p.name
                    ),
                    format!("{} uses {} as its {}.", p.name, step.value, topic.label),
                )
            } else if step.explicit {
                (
                    format!("# Decision revised: {} for {}", topic.label, p.name),
                    format!(
                        "We are switching the {} for {} to {}. {reason} This replaces the earlier choice. The change takes effect on {}.",
                        topic.label,
                        p.name,
                        step.value,
                        long_date(step.effective_day)
                    ),
                    format!(
                        "{} switches its {} to {}.",
                        p.name, topic.label, step.value
                    ),
                )
            } else {
                (
                    format!("# Update: {} for {}", topic.label, p.name),
                    format!(
                        "{} now uses {} as its {}. {reason}",
                        p.name, step.value, topic.label
                    ),
                    format!("{} now uses {} as its {}.", p.name, step.value, topic.label),
                )
            };
            let body = format!("{title}\n\n{opening}\n\n{}\n", paragraph(&mut r, 1, 2));
            let id = format!("decision-{}-{}-{}", topic.slug, p.slug, si + 1);
            let mut sk = base(
                &id,
                "decision",
                "primary",
                title.trim_start_matches("# ").to_string(),
                target_for(word_count(&body)),
            );
            sk.entities = vec![entity_sk(&p)];
            sk.facts = vec![SkFact {
                entity: p.name.clone(),
                attribute: topic.label.to_string(),
                value: step.value.clone(),
                statement,
                section: None,
            }];
            sk.topic = Some(topic.label.to_string());
            sk.explicit = Some(si > 0 && step.explicit);
            if si > 0 && step.explicit {
                sk.effective_text = Some(long_date(step.effective_day));
            }
            if si > 0 && !step.explicit {
                sk.earlier_values = chain.steps[..si].iter().map(|s| s.value.clone()).collect();
            }
            b.push(sk, step.day, body)?;
            w.chains[ci].steps[si].note = id.clone();
            if step.explicit {
                if let Some(old) = &prev {
                    corpus
                        .supersedes
                        .push((old.clone(), id.clone(), step.effective_day));
                }
            }
            prev = Some(id);
        }
    }

    // ── recaps: near-duplicate primary notes restating a fact ─────────────────────
    let recap_n = sc.recaps.min(w.pools.direct.len());
    for (n, fi) in w.pools.direct.clone().into_iter().take(recap_n).enumerate() {
        let f = w.facts[fi].clone();
        let e = w.entities[f.entity].clone();
        let mut r = b.root.fork(&format!("recap|{fi}"));
        let day = b.day_of(&f.note) + r.range(7, 60) as Day;
        let restated = w.restated(fi);
        let body = format!(
            "# Standup recap\n\nQuick recap from the week: {} {}\n\n{}\n",
            restated,
            filler_sentence(&mut r),
            paragraph(&mut r, 1, 2)
        );
        let id = format!("recap-{:02}-{}", n + 1, e.slug);
        let mut sk = base(
            &id,
            "recap",
            "primary",
            "Standup recap".into(),
            target_for(word_count(&body)),
        );
        sk.entities = vec![entity_sk(&e)];
        sk.facts = vec![fact_sk(w, fi, restated)];
        b.push(sk, day, body)?;
        corpus.recaps.insert(fi, id);
    }

    // ── long notes with buried facts, in sections ─────────────────────────────────
    let mut log_order: Vec<usize> = (0..w.log_facts.len()).collect();
    b.root.fork("logorder").shuffle(&mut log_order);
    let huge_each = 5usize;
    let huge_total = (sc.long_notes.huge * huge_each).min(log_order.len());
    let (huge_facts, mid_facts) = log_order.split_at(huge_total);
    let mut assignments: Vec<Vec<usize>> = Vec::new(); // huge notes first, then mid
    for h in 0..sc.long_notes.huge {
        assignments.push(
            huge_facts
                .iter()
                .skip(h * huge_each)
                .take(huge_each)
                .copied()
                .collect(),
        );
    }
    let mut mid: Vec<Vec<usize>> = vec![Vec::new(); sc.long_notes.mid];
    if !mid.is_empty() {
        for (i, lf) in mid_facts.iter().enumerate() {
            mid[i % sc.long_notes.mid].push(*lf);
        }
    }
    assignments.extend(mid);
    for (li, logs) in assignments.iter().enumerate() {
        let huge = li < sc.long_notes.huge;
        let mut r = b.root.fork(&format!("long|{li}"));
        let target = if huge {
            r.range(5200, 6800)
        } else {
            r.range(520, 1900)
        };
        let day = r.range(59, 607) as Day;
        let n_sections = (target / SECTION_WORDS).max(1);
        let section_of: Vec<usize> = logs.iter().map(|_| r.below(n_sections)).collect();
        let title = *r.pick(&DIGEST_TITLES);
        let heading_title = format!("{title} — {}", long_date(day));
        let mut body = format!("# {heading_title}\n\n");
        for s in 0..n_sections {
            if s > 0 {
                body.push_str(&format!("## {}\n\n", r.pick(&HEADINGS)));
            }
            let mut paras: Vec<String> = Vec::new();
            let mut words = 0;
            while words < target / n_sections {
                let p = paragraph(&mut r, 3, 5);
                words += word_count(&p);
                paras.push(p);
            }
            for lf in logs
                .iter()
                .zip(&section_of)
                .filter(|(_, sec)| **sec == s)
                .map(|(lf, _)| lf)
            {
                let para = format!(
                    "{} {} {}",
                    filler_sentence(&mut r),
                    w.log_facts[*lf].sentence,
                    filler_sentence(&mut r)
                );
                let pos = r.range(0, paras.len());
                paras.insert(pos, para);
            }
            for p in paras {
                body.push_str(&p);
                body.push_str("\n\n");
            }
        }
        let id = if huge {
            format!("journal-{:02}", li + 1)
        } else {
            format!("digest-{:03}", li - sc.long_notes.huge + 1)
        };
        let mut sk = base(
            &id,
            if huge { "journal" } else { "digest" },
            "primary",
            heading_title,
            target,
        );
        sk.sections = n_sections;
        sk.facts = logs
            .iter()
            .zip(&section_of)
            .map(|(lf, s)| {
                let l = &w.log_facts[*lf];
                SkFact {
                    entity: w.entities[l.entity].name.clone(),
                    attribute: LOG_ATTRS[l.attr].key.to_string(),
                    value: l.value.clone(),
                    statement: l.sentence.clone(),
                    section: Some(*s),
                }
            })
            .collect();
        sk.entities = sorted_unique(
            logs.iter()
                .map(|lf| w.entities[w.log_facts[*lf].entity].name.clone())
                .collect(),
        )
        .into_iter()
        .filter_map(|n| w.entities.iter().find(|e| e.name == n).map(entity_sk))
        .collect();
        b.push(sk, day, body)?;
        for (lf, sec) in logs.iter().zip(&section_of) {
            w.log_facts[*lf].note = id.clone();
            w.log_facts[*lf].section = *sec;
        }
    }

    // ── external clippings ────────────────────────────────────────────────────────
    for n in 0..sc.external_notes {
        let mut r = b.root.fork(&format!("clip|{n}"));
        let (tslug, title) = CLIP_TOPICS[n % CLIP_TOPICS.len()];
        let target = if n % 5 < 3 {
            r.range(250, 480)
        } else {
            r.range(500, 900)
        };
        let day = r.range(20, 600) as Day;
        let mut paras: Vec<String> = Vec::new();
        let mut words = 0;
        while words < target {
            let k = r.range(3, 5);
            let p = (0..k)
                .map(|_| *r.pick(&CLIP_SENTENCES))
                .collect::<Vec<_>>()
                .join(" ");
            words += word_count(&p);
            paras.push(p);
        }
        let id = format!("clip-{:02}-{tslug}", n + 1);
        let mut sk = base(&id, "clip", "external", title.to_string(), target);
        if let Some(x) = w.ext_facts.get_mut(n) {
            let para = format!("{} The figure was not independently verified.", x.sentence);
            let pos = r.range(0, paras.len());
            paras.insert(pos, para);
            x.note = id.clone();
            let e = w.entities[x.entity].clone();
            sk.entities = vec![entity_sk(&e)];
            sk.facts = vec![SkFact {
                entity: e.name.clone(),
                attribute: "public list price".into(),
                value: x.value.clone(),
                statement: x.sentence.clone(),
                section: None,
            }];
        }
        let body = format!(
            "# {title}\n\n_Clipped from the web. Source: example.org/{tslug}._\n\n{}\n",
            paras.join("\n\n")
        );
        b.push(sk, day, body)?;
    }

    // ── derived entity pages (the wiki citing itself) ─────────────────────────────
    for ei in 0..w.entities.len() {
        let e = w.entities[ei].clone();
        let mut r = b.root.fork(&format!("wiki|{}", e.name));
        let mut restated: Vec<usize> = Vec::new();
        for (i, fi) in w.by_entity[ei].iter().enumerate() {
            if i < 2 || r.chance(sc.derived_restate_percent, 100) {
                restated.push(*fi);
            }
        }
        let cites = sorted_unique(
            restated
                .iter()
                .map(|fi| w.facts[*fi].note.clone())
                .collect(),
        );
        let day = cites.iter().map(|c| b.day_of(c)).max().unwrap_or(0) + r.range(3, 40) as Day;
        let mut body = format!(
            "# {} — summary\n\n_Agent-maintained page. Facts below were collected from the notes listed under Sources._\n\n{} is our {} for {}.\n\n",
            e.name,
            e.name,
            KINDS[e.kind].note,
            DOMAINS[e.domain].note
        );
        for fi in &restated {
            let f = &w.facts[*fi];
            body.push_str(&format!("- {}: {}\n", ATTRS[f.attr].label, f.value));
        }
        body.push_str(&format!(
            "\nSources: {}\n",
            cites
                .iter()
                .map(|c| format!("[[{c}]]"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let id = format!("wiki-{}", e.slug);
        let mut sk = base(
            &id,
            "wiki",
            "derived",
            format!("{} — summary", e.name),
            target_for(word_count(&body)),
        );
        sk.cites = cites;
        sk.entities = vec![entity_sk(&e)];
        sk.facts = restated
            .iter()
            .map(|fi| fact_sk(w, *fi, w.facts[*fi].sentence.clone()))
            .collect();
        b.push(sk, day, body)?;
        corpus.wiki_pages.insert(ei, id);
    }

    // ── derived pages over decision chains; every second one lags (restates a stale value) ──
    for ci in 0..sc.derived_chain_pages.min(w.chains.len()) {
        let chain = w.chains[ci].clone();
        let topic = &CHAIN_TOPICS[chain.topic];
        let p = w.entities[chain.project].clone();
        let lag = ci % 2 == 1 && chain.steps.len() >= 2;
        let step = &chain.steps[if lag {
            chain.steps.len() - 2
        } else {
            chain.steps.len() - 1
        }];
        let mut r = b.root.fork(&format!("chainwiki|{ci}"));
        let day = b.day_of(&step.note) + r.range(3, 20) as Day;
        let body = format!(
            "# {} for {} — summary\n\n_Agent-maintained page._\n\nCurrent setting: {}.\n\nSources: [[{}]]\n",
            topic.label, p.name, step.value, step.note
        );
        let id = format!("wiki-{}-{}", topic.slug, p.slug);
        let mut sk = base(
            &id,
            "wiki",
            "derived",
            format!("{} for {} — summary", topic.label, p.name),
            target_for(word_count(&body)),
        );
        sk.cites = vec![step.note.clone()];
        sk.entities = vec![entity_sk(&p)];
        sk.topic = Some(topic.label.to_string());
        sk.facts = vec![SkFact {
            entity: p.name.clone(),
            attribute: topic.label.to_string(),
            value: step.value.clone(),
            statement: format!("Current {} for {}: {}.", topic.label, p.name, step.value),
            section: None,
        }];
        b.push(sk, day, body)?;
    }

    // ── filed answers ─────────────────────────────────────────────────────────────
    for (n, fi) in w
        .pools
        .answer
        .clone()
        .into_iter()
        .take(sc.answer_notes)
        .enumerate()
    {
        let f = w.facts[fi].clone();
        let e = w.entities[f.entity].clone();
        let a = &ATTRS[f.attr];
        let mut r = b.root.fork(&format!("answer|{fi}"));
        let day = b.day_of(&f.note) + r.range(2, 30) as Day;
        let q = fill(a.qd[a.qd.len() - 1], &[("e", &e.name)]);
        let restated = w.restated(fi);
        let body = format!(
            "# Q&A: {} — {}\n\n**Question:** Remind me, {}\n\n**Answer:** {} (from [[{}]])\n",
            e.name,
            a.label.to_lowercase(),
            lowercase_first(&q),
            restated,
            f.note
        );
        let id = format!("answer-{:02}-{}-{}", n + 1, e.slug, a.key);
        let mut sk = base(
            &id,
            "answer",
            "answer",
            format!("Q&A: {} — {}", e.name, a.label.to_lowercase()),
            target_for(word_count(&body)),
        );
        sk.cites = vec![f.note.clone()];
        sk.entities = vec![entity_sk(&e)];
        sk.facts = vec![fact_sk(w, fi, restated)];
        b.push(sk, day, body)?;
        corpus.answer_notes.insert(fi, id);
    }

    corpus.notes = b.notes;
    Ok(corpus)
}

fn lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
