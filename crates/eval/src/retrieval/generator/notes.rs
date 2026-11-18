//! Note rendering for the retrieval-v2 generator (#788). Every note is rendered from the fact
//! model in [`World`]; nothing here invents a fact.

use std::collections::BTreeMap;

use super::vocab::*;
use super::world::*;
use super::Scenario;

#[derive(Debug, Clone)]
pub struct GenNote {
    pub id: String,
    pub day: Day,
    pub seq: usize,
    pub origin: &'static str,
    pub cites: Vec<String>,
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
    /// derived-page ids per entity index (restating pages).
    pub wiki_pages: BTreeMap<usize, String>,
}

pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

struct Builder<'a> {
    notes: Vec<GenNote>,
    days: BTreeMap<String, Day>,
    ids: std::collections::BTreeSet<String>,
    root: &'a Rng,
}

impl Builder<'_> {
    fn push(
        &mut self,
        id: String,
        day: Day,
        origin: &'static str,
        cites: Vec<String>,
        body: String,
    ) -> Result<String, String> {
        if !valid_slug(&id) {
            return Err(format!("generated note id '{id}' is not a lowercase slug"));
        }
        if !self.ids.insert(id.clone()) {
            return Err(format!("generated duplicate note id '{id}'"));
        }
        self.days.insert(id.clone(), day);
        let seq = self.notes.len();
        self.notes.push(GenNote {
            id: id.clone(),
            day,
            seq,
            origin,
            cites,
            body,
        });
        Ok(id)
    }

    fn day_of(&self, id: &str) -> Day {
        self.days.get(id).copied().unwrap_or(0)
    }
}

fn valid_slug(id: &str) -> bool {
    crate::retrieval::fixture::valid_note_id(id)
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
            let mut body = format!(
                "{}\n\n{} {}\n\n",
                r.pick(&titles),
                intro,
                filler_sentence(&mut r)
            );
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
            b.push(id.clone(), day, "primary", vec![], body)?;
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
            let (title, opening) = if si == 0 {
                (
                    format!("# Decision: {} for {}", topic.label, p.name),
                    format!(
                        "We have chosen {} as the {} for {}. {reason}",
                        step.value, topic.label, p.name
                    ),
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
                )
            } else {
                (
                    format!("# Update: {} for {}", topic.label, p.name),
                    format!(
                        "{} now uses {} as its {}. {reason}",
                        p.name, step.value, topic.label
                    ),
                )
            };
            let body = format!("{title}\n\n{opening}\n\n{}\n", paragraph(&mut r, 1, 2));
            let id = format!("decision-{}-{}-{}", topic.slug, p.slug, si + 1);
            b.push(id.clone(), step.day, "primary", vec![], body)?;
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
        let body = format!(
            "# Standup recap\n\nQuick recap from the week: {} {}\n\n{}\n",
            w.restated(fi),
            filler_sentence(&mut r),
            paragraph(&mut r, 1, 2)
        );
        let id = format!("recap-{:02}-{}", n + 1, e.slug);
        b.push(id.clone(), day, "primary", vec![], body)?;
        corpus.recaps.insert(fi, id);
    }

    // ── long notes with buried facts ──────────────────────────────────────────────
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
        let mut paras: Vec<String> = Vec::new();
        let mut words = 0;
        while words < target {
            let p = paragraph(&mut r, 3, 5);
            words += word_count(&p);
            paras.push(p);
        }
        for lf in logs {
            let para = format!(
                "{} {} {}",
                filler_sentence(&mut r),
                w.log_facts[*lf].sentence,
                filler_sentence(&mut r)
            );
            let pos = r.range(0, paras.len());
            paras.insert(pos, para);
        }
        let title = *r.pick(&DIGEST_TITLES);
        let mut body = format!("# {title} — {}\n\n", long_date(day));
        let mut since_heading = 0;
        for (pi, p) in paras.iter().enumerate() {
            if pi > 0 && since_heading >= 400 {
                body.push_str(&format!("## {}\n\n", r.pick(&HEADINGS)));
                since_heading = 0;
            }
            since_heading += word_count(p);
            body.push_str(p);
            body.push_str("\n\n");
        }
        let id = if huge {
            format!("journal-{:02}", li + 1)
        } else {
            format!("digest-{:03}", li - sc.long_notes.huge + 1)
        };
        b.push(id.clone(), day, "primary", vec![], body)?;
        for lf in logs {
            w.log_facts[*lf].note = id.clone();
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
        let ext = w.ext_facts.get_mut(n);
        let id = format!("clip-{:02}-{tslug}", n + 1);
        if let Some(x) = ext {
            let para = format!("{} The figure was not independently verified.", x.sentence);
            let pos = r.range(0, paras.len());
            paras.insert(pos, para);
            x.note = id.clone();
        }
        let body = format!(
            "# {title}\n\n_Clipped from the web. Source: example.org/{tslug}._\n\n{}\n",
            paras.join("\n\n")
        );
        b.push(id, day, "external", vec![], body)?;
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
        b.push(id.clone(), day, "derived", cites, body)?;
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
        b.push(id, day, "derived", vec![step.note.clone()], body)?;
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
        let body = format!(
            "# Q&A: {} — {}\n\n**Question:** Remind me, {}\n\n**Answer:** {} (from [[{}]])\n",
            e.name,
            a.label.to_lowercase(),
            lowercase_first(&q),
            w.restated(fi),
            f.note
        );
        let id = format!("answer-{:02}-{}-{}", n + 1, e.slug, a.key);
        b.push(id.clone(), day, "answer", vec![f.note.clone()], body)?;
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
