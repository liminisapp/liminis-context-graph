//! Per-origin reading of retrieval results (#788 FR-022, ADR-0788).
//!
//! Real notebooks hold agent-maintained pages that restate facts from human notes. This module
//! measures how each system treats those restatements. Definitions (fixed so they are comparable
//! across configurations):
//!
//! * A **restating note** for question *q* is a `derived` note whose `cites` contains one of *q*'s
//!   relevant `primary` notes and which is not itself in *q*'s `relevant_notes`.
//! * A question is **eligible** when it is not negative and its restating set is non-empty.
//! * `restating_share` = |ranked ∩ restating| / |ranked| (0 when nothing was returned).
//! * `outrank` (only where defined) = the best-ranked restating note sits above the best-ranked
//!   relevant primary note. Defined only when both appear in the ranked list.
//! * `derived_only` = a restating note appears but no relevant primary note does.
//!
//! `ranked` is always the de-duplicated top-k list (ADR-0754 rules), so these are computed on the
//! same list as the headline metrics.

use std::collections::{BTreeMap, BTreeSet};

use super::fixture::{Manifest, Question};

/// Note → origin / citations, taken from the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OriginIndex {
    origin: BTreeMap<String, String>,
    cites: BTreeMap<String, Vec<String>>,
}

impl OriginIndex {
    pub fn from_manifest(m: &Manifest) -> Self {
        let mut idx = OriginIndex::default();
        for n in &m.notes {
            if let Some(o) = &n.origin {
                idx.origin.insert(n.id.clone(), o.clone());
                idx.cites.insert(n.id.clone(), n.cites.clone());
            }
        }
        idx
    }

    /// True when the fixture declares no origins at all (v1, a plain private notebook).
    pub fn is_empty(&self) -> bool {
        self.origin.is_empty()
    }

    pub fn origin_of(&self, note: &str) -> Option<&str> {
        self.origin.get(note).map(String::as_str)
    }

