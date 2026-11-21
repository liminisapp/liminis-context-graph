//! The graph question set for the retrieval-v2 fixture (#821, FR-008..FR-012):
//! `graph-questions.jsonl`, generated from the skeleton.
//!
//! The E13 part is a port of the maintainer's `gen-graph-questions.py` (probe branch
//! `maintainer/e13-probe-files` @ `3e1f17f2`): the same families, wording, gold answers and hop
//! records, the same selection (one shared `random.Random(788)`, 12 per family) and the same
//! dev/test split (`random.Random(815)`: per family, in order of first appearance, shuffle the ids
//! and put the first half, rounded up, in dev). [`super::pyrandom::PyRandom`] reproduces Python's
//! `shuffle`, so the output matches the reference question for question.
//!
//! The **inverse** family is new: questions whose answer is the *source* side of a relation given
//! its target, where direction matters. It is generated after the E13 families from its own RNGs
//! (`Random(821)` for selection, a fresh `Random(815)` for its split), so the E13 part is unchanged
//! by construction. It avoids the E13 inverse questions it would otherwise duplicate (`owned-by`
//! covers every owner; `reports` every manager with two or more reports).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::pyrandom::PyRandom;
use super::skeleton::Skeleton;

/// The committed file, beside `skeleton.json`.
pub const FILE: &str = "graph-questions.jsonl";
/// Questions per E13 family (`CAP` in the reference).
pub const E13_CAP: usize = 12;
/// Questions per inverse sub-family.
pub const INVERSE_CAP: usize = 5;
pub const E13_SEED: u64 = 788;
pub const SPLIT_SEED: u64 = 815;
pub const INVERSE_SEED: u64 = 821;

/// One fact a complete answer rests on: `notes` are every primary note stating that value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopRecord {
    pub entity: String,
    pub attribute: String,
    pub value: String,
    pub notes: Vec<String>,
}

/// One step of the expected relation path from the question's named entity (FR-012). `relation`
/// and `direction` are `None` for history questions, which follow no single relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathStep {
    pub attribute: String,
    pub relation: Option<String>,
    pub direction: Option<String>,
}

/// One graph question (FR-011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphQuestion {
    pub id: String,
    /// `e13` (the 129 reference questions) or `inverse`.
    #[serde(default = "default_set")]
    pub set: String,
    pub family: String,
    #[serde(default)]
    pub cq: String,
    /// `chain-2`, `chain-3`, `list`, `join`, `history` or `inverse`.
    pub shape: String,
    pub text: String,
    /// Gold answers; ordered for `history`.
    pub answers: Vec<String>,
    /// `single` (first answer matches), `set` (set F1) or `ordered` (exact in-order match).
    pub scoring: String,
    /// `dev` or `test`.
    pub split: String,
    #[serde(default)]
    pub hops: Vec<HopRecord>,
    #[serde(default)]
    pub path: Vec<PathStep>,
}

fn default_set() -> String {
    "e13".into()
}

/// The scoring mode the reference `score()` applies to an E13 shape.
pub fn scoring_for_shape(shape: &str) -> &'static str {
    match shape {
        "history" => "ordered",
        "list" | "join" => "set",
        _ => "single",
    }
}

/// The ontology-v3.1 relation that carries a skeleton attribute, and its direction from the fact's
/// subject (the entity the attribute belongs to) to its value.
fn relation_of(attr: &str) -> Option<(&'static str, &'static str)> {
    Some(match attr {
        "Supplier" => ("SUPPLIED_BY", "out"),
        "Account manager" => ("CONTACT_FOR", "in"),
        "Owner" => ("OWNS", "in"),
        "Reports to" => ("REPORTS_TO", "out"),
        "Location" => ("LOCATED_IN", "out"),
        "Exception approver" => ("APPROVES", "in"),
        "Uptime SLA" | "Contract price" | "Renewal" => ("HAS_TERM", "out"),
        _ => return None,
    })
}

