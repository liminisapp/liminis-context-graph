//! The fact model: entities, attribute facts, buried log facts, decision chains and question
//! pools. It is the single source of truth — notes render facts, and questions and ground truth
//! are derived from the same objects, so evidence, negatives and currency are correct by
//! construction (ADR-0788).

use super::vocab::*;
use super::Scenario;
use crate::retrieval::stats::{fnv1a, SplitMix64};

/// Deterministic random stream. `fork` derives an independent stream from a label without
/// consuming this one, so reordering generator phases never perturbs another phase's output.
#[derive(Debug, Clone)]
pub struct Rng {
    seed: u64,
    inner: SplitMix64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            inner: SplitMix64::new(seed),
        }
    }
    pub fn fork(&self, label: &str) -> Rng {
        Rng::new(self.seed ^ fnv1a(label))
    }
    pub fn below(&mut self, n: usize) -> usize {
        self.inner.below(n)
    }
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        self.inner.range(lo, hi)
    }
    pub fn chance(&mut self, num: usize, den: usize) -> bool {
        self.inner.chance(num, den)
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            let j = self.below(i + 1);
            xs.swap(i, j);
        }
    }
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn fill(template: &str, pairs: &[(&str, &str)]) -> String {
    let mut s = template.to_string();
    for (k, v) in pairs {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Day number since 2025-01-01.
pub type Day = i64;

pub fn date_of(day: Day) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2025, 1, 1).expect("valid") + chrono::Duration::days(day)
}

/// RFC 3339 timestamp at 09:00 UTC.
pub fn rfc3339(day: Day) -> String {
    format!("{}T09:00:00Z", date_of(day).format("%Y-%m-%d"))
}

/// "14 March 2026".
pub fn long_date(day: Day) -> String {
    use chrono::Datelike;
    let d = date_of(day);
    format!("{} {} {}", d.day(), MONTHS[d.month0() as usize], d.year())
}

fn money(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    format!("${out}")
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub kind: usize,
    pub domain: usize,
    pub name: String,
    pub slug: String,
}