    /// The restating notes of `q` (see the module docs).
    pub fn restating(&self, q: &Question) -> BTreeSet<String> {
        let primaries: BTreeSet<&str> = q
            .relevant
            .iter()
            .filter(|r| self.origin_of(&r.note) == Some("primary"))
            .map(|r| r.note.as_str())
            .collect();
        if primaries.is_empty() {
            return BTreeSet::new();
        }
        let relevant: BTreeSet<&str> = q.relevant.iter().map(|r| r.note.as_str()).collect();
        self.cites
            .iter()
            .filter(|(id, cited)| {
                self.origin_of(id) == Some("derived")
                    && !relevant.contains(id.as_str())
                    && cited.iter().any(|c| primaries.contains(c.as_str()))
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

/// Per-question origin reading for an eligible question.
#[derive(Debug, Clone, PartialEq)]
pub struct OriginQuestion {
    pub restating_share: f64,
    /// `None` unless both a relevant primary and a restating note were returned.
    pub outranks_primary: Option<bool>,
    pub derived_only: bool,
}

/// `None` when the question is ineligible (negative, or no restating note exists).
pub fn origin_question(
    idx: &OriginIndex,
    q: &Question,
    ranked: &[String],
) -> Option<OriginQuestion> {
    if q.negative {
        return None;
    }
    let restating = idx.restating(q);
    if restating.is_empty() {
        return None;
    }
    let primaries: Vec<&str> = q
        .relevant
        .iter()
        .filter(|r| idx.origin_of(&r.note) == Some("primary"))
        .map(|r| r.note.as_str())
        .collect();
    let best_restating = ranked.iter().position(|n| restating.contains(n));
    let best_primary = ranked.iter().position(|n| primaries.contains(&n.as_str()));
    let hits = ranked.iter().filter(|n| restating.contains(*n)).count();
    let share = if ranked.is_empty() {
        0.0
    } else {
        hits as f64 / ranked.len() as f64
    };
    Some(OriginQuestion {
        restating_share: share,
        outranks_primary: match (best_restating, best_primary) {
            (Some(r), Some(p)) => Some(r < p),
            _ => None,
        },
        derived_only: best_restating.is_some() && best_primary.is_none(),
    })
}

/// Everything the report needs about origins for one configuration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OriginData {
    /// `(question id, reading)` for eligible questions, in question order.
    pub per_question: Vec<(String, OriginQuestion)>,
    /// Returned results by origin over all non-negative questions (`undeclared` for notes with
    /// none), and the total number of returned results.
    pub mix: BTreeMap<String, usize>,
    pub returned: usize,
}

/// Builds [`OriginData`] from per-question ranked lists. `ranked` pairs a question id with its
/// de-duplicated top-k list.
pub fn origin_data<'a>(
    idx: &OriginIndex,
    questions: &'a [Question],
    ranked: impl Iterator<Item = (&'a str, &'a [String])>,
) -> OriginData {
    let by_id: BTreeMap<&str, &Question> = questions.iter().map(|q| (q.id.as_str(), q)).collect();
    let mut d = OriginData::default();
    for (id, list) in ranked {
        let Some(q) = by_id.get(id) else { continue };
        if q.negative {
            continue;
        }
        for n in list {
            let o = idx.origin_of(n).unwrap_or("undeclared");
            *d.mix.entry(o.to_string()).or_default() += 1;
            d.returned += 1;
        }
        if let Some(oq) = origin_question(idx, q, list) {
            d.per_question.push((id.to_string(), oq));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::fixture::{Category, NoteEntry, Relevant};

    fn note(id: &str, origin: Option<&str>, cites: &[&str]) -> NoteEntry {
        NoteEntry {
            id: id.into(),
            reference_time: None,
            origin: origin.map(str::to_string),
            cites: cites.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn idx() -> OriginIndex {
        OriginIndex::from_manifest(&Manifest {
            corpus_version: "v".into(),
            max_chunk_tokens: 900,
            notes: vec![
                note("src", Some("primary"), &[]),
                note("other", Some("primary"), &[]),
                note("wiki", Some("derived"), &["src"]),
                note("wiki2", Some("derived"), &["src", "other"]),
                note("clip", Some("external"), &[]),
                note("ans", Some("answer"), &["src"]),
            ],
            supersedes: vec![],
        })
    }

    fn q(rel: &[&str], negative: bool) -> Question {
        Question {
            id: "q".into(),
            category: Category::Direct,
            text: "t".into(),
            relevant: rel
                .iter()
                .map(|n| Relevant {
                    note: n.to_string(),
                    gain: 1,
                })
                .collect(),
            evidence: vec![],
            current: vec![],
            stale: vec![],
            negative,
        }
    }

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn restating_set_is_derived_notes_citing_a_relevant_primary() {
        let i = idx();
        let r = i.restating(&q(&["src"], false));
        assert_eq!(r.into_iter().collect::<Vec<_>>(), vec!["wiki", "wiki2"]);
        // `answer` notes are not restating notes; `other` is cited only by wiki2.
        let r = i.restating(&q(&["other"], false));
        assert_eq!(r.into_iter().collect::<Vec<_>>(), vec!["wiki2"]);
        // A restating note that is itself relevant is excluded (it is a legitimate hit).
        let r = i.restating(&q(&["src", "wiki"], false));
        assert_eq!(r.into_iter().collect::<Vec<_>>(), vec!["wiki2"]);
        // Relevant note that is not primary: nothing to restate.
        assert!(i.restating(&q(&["clip"], false)).is_empty());
    }

    #[test]
    fn share_outrank_and_derived_only() {
        let i = idx();
        let qq = q(&["src"], false);
        let a = origin_question(&i, &qq, &v(&["wiki", "src", "clip"])).unwrap();
        assert!((a.restating_share - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(a.outranks_primary, Some(true));
        assert!(!a.derived_only);
        let b = origin_question(&i, &qq, &v(&["src", "wiki", "wiki2"])).unwrap();
        assert_eq!(b.outranks_primary, Some(false));
        assert!((b.restating_share - 2.0 / 3.0).abs() < 1e-12);
        let c = origin_question(&i, &qq, &v(&["wiki", "clip"])).unwrap();
        assert!(c.derived_only);
        assert_eq!(
            c.outranks_primary, None,
            "primary absent: outrank undefined"
        );
        let d = origin_question(&i, &qq, &v(&["clip"])).unwrap();
        assert_eq!(d.restating_share, 0.0);
        assert!(!d.derived_only && d.outranks_primary.is_none());
        let e = origin_question(&i, &qq, &[]).unwrap();
        assert_eq!(e.restating_share, 0.0);
    }

    #[test]
    fn negatives_and_questions_without_restatements_are_ineligible() {
        let i = idx();
        assert!(origin_question(&i, &q(&[], true), &v(&["wiki"])).is_none());
        assert!(origin_question(&i, &q(&["clip"], false), &v(&["wiki"])).is_none());
    }

    #[test]
    fn restating_note_that_is_itself_relevant_is_not_counted_as_a_restatement() {
        let i = idx();
        let qq = q(&["src", "wiki", "wiki2"], false);
        assert!(origin_question(&i, &qq, &v(&["wiki", "src"])).is_none());
    }

    #[test]
    fn origin_data_collects_mix_over_non_negative_questions() {
        let i = idx();
        let qs = vec![
            Question {
                id: "a".into(),
                ..q(&["src"], false)
            },
            Question {
                id: "n".into(),
                ..q(&[], true)
            },
        ];
        let ra = v(&["wiki", "src", "zzz"]);
        let rn = v(&["clip"]);
        let d = origin_data(
            &i,
            &qs,
            [("a", ra.as_slice()), ("n", rn.as_slice())].into_iter(),
        );
        assert_eq!(d.returned, 3);
        assert_eq!(d.mix["derived"], 1);
        assert_eq!(d.mix["primary"], 1);
        assert_eq!(d.mix["undeclared"], 1);
        assert!(!d.mix.contains_key("external"));
        assert_eq!(d.per_question.len(), 1);
    }

    #[test]
    fn index_without_origins_is_empty() {
        let m = Manifest {
            corpus_version: "v".into(),
            max_chunk_tokens: 900,
            notes: vec![note("a", None, &[])],
            supersedes: vec![],
        };
        assert!(OriginIndex::from_manifest(&m).is_empty());
    }
}