fn flip(d: &str) -> &'static str {
    if d == "out" {
        "in"
    } else {
        "out"
    }
}

/// The expected path for `attrs` (in hop order from the subject). `reverse` walks it from the
/// value side, as inverse and join questions do.
fn path_of(attrs: &[&str], reverse: bool) -> Vec<PathStep> {
    let mut uniq: Vec<&str> = Vec::new();
    for a in attrs {
        if !uniq.contains(a) {
            uniq.push(a);
        }
    }
    if reverse {
        uniq.reverse();
    }
    uniq.into_iter()
        .map(|a| {
            let r = relation_of(a);
            PathStep {
                attribute: a.to_string(),
                relation: r.map(|(n, _)| n.to_string()),
                direction: r.map(|(_, d)| if reverse { flip(d) } else { d }.to_string()),
            }
        })
        .collect()
}

/// A question before ids and splits are assigned.
struct Draft {
    family: &'static str,
    cq: &'static str,
    text: String,
    answers: Vec<String>,
    hops: Vec<HopRecord>,
    shape: &'static str,
    scoring: &'static str,
    path: Vec<PathStep>,
}

/// The skeleton's facts as the reference reads them: primary notes only.
struct Facts<'a> {
    /// (entity, attribute) → [(reference_time, value, note id)] in note order.
    stated: BTreeMap<(&'a str, &'a str), Vec<(&'a str, &'a str, &'a str)>>,
}

impl<'a> Facts<'a> {
    fn new(sk: &'a Skeleton) -> Self {
        let mut stated: BTreeMap<(&str, &str), Vec<(&str, &str, &str)>> = BTreeMap::new();
        for n in &sk.notes {
            if n.origin != "primary" {
                continue;
            }
            for f in &n.facts {
                stated
                    .entry((f.entity.as_str(), f.attribute.as_str()))
                    .or_default()
                    .push((n.reference_time.as_str(), f.value.as_str(), n.id.as_str()));
            }
        }
        Facts { stated }
    }

    fn sorted(&self, e: &str, a: &str) -> Vec<(&'a str, &'a str, &'a str)> {
        let mut xs = self.stated.get(&(e, a)).cloned().unwrap_or_default();
        xs.sort();
        xs
    }