#[derive(Debug, Clone)]
pub struct Fact {
    pub entity: usize,
    /// Index into [`ATTRS`].
    pub attr: usize,
    pub value: String,
    /// Which statement template this fact's own sentence used.
    pub stmt: usize,
    pub sentence: String,
    /// Id of the note that states `sentence`; set when notes are built.
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct LogFact {
    pub entity: usize,
    pub attr: usize,
    pub value: String,
    pub sentence: String,
    pub note: String,
}

/// An external (clipped-article) fact: a vendor's public list price.
#[derive(Debug, Clone)]
pub struct ExtFact {
    pub entity: usize,
    pub value: String,
    pub sentence: String,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct ChainStep {
    pub value: String,
    pub day: Day,
    /// Recorded as a manifest supersession of the previous step (false for step 0 and for plain
    /// later notes that merely contradict the earlier one).
    pub explicit: bool,
    pub effective_day: Day,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct Chain {
    pub topic: usize,
    pub project: usize,
    pub steps: Vec<ChainStep>,
}

#[derive(Debug, Clone, Default)]
pub struct Pools {
    pub direct: Vec<usize>,
    pub paraphrase: Vec<usize>,
    pub answer: Vec<usize>,
    /// Unused facts, drawn on when a paraphrase candidate fails the lexical-overlap bound.
    pub reserve: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct World {
    pub entities: Vec<Entity>,
    pub facts: Vec<Fact>,
    pub by_entity: Vec<Vec<usize>>,
    pub log_facts: Vec<LogFact>,
    pub ext_facts: Vec<ExtFact>,
    pub chains: Vec<Chain>,
    pub pools: Pools,
}

pub fn person_name(d: usize) -> String {
    format!("{} {}", FIRST[d % 12], LAST[(d * 5 + 2) % 12])
}

fn entity_name(kind: usize, d: usize) -> String {
    if kind == 2 {
        person_name(d)
    } else {
        format!("{} {}", WORDS[(d + 3 * kind) % 12], NAME_SUFFIX[kind])
    }
}

/// Index of the entity of `kind` with domain `d`.
pub fn entity_index(kind: usize, d: usize) -> usize {
    kind * 12 + d
}

pub fn draw_value(val: Val, rng: &mut Rng, self_entity: Option<&Entity>) -> String {
    match val {
        Val::Money(lo, hi) => {
            let n = rng.range(lo as usize, hi as usize) as u32;
            // Round larger sums so they read like budgets.
            money(if n >= 1000 { n / 50 * 50 } else { n })
        }
        Val::Count(lo, hi) => rng.range(lo as usize, hi as usize).to_string(),
        Val::Month => format!(
            "{} {}",
            rng.pick(&MONTHS),
            if rng.chance(1, 2) { 2026 } else { 2027 }
        ),
        Val::Date => format!(
            "{} {} {}",
            rng.range(1, 28),
            rng.pick(&MONTHS),
            if rng.chance(1, 2) { 2026 } else { 2027 }
        ),
        Val::Person => loop {
            let n = person_name(rng.below(12));
            if self_entity.is_none_or(|e| e.name != n) {
                break n;
            }
        },
        Val::City => rng.pick(&CITIES).to_string(),
        Val::Cadence => rng.pick(&CADENCES).to_string(),
        Val::Percent => rng
            .pick(&["99.0%", "99.5%", "99.9%", "99.95%", "98.5%"])
            .to_string(),
        Val::Vendor => entity_name(1, rng.below(12)),
        Val::Score => format!("{}.{}", rng.range(3, 9), rng.below(10)),
    }
}

/// Statement text for a fact with its `stmt` template index (wraps).
pub fn render_stmt(attr: &AttrDef, stmt: usize, e: &Entity, v: &str) -> String {
    let t = attr.stmts[stmt % attr.stmts.len()];
    fill(t, &[("e", &e.name), ("v", v)])
}

impl World {
    pub fn build(sc: &Scenario, root: &Rng) -> Result<World, String> {
        let mut entities = Vec::new();
        for kind in 0..4 {
            for d in 0..12 {
                let name = entity_name(kind, d);
                entities.push(Entity {
                    kind,
                    domain: d,
                    slug: slug(&name),
                    name,
                });
            }
        }

        let mut facts = Vec::new();
        let mut by_entity = vec![Vec::new(); entities.len()];
        for (ei, e) in entities.iter().enumerate() {
            for (ai, a) in ATTRS.iter().enumerate().filter(|(_, a)| a.kind == e.kind) {
                let mut r = root.fork(&format!("fact|{}|{}", e.name, a.key));
                let value = if a.key == "dependency" {
                    // A distinct supplier per project, so "the supplier of {p}" is unambiguous.
                    entity_name(1, (e.domain * 5 + 1) % 12)
                } else {
                    draw_value(a.val, &mut r, Some(e))
                };
                let stmt = r.below(a.stmts.len());
                let sentence = render_stmt(a, stmt, e, &value);
                by_entity[ei].push(facts.len());
                facts.push(Fact {
                    entity: ei,
                    attr: ai,
                    value,
                    stmt,
                    sentence,
                    note: String::new(),
                });
            }
        }

        // Buried facts: projects and persons × log attributes.
        let mut log_facts = Vec::new();
        for (ei, e) in entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == 0 || e.kind == 2)
        {
            for (li, l) in LOG_ATTRS.iter().enumerate() {
                let mut r = root.fork(&format!("log|{}|{}", e.name, l.key));
                let value = draw_value(l.val, &mut r, Some(e));
                let t = l.stmts[0];
                log_facts.push(LogFact {
                    entity: ei,
                    attr: li,
                    sentence: fill(t, &[("e", &e.name), ("v", &value)]),
                    value,
                    note: String::new(),
                });
            }
        }

        // External list prices, one per vendor.
        let mut ext_facts = Vec::new();
        for (ei, e) in entities.iter().enumerate().filter(|(_, e)| e.kind == 1) {
            let mut r = root.fork(&format!("ext|{}", e.name));
            let value = draw_value(Val::Money(18, 140), &mut r, Some(e));
            ext_facts.push(ExtFact {
                entity: ei,
                sentence: format!(
                    "Trade press reported that {} lists its standard rate at {value} per unit.",
                    e.name
                ),
                value,
                note: String::new(),
            });
        }

        // Decision chains over projects.
        let mut combos: Vec<(usize, usize)> = (0..12)
            .flat_map(|p| (0..CHAIN_TOPICS.len()).map(move |t| (p, t)))
            .collect();
        let mut r = root.fork("chains");
        r.shuffle(&mut combos);
        if sc.chains > combos.len() {
            return Err(format!(
                "scenario asks for {} chains, at most {} fit",
                sc.chains,
                combos.len()
            ));
        }
        let mut chains = Vec::new();
        for (ci, (p, t)) in combos.into_iter().take(sc.chains).enumerate() {
            let mut cr = root.fork(&format!("chain|{ci}"));
            let topic = &CHAIN_TOPICS[t];
            let len = 3 + (ci * 5) % 3;
            let mut values: Vec<&str> = topic.values.to_vec();
            cr.shuffle(&mut values);
            let mut day: Day = cr.range(59, 211) as Day;
            let mut steps: Vec<ChainStep> = Vec::new();
            for (si, value) in values.iter().enumerate().take(len) {
                if si > 0 {
                    day += cr.range(28, 75) as Day;
                }
                let explicit = if si == 0 {
                    false
                } else if si == len - 1 {
                    ci.is_multiple_of(2)
                } else {
                    cr.chance(1, 2)
                };
                // Effective date: with the recording, a little before it (retroactive), or after.
                let effective_day = match (ci + si) % 3 {
                    0 => day,
                    1 => (day - cr.range(3, 14) as Day).max(steps.last().map_or(0, |s| s.day) + 1),
                    _ => day + cr.range(5, 20) as Day,
                };
                steps.push(ChainStep {
                    value: value.to_string(),
                    day,
                    explicit,
                    effective_day,
                    note: String::new(),
                });
            }
            chains.push(Chain {
                topic: t,
                project: entity_index(0, p),
                steps,
            });
        }

        // Question pools over non-relation facts, shuffled, disjoint by construction.
        let mut eligible: Vec<usize> = (0..facts.len())
            .filter(|i| ATTRS[facts[*i].attr].key != "dependency")
            .collect();
        let mut pr = root.fork("pools");
        pr.shuffle(&mut eligible);
        let n_direct = sc.questions.direct.saturating_sub(sc.external_direct);
        let need = n_direct + sc.questions.paraphrase + sc.questions.answer;
        if need > eligible.len() {
            return Err(format!(
                "scenario needs {need} distinct facts for direct/paraphrase/answer, only {} exist",
                eligible.len()
            ));
        }
        let pools = Pools {
            direct: eligible[..n_direct].to_vec(),
            paraphrase: eligible[n_direct..n_direct + sc.questions.paraphrase].to_vec(),
            answer: eligible[n_direct + sc.questions.paraphrase..need].to_vec(),
            reserve: eligible[need..].to_vec(),
        };

        Ok(World {
            entities,
            facts,
            by_entity,
            log_facts,
            ext_facts,
            chains,
            pools,
        })
    }

    pub fn attr(&self, f: &Fact) -> &'static AttrDef {
        &ATTRS[f.attr]
    }

    /// The fact of `entity` with attribute `key`.
    pub fn fact_of(&self, entity: usize, key: &str) -> Option<usize> {
        self.by_entity[entity]
            .iter()
            .copied()
            .find(|i| ATTRS[self.facts[*i].attr].key == key)
    }

    /// The same fact worded with a different template than its own.
    pub fn restated(&self, fi: usize) -> String {
        let f = &self.facts[fi];
        render_stmt(
            &ATTRS[f.attr],
            f.stmt + 1,
            &self.entities[f.entity],
            &f.value,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lowercase_ids() {
        assert_eq!(slug("Cedar Core"), "cedar-core");
        assert_eq!(slug("  Ines  Varga!! "), "ines-varga");
        assert!(crate::retrieval::fixture::valid_note_id(&slug(
            "Orbit Policy"
        )));
    }

    #[test]
    fn dates_format_as_rfc3339_and_prose() {
        assert_eq!(rfc3339(0), "2025-01-01T09:00:00Z");
        assert_eq!(long_date(59), "1 March 2025");
    }

    #[test]
    fn forks_are_independent_of_each_other_and_of_consumption() {
        let root = Rng::new(7);
        let mut a1 = root.fork("a");
        let mut b = root.fork("b");
        let mut a2 = root.fork("a");
        let _ = b.below(100);
        assert_eq!(a1.below(1_000_000), a2.below(1_000_000));
    }

    #[test]
    fn shuffle_is_a_deterministic_permutation() {
        let mut v: Vec<usize> = (0..20).collect();
        Rng::new(3).shuffle(&mut v);
        let mut w: Vec<usize> = (0..20).collect();
        Rng::new(3).shuffle(&mut w);
        assert_eq!(v, w);
        v.sort();
        assert_eq!(v, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn money_has_thousands_separators() {
        assert_eq!(money(5250), "$5,250");
        assert_eq!(money(37), "$37");
    }
}