    /// The current value: the latest primary note's (decisions supersede).
    fn cur(&self, e: &str, a: &str) -> Option<&'a str> {
        self.sorted(e, a).last().map(|x| x.1)
    }

    /// `cur` through an optional subject.
    fn cur2(&self, e: Option<&str>, a: &str) -> Option<&'a str> {
        e.and_then(|e| self.cur(e, a))
    }

    fn notes_stating(&self, e: &str, a: &str, v: &str) -> Vec<String> {
        let set: BTreeSet<&str> = self
            .stated
            .get(&(e, a))
            .map(|xs| xs.iter().filter(|x| x.1 == v).map(|x| x.2).collect())
            .unwrap_or_default();
        set.into_iter().map(str::to_string).collect()
    }

    fn hop(&self, e: &str, a: &str) -> HopRecord {
        let v = self.cur(e, a).unwrap_or_default();
        HopRecord {
            entity: e.to_string(),
            attribute: a.to_string(),
            value: v.to_string(),
            notes: self.notes_stating(e, a, v),
        }
    }

    /// The values in time order, consecutive repeats collapsed.
    fn history(&self, e: &str, a: &str) -> Vec<&'a str> {
        let seq: Vec<&str> = self.sorted(e, a).into_iter().map(|x| x.1).collect();
        let mut out: Vec<&str> = Vec::new();
        for (i, v) in seq.iter().enumerate() {
            if i == 0 || *v != seq[i - 1] {
                out.push(v);
            }
        }
        out
    }

    fn inverse<'b>(&self, attr: &str, subjects: &[&'b str], obj: &str) -> Vec<&'b str> {
        subjects
            .iter()
            .copied()
            .filter(|s| self.cur(s, attr) == Some(obj))
            .collect()
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `sorted(set(answers))` (non-history questions).
fn sorted_set(v: Vec<String>) -> Vec<String> {
    v.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn draft(
    family: &'static str,
    cq: &'static str,
    text: String,
    answers: Vec<String>,
    hops: Vec<HopRecord>,
    shape: &'static str,
    path: Vec<PathStep>,
) -> Draft {
    let answers = if shape == "history" {
        answers
    } else {
        sorted_set(answers)
    };
    Draft {
        family,
        cq,
        text,
        answers,
        hops,
        shape,
        scoring: scoring_for_shape(shape),
        path,
    }
}

/// `fam()`: shuffle with the shared RNG, keep the first `cap`.
fn fam(out: &mut Vec<Draft>, rng: &mut PyRandom, cap: usize, mut items: Vec<Draft>) {
    rng.shuffle(&mut items);
    out.extend(items.into_iter().take(cap));
}

fn e13(sk: &Skeleton, f: &Facts) -> Vec<Draft> {
    let by_kind = |k: &str| -> Vec<&str> {
        sk.entities
            .iter()
            .filter(|e| e.kind == k)
            .map(|e| e.name.as_str())
            .collect()
    };
    let (p_, v_, h_, pol) = (
        by_kind("project"),
        by_kind("vendor"),
        by_kind("person"),
        by_kind("policy"),
    );
    let mut rng = PyRandom::new(E13_SEED);
    let mut q: Vec<Draft> = Vec::new();
    const S: &str = "Supplier";
    const AM: &str = "Account manager";
    const OW: &str = "Owner";
    const RT: &str = "Reports to";
    const LO: &str = "Location";

    // --- chains (2 and 3 hops) ---
    let items = p_
        .iter()
        .filter_map(|p| {
            let s = f.cur(p, S)?;
            let am = f.cur(s, AM)?;
            Some(draft(
                "supplier-contact",
                "A4",
                format!("Who is the account manager at the supplier for {p}?"),
                vec![am.into()],
                vec![f.hop(p, S), f.hop(s, AM)],
                "chain-2",
                path_of(&[S, AM], false),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);

    let terms: [(&str, &str); 3] = [
        (
            "Uptime SLA",
            "What uptime does the supplier for {p} commit to?",
        ),
        (
            "Contract price",
            "What is the contract price agreed with {p}'s supplier?",
        ),
        (
            "Renewal",
            "When does the contract with {p}'s supplier renew?",
        ),
    ];
    let mut items = Vec::new();
    for p in &p_ {
        for (a, t) in terms {
            let Some(s) = f.cur(p, S) else { continue };
            let Some(v) = f.cur(s, a) else { continue };
            items.push(draft(
                "supplier-term",
                "A5",
                t.replace("{p}", p),
                vec![v.into()],
                vec![f.hop(p, S), f.hop(s, a)],
                "chain-2",
                path_of(&[S, a], false),
            ));
        }
    }
    fam(&mut q, &mut rng, E13_CAP, items);

    let items = p_
        .iter()
        .filter_map(|p| {
            let o = f.cur(p, OW)?;
            let m = f.cur(o, RT)?;
            Some(draft(
                "owner-manager",
                "R1+R2",
                format!("Who does the owner of {p} report to?"),
                vec![m.into()],
                vec![f.hop(p, OW), f.hop(o, RT)],
                "chain-2",
                path_of(&[OW, RT], false),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);

    let items = p_
        .iter()
        .filter_map(|p| {
            let o = f.cur(p, OW)?;
            let l = f.cur(o, LO)?;
            Some(draft(
                "owner-location",
                "R1+L1",
                format!("Where is the owner of {p} based?"),
                vec![l.into()],
                vec![f.hop(p, OW), f.hop(o, LO)],
                "chain-2",
                path_of(&[OW, LO], false),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);

    let items = p_
        .iter()
        .filter_map(|p| {
            let s = f.cur(p, S)?;
            let am = f.cur(s, AM)?;
            let l = f.cur(am, LO)?;
            Some(draft(
                "supplier-contact-location",
                "A4+L1",
                format!("Where is the account manager for {p}'s supplier based?"),
                vec![l.into()],
                vec![f.hop(p, S), f.hop(s, AM), f.hop(am, LO)],
                "chain-3",
                path_of(&[S, AM, LO], false),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);

    let items = p_
        .iter()
        .filter_map(|p| {
            let o = f.cur(p, OW)?;
            let m = f.cur(o, RT)?;
            let l = f.cur(m, LO)?;
            Some(draft(
                "owner-manager-location",
                "R1+R2+L1",
                format!("Where is the manager of {p}'s owner based?"),
                vec![l.into()],
                vec![f.hop(p, OW), f.hop(o, RT), f.hop(m, LO)],
                "chain-3",
                path_of(&[OW, RT, LO], false),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);

    // --- inverse / aggregation ---
    let inv_list = |family: &'static str,
                    cq: &'static str,
                    attr: &'static str,
                    subjects: &[&str],
                    objects: &[&str],
                    text: &dyn Fn(&str) -> String,
                    min: usize|
     -> Vec<Draft> {
        objects
            .iter()
            .filter_map(|h| {
                let xs = f.inverse(attr, subjects, h);
                if xs.len() < min.max(1) {
                    return None;
                }
                Some(draft(
                    family,
                    cq,
                    text(h),
                    strings(&xs),
                    xs.iter().map(|x| f.hop(x, attr)).collect(),
                    "list",
                    path_of(&[attr], true),
                ))
            })
            .collect()
    };
    let items = inv_list(
        "owned-by",
        "R1 inverse",
        OW,
        &p_,
        &h_,
        &|h| format!("Which projects does {h} own?"),
        1,
    );
    fam(&mut q, &mut rng, E13_CAP, items);
    let items = inv_list(
        "supplied-by",
        "A1 inverse",
        S,
        &p_,
        &v_,
        &|v| format!("Which projects does {v} supply?"),
        1,
    );
    fam(&mut q, &mut rng, E13_CAP, items);
    let items = inv_list(
        "approver-of",
        "P2 inverse",
        "Exception approver",
        &pol,
        &h_,
        &|h| format!("Which policies can {h} approve exceptions to?"),
        1,
    );
    fam(&mut q, &mut rng, E13_CAP, items);
    let items = inv_list(
        "reports",
        "R2 inverse",
        RT,
        &h_,
        &h_,
        &|h| format!("Who reports to {h}?"),
        2,
    );
    fam(&mut q, &mut rng, E13_CAP, items);

    // --- joins ---
    let cities: BTreeSet<&str> = h_.iter().filter_map(|h| f.cur(h, LO)).collect();
    let items = cities
        .iter()
        .filter_map(|c| {
            let ps: Vec<&str> = p_
                .iter()
                .copied()
                .filter(|p| f.cur2(f.cur(p, OW), LO) == Some(c))
                .collect();
            if ps.is_empty() {
                return None;
            }
            let hops = ps
                .iter()
                .flat_map(|p| [f.hop(p, OW), f.hop(f.cur(p, OW).unwrap_or_default(), LO)])
                .collect();
            Some(draft(
                "projects-by-owner-city",
                "R1+L1 join",
                format!("Which projects are owned by someone based in {c}?"),
                strings(&ps),
                hops,
                "join",
                path_of(&[OW, LO], true),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);
    let items = h_
        .iter()
        .filter_map(|h| {
            let ps: Vec<&str> = p_
                .iter()
                .copied()
                .filter(|p| f.cur2(f.cur(p, S), AM) == Some(h))
                .collect();
            if ps.is_empty() {
                return None;
            }
            let hops = ps
                .iter()
                .flat_map(|p| [f.hop(p, S), f.hop(f.cur(p, S).unwrap_or_default(), AM)])
                .collect();
            Some(draft(
                "projects-by-supplier-contact",
                "A1+A4 join",
                format!("For which projects is {h} the account manager at the supplier?"),
                strings(&ps),
                hops,
                "join",
                path_of(&[S, AM], true),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);
    let items = p_
        .iter()
        .filter_map(|p| {
            let s = f.cur(p, S)?;
            let others: Vec<&str> = p_
                .iter()
                .copied()
                .filter(|o| o != p && f.cur(o, S) == Some(s))
                .collect();
            if others.is_empty() {
                return None;
            }
            let mut all = vec![*p];
            all.extend(others.iter().copied());
            Some(draft(
                "shared-supplier",
                "A1 shared",
                format!("Which other projects use the same supplier as {p}?"),
                strings(&others),
                all.iter().map(|x| f.hop(x, S)).collect(),
                "join",
                path_of(&[S], false),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, E13_CAP, items);

    // --- change over time ---
    let changing: BTreeSet<&str> = f
        .stated
        .keys()
        .filter(|(e, a)| {
            sk.entities
                .iter()
                .any(|x| x.name == *e && x.kind == "project")
                && f.history(e, a).len() >= 3
        })
        .map(|(_, a)| *a)
        .collect();
    let mut items = Vec::new();
    for p in &p_ {
        for a in &changing {
            let hist = f.history(p, a);
            if hist.len() < 3 {
                continue;
            }
            let hops = hist
                .iter()
                .map(|v| HopRecord {
                    entity: p.to_string(),
                    attribute: a.to_string(),
                    value: v.to_string(),
                    notes: f.notes_stating(p, a, v),
                })
                .collect();
            items.push(draft(
                "change-history",
                "D1+D2",
                format!("What has {p}'s {a} been, in order, and what is it now?"),
                strings(&hist),
                hops,
                "history",
                vec![PathStep {
                    attribute: a.to_string(),
                    relation: None,
                    direction: None,
                }],
            ));
        }
    }
    fam(&mut q, &mut rng, E13_CAP, items);
    q
}

/// The inverse-direction family (FR-010): the source side of `REPORTS_TO`, `CONTACT_FOR` and
/// `OWNS` given the target, none of which E13 asks.
fn inverse_family(sk: &Skeleton, f: &Facts) -> Vec<Draft> {
    let by_kind = |k: &str| -> Vec<&str> {
        sk.entities
            .iter()
            .filter(|e| e.kind == k)
            .map(|e| e.name.as_str())
            .collect()
    };
    let (p_, v_, h_) = (by_kind("project"), by_kind("vendor"), by_kind("person"));
    let mut rng = PyRandom::new(INVERSE_SEED);
    let mut q = Vec::new();
    let mk = |family, cq, text: String, answers: Vec<String>, hops, scoring, path| Draft {
        family,
        cq,
        text,
        answers: sorted_set(answers),
        hops,
        shape: "inverse",
        scoring,
        path,
    };

    // A manager with exactly one report: a single-answer inverse (E13 `reports` asks only of
    // managers with two or more).
    let items = h_
        .iter()
        .filter_map(|h| {
            let xs = f.inverse("Reports to", &h_, h);
            (xs.len() == 1).then(|| {
                mk(
                    "reports-to-single",
                    "R2 inverse",
                    format!("Who reports to {h}?"),
                    strings(&xs),
                    xs.iter().map(|x| f.hop(x, "Reports to")).collect(),
                    "single",
                    path_of(&["Reports to"], true),
                )
            })
        })
        .collect();
    fam(&mut q, &mut rng, INVERSE_CAP, items);

    // Which suppliers a person is the account manager for (the source side of CONTACT_FOR).
    let items = h_
        .iter()
        .filter_map(|h| {
            let xs = f.inverse("Account manager", &v_, h);
            (!xs.is_empty()).then(|| {
                mk(
                    "account-manager-of",
                    "A4 inverse",
                    format!("Which suppliers is {h} the account manager for?"),
                    strings(&xs),
                    xs.iter().map(|x| f.hop(x, "Account manager")).collect(),
                    "set",
                    path_of(&["Account manager"], true),
                )
            })
        })
        .collect();
    fam(&mut q, &mut rng, INVERSE_CAP, items);

    // Projects owned by someone who reports to X: REPORTS_TO inverse, then OWNS from the person.
    let items = h_
        .iter()
        .filter_map(|h| {
            let ps: Vec<&str> = p_
                .iter()
                .copied()
                .filter(|p| f.cur2(f.cur(p, "Owner"), "Reports to") == Some(h))
                .collect();
            if ps.is_empty() {
                return None;
            }
            let hops = ps
                .iter()
                .flat_map(|p| {
                    [
                        f.hop(p, "Owner"),
                        f.hop(f.cur(p, "Owner").unwrap_or_default(), "Reports to"),
                    ]
                })
                .collect();
            Some(mk(
                "projects-of-reports",
                "R1+R2 inverse",
                format!("Which projects are owned by someone who reports to {h}?"),
                strings(&ps),
                hops,
                "set",
                path_of(&["Owner", "Reports to"], true),
            ))
        })
        .collect();
    fam(&mut q, &mut rng, INVERSE_CAP, items);
    q
}

/// The dev ids: per family in order of first appearance, shuffle its ids with the shared
/// `Random(815)` and take the first half, rounded up.
fn dev_ids(qs: &[(String, &'static str)]) -> BTreeSet<String> {
    let mut order: Vec<&str> = Vec::new();
    let mut by_family: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (id, fam) in qs {
        if !by_family.contains_key(fam) {
            order.push(fam);
        }
        by_family.entry(fam).or_default().push(id.clone());
    }
    let mut rng = PyRandom::new(SPLIT_SEED);
    let mut dev = BTreeSet::new();
    for fam in order {
        let mut ids = by_family.remove(fam).unwrap_or_default();
        rng.shuffle(&mut ids);
        let n = ids.len().div_ceil(2);
        dev.extend(ids.into_iter().take(n));
    }
    dev
}

fn finish(drafts: Vec<Draft>, prefix: &str, set: &str) -> Result<Vec<GraphQuestion>, String> {
    let ids: Vec<(String, &'static str)> = drafts
        .iter()
        .enumerate()
        .map(|(i, d)| (format!("{prefix}{i:03}"), d.family))
        .collect();
    let dev = dev_ids(&ids);
    drafts
        .into_iter()
        .zip(ids)
        .map(|(d, (id, _))| {
            if d.answers.is_empty() || d.answers.iter().any(|a| a.is_empty()) {
                return Err(format!(
                    "graph question {id} ({}) has an empty gold answer",
                    d.family
                ));
            }
            Ok(GraphQuestion {
                split: if dev.contains(&id) { "dev" } else { "test" }.into(),
                id,
                set: set.into(),
                family: d.family.into(),
                cq: d.cq.into(),
                shape: d.shape.into(),
                text: d.text,
                answers: d.answers,
                scoring: d.scoring.into(),
                hops: d.hops,
                path: d.path,
            })
        })
        .collect()
}

/// The full graph question set: the E13 questions (`g000`…), then the inverse family (`i000`…).
pub fn generate(sk: &Skeleton) -> Result<Vec<GraphQuestion>, String> {
    let f = Facts::new(sk);
    let mut out = finish(e13(sk, &f), "g", "e13")?;
    out.extend(finish(inverse_family(sk, &f), "i", "inverse")?);
    Ok(out)
}

/// One JSON object per line, in generation order.
pub fn to_jsonl(qs: &[GraphQuestion]) -> String {
    let mut s = String::new();
    for q in qs {
        s.push_str(&serde_json::to_string(q).expect("serializes"));
        s.push('\n');
    }
    s
}

pub fn parse_jsonl(text: &str) -> Result<Vec<GraphQuestion>, String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("{FILE} line {}: {e}", i + 1)))
        .collect()
}
