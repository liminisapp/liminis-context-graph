//! The document-mode diff (issue #785, ADR-0785): classifies each chunk of a new revision
//! against the document's previous committed revision. Pure — no I/O.
//!
//! The match key is the SHA-256 of a chunk's exact source text, heading line included: a chunk
//! is re-extracted iff its own text changed. A context-only change (title, an ancestor heading,
//! the document map) never re-extracts. Pairing runs in four steps:
//!
//! 1. **Exact hash**, as a multiset in occurrence order → `unchanged` (covers moves and
//!    duplicated boilerplate without collapsing or double-retiring).
//! 2. **Identical heading path** among the leftovers → `changed` (a body edit).
//! 3. **Positional**: a leftover new chunk pairs with the leftover old chunk directly after the
//!    old counterpart of its nearest matched predecessor → `changed` (a heading rename, or a
//!    heading and body edited together).
//! 4. Leftover new chunks are `added`; leftover old chunks are `removed`.
//!
//! A `changed` chunk inherits the old chunk's stable `name`, so `knowledge_get_correction_changes`
//! pairs the two versions.

use std::collections::HashMap;

use serde::Serialize;

use super::chunker::{chunk_document, ChunkedDocument};

/// How one chunk fared in a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Added,
    Changed,
    Unchanged,
    Removed,
}

impl Classification {
    pub fn as_str(self) -> &'static str {
        match self {
            Classification::Added => "added",
            Classification::Changed => "changed",
            Classification::Unchanged => "unchanged",
            Classification::Removed => "removed",
        }
    }
}

/// One live, document-mode episode of the document as the baseline loader sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaselineEpisode {
    pub uuid: String,
    pub name: String,
    pub write_path: Option<String>,
    pub revision_id: Option<String>,
    /// `None` while the episode's revision is pending (uncommitted or abandoned).
    pub revision_committed_at_us: Option<i64>,
    pub heading_path: Vec<String>,
    pub chunk_hash: Option<String>,
    pub byte_start: Option<i64>,
    pub byte_end: Option<i64>,
}

/// One logical chunk of the previous committed state: every live episode (split units included)
/// of one `name` from the latest commit that wrote or re-stamped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineChunk {
    pub name: String,
    pub heading_path: Vec<String>,
    pub chunk_hash: String,
    pub revision_id: String,
    /// Ordered by `byte_start`.
    pub episode_uuids: Vec<String>,
    /// `(byte_start, byte_end)` of each episode in `episode_uuids`, in the same order: where the
    /// episode sits in the revision named by its `revision_id`.
    pub episode_spans: Vec<Option<(i64, i64)>>,
}

/// The previous committed state of a document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    /// In the previous revision's manifest order; names the manifest does not list follow, by name.
    pub chunks: Vec<BaselineChunk>,
    /// Live committed document-mode episodes of a name whose latest committed revision is not
    /// theirs — an overlap a torn commit left behind. Superseded at the next commit.
    pub stale_uuids: Vec<String>,
    /// Live chunk-mode (or pre-marker) episodes sharing the `doc_id`: the first document-mode
    /// commit replaces them (FR-023).
    pub mode_transition_uuids: Vec<String>,
}

impl Baseline {
    /// Builds the baseline from the document's live episodes and the previous revision's
    /// manifest (`chunk_names`, in order).
    pub fn from_episodes(episodes: Vec<BaselineEpisode>, manifest: &[String]) -> Baseline {
        let mut out = Baseline::default();
        // name → commit instant → episodes. Grouped by the instant, not the revision id: the
        // commit step re-stamps an unchanged chunk's episodes with the new revision id but keeps
        // their original commit instant (they have been believed since then), so a re-stamp that
        // a torn WAL tail applied to only some of a chunk's split units still leaves those units
        // one chunk, never a "stale" half.
        let mut by_name: HashMap<String, HashMap<i64, Vec<BaselineEpisode>>> = HashMap::new();
        for ep in episodes {
            match ep.write_path.as_deref() {
                Some(crate::types::WRITE_PATH_DOCUMENT) => {
                    let (Some(_), Some(at)) =
                        (ep.revision_id.as_ref(), ep.revision_committed_at_us)
                    else {
                        continue; // Pending: never part of the believed state.
                    };
                    by_name
                        .entry(ep.name.clone())
                        .or_default()
                        .entry(at)
                        .or_default()
                        .push(ep);
                }
                wp if crate::types::write_path_is_chunk_eligible(wp) => {
                    out.mode_transition_uuids.push(ep.uuid)
                }
                _ => {}
            }
        }
        let pos: HashMap<&str, usize> = manifest
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        let mut names: Vec<String> = by_name.keys().cloned().collect();
        names.sort_by(|a, b| {
            let pa = pos.get(a.as_str()).copied().unwrap_or(usize::MAX);
            let pb = pos.get(b.as_str()).copied().unwrap_or(usize::MAX);
            pa.cmp(&pb).then_with(|| a.cmp(b))
        });
        for name in names {
            let mut commits: Vec<(i64, Vec<BaselineEpisode>)> = by_name
                .remove(&name)
                .unwrap_or_default()
                .into_iter()
                .collect();
            // Latest commit wins; anything older under the same name is a torn-commit overlap.
            commits.sort_by_key(|c| std::cmp::Reverse(c.0));
            let mut iter = commits.into_iter();
            let Some((_, mut eps)) = iter.next() else {
                continue;
            };
            for (_, stale) in iter {
                out.stale_uuids.extend(stale.into_iter().map(|e| e.uuid));
            }
            // The newest revision stamp among the chunk's episodes (lexical tie-break keeps the
            // choice deterministic if a torn re-stamp left two).
            let rev = eps
                .iter()
                .filter_map(|e| e.revision_id.clone())
                .max()
                .unwrap_or_default();
            eps.sort_by(|a, b| {
                a.byte_start
                    .cmp(&b.byte_start)
                    .then_with(|| a.uuid.cmp(&b.uuid))
            });
            out.chunks.push(BaselineChunk {
                name,
                heading_path: eps[0].heading_path.clone(),
                chunk_hash: eps[0].chunk_hash.clone().unwrap_or_default(),
                revision_id: rev,
                episode_spans: eps.iter().map(|e| e.byte_start.zip(e.byte_end)).collect(),
                episode_uuids: eps.into_iter().map(|e| e.uuid).collect(),
            });
        }
        out.stale_uuids.sort();
        out.mode_transition_uuids.sort();
        out
    }
}

/// The diff result for one new chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkMatch {
    pub classification: Classification,
    /// The logical chunk's stable `name`: inherited when matched, freshly minted when added.
    pub name: String,
    /// Index into [`Baseline::chunks`] of the counterpart, for `unchanged` / `changed`.
    pub previous: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkDiff {
    /// One entry per new chunk, in new-document order.
    pub matches: Vec<ChunkMatch>,
    /// Indices into [`Baseline::chunks`] with no counterpart.
    pub removed: Vec<usize>,
}

impl ChunkDiff {
    pub fn count(&self, c: Classification) -> usize {
        if c == Classification::Removed {
            return self.removed.len();
        }
        self.matches
            .iter()
            .filter(|m| m.classification == c)
            .count()
    }
}

/// Classifies `new`'s chunks against `baseline`. `mint` produces a fresh name for an added chunk.
pub fn diff(
    baseline: &Baseline,
    new: &ChunkedDocument,
    mint: &mut dyn FnMut() -> String,
) -> ChunkDiff {
    let n = new.chunks.len();
    let mut pair: Vec<Option<(usize, Classification)>> = vec![None; n];
    let mut taken = vec![false; baseline.chunks.len()];

    // 1. Exact hash, first-come in occurrence order.
    for (i, c) in new.chunks.iter().enumerate() {
        if let Some(j) = (0..baseline.chunks.len())
            .find(|&j| !taken[j] && baseline.chunks[j].chunk_hash == c.chunk_hash)
        {
            taken[j] = true;
            pair[i] = Some((j, Classification::Unchanged));
        }
    }
    // 2. Identical heading path among the leftovers.
    for (i, c) in new.chunks.iter().enumerate() {
        if pair[i].is_some() {
            continue;
        }
        if let Some(j) = (0..baseline.chunks.len())
            .find(|&j| !taken[j] && baseline.chunks[j].heading_path == c.heading_path)
        {
            taken[j] = true;
            pair[i] = Some((j, Classification::Changed));
        }
    }
    // 3. Positional: the old chunk right after the nearest matched predecessor's counterpart.
    for i in 0..n {
        if pair[i].is_some() {
            continue;
        }
        let candidate = match (0..i).rev().find_map(|k| pair[k].map(|(j, _)| j)) {
            Some(j) => j + 1,
            None => 0,
        };
        if candidate < baseline.chunks.len() && !taken[candidate] {
            taken[candidate] = true;
            pair[i] = Some((candidate, Classification::Changed));
        }
    }
    // 4. Leftovers.
    let matches = pair
        .into_iter()
        .map(|p| match p {
            Some((j, classification)) => ChunkMatch {
                classification,
                name: baseline.chunks[j].name.clone(),
                previous: Some(j),
            },
            None => ChunkMatch {
                classification: Classification::Added,
                name: mint(),
                previous: None,
            },
        })
        .collect();
    let removed = (0..baseline.chunks.len()).filter(|&j| !taken[j]).collect();
    ChunkDiff { matches, removed }
}

/// SC-009 measurement of one revision pair under the chosen strategy (no LLM, no DB).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PairMeasure {
    pub old_chunks: usize,
    pub new_chunks: usize,
    pub unchanged: usize,
    pub changed: usize,
    pub added: usize,
    pub removed: usize,
    /// Chunks this strategy re-extracts (`changed + added`).
    pub extracted: usize,
    /// New chunks whose heading path no old chunk had: what a heading-path-addressed scheme (the
    /// app's `doc::heading_path::index` chunk ids) orphans and re-extracts.
    pub heading_path_orphans: usize,
    /// The pair differs only in heading text: same chunk count, identical bodies.
    pub heading_change_only: bool,
}

fn body_of(text: &str, headed: bool) -> &str {
    if !headed {
        return text;
    }
    text.find('\n').map_or("", |p| &text[p + 1..])
}

/// Measures one `(old, new)` revision pair.
pub fn measure_pair(old: &str, new: &str, max_chars: usize) -> PairMeasure {
    let old_doc = chunk_document(old, max_chars);
    let new_doc = chunk_document(new, max_chars);
    let baseline = Baseline {
        chunks: old_doc
            .chunks
            .iter()
            .map(|c| BaselineChunk {
                name: format!("old-{}", c.index),
                heading_path: c.heading_path.clone(),
                chunk_hash: c.chunk_hash.clone(),
                revision_id: "old".into(),
                episode_uuids: vec![format!("ep-{}", c.index)],
                episode_spans: vec![Some((c.byte_start as i64, c.byte_end as i64))],
            })
            .collect(),
        ..Default::default()
    };
    let mut k = 0usize;
    let d = diff(&baseline, &new_doc, &mut || {
        k += 1;
        format!("new-{k}")
    });
    let old_paths: Vec<&Vec<String>> = old_doc.chunks.iter().map(|c| &c.heading_path).collect();
    let heading_change_only = old != new
        && old_doc.chunks.len() == new_doc.chunks.len()
        && old_doc.chunks.iter().zip(&new_doc.chunks).all(|(a, b)| {
            body_of(&a.text, !a.heading_path.is_empty())
                == body_of(&b.text, !b.heading_path.is_empty())
        })
        && old_doc
            .chunks
            .iter()
            .zip(&new_doc.chunks)
            .any(|(a, b)| a.heading_path != b.heading_path);
    PairMeasure {
        old_chunks: old_doc.chunks.len(),
        new_chunks: new_doc.chunks.len(),
        unchanged: d.count(Classification::Unchanged),
        changed: d.count(Classification::Changed),
        added: d.count(Classification::Added),
        removed: d.count(Classification::Removed),
        extracted: d.count(Classification::Changed) + d.count(Classification::Added),
        heading_path_orphans: new_doc
            .chunks
            .iter()
            .filter(|c| !old_paths.contains(&&c.heading_path))
            .count(),
        heading_change_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "# Note\n\nIntro.\n\n## Goals\n\nShip it.\n\n## Options\n\nX or Y.\n\n## Decision\n\nWe chose Z.\n\n## Risks\n\nNone.\n";

    fn baseline_of(doc: &str) -> Baseline {
        let d = chunk_document(doc, 6000);
        let eps = d
            .chunks
            .iter()
            .map(|c| BaselineEpisode {
                uuid: format!("ep-{}", c.index),
                name: format!("n-{}", c.index),
                write_path: Some("document".into()),
                revision_id: Some("r1".into()),
                revision_committed_at_us: Some(10),
                heading_path: c.heading_path.clone(),
                chunk_hash: Some(c.chunk_hash.clone()),
                byte_start: Some(c.byte_start as i64),
                byte_end: Some(c.byte_end as i64),
            })
            .collect();
        let manifest: Vec<String> = (0..d.chunks.len()).map(|i| format!("n-{i}")).collect();
        Baseline::from_episodes(eps, &manifest)
    }

    fn run(old: &str, new: &str) -> ChunkDiff {
        let mut k = 0;
        diff(&baseline_of(old), &chunk_document(new, 6000), &mut || {
            k += 1;
            format!("fresh-{k}")
        })
    }

    fn classes(d: &ChunkDiff) -> Vec<Classification> {
        d.matches.iter().map(|m| m.classification).collect()
    }

    use Classification::*;

    #[test]
    fn identical_document_is_all_unchanged() {
        let d = run(NOTE, NOTE);
        assert_eq!(classes(&d), vec![Unchanged; 5]);
        assert!(d.removed.is_empty());
        assert_eq!(d.matches[2].name, "n-2");
    }

    #[test]
    fn one_paragraph_edit_is_one_changed_chunk_keeping_its_name() {
        let d = run(NOTE, &NOTE.replace("X or Y.", "X, Y or W."));
        assert_eq!(
            classes(&d),
            vec![Unchanged, Unchanged, Changed, Unchanged, Unchanged]
        );
        assert_eq!(d.matches[2].name, "n-2");
    }

    #[test]
    fn deleted_section_is_removed_without_disturbing_the_rest() {
        let d = run(NOTE, &NOTE.replace("## Risks\n\nNone.\n", ""));
        assert_eq!(classes(&d), vec![Unchanged; 4]);
        assert_eq!(d.removed, vec![4]);
        // A middle section too: the next section must not be paired positionally into it.
        let d = run(NOTE, &NOTE.replace("## Options\n\nX or Y.\n\n", ""));
        assert_eq!(classes(&d), vec![Unchanged; 4]);
        assert_eq!(d.removed, vec![2]);
    }

    #[test]
    fn moved_section_is_unchanged() {
        let moved = "# Note\n\nIntro.\n\n## Risks\n\nNone.\n\n## Goals\n\nShip it.\n\n## Options\n\nX or Y.\n\n## Decision\n\nWe chose Z.\n";
        let d = run(NOTE, moved);
        assert_eq!(classes(&d), vec![Unchanged; 5]);
        assert_eq!(d.matches[1].name, "n-4");
    }

    #[test]
    fn heading_rename_reextracts_only_that_section() {
        let nested = "# Doc\n\nTop.\n\n## Accepted\n\nPlan A.\n\n### Detail\n\nMore on A.\n";
        let renamed = nested.replace("## Accepted", "## Rejected");
        let d = run(nested, &renamed);
        assert_eq!(classes(&d), vec![Unchanged, Changed, Unchanged]);
        assert_eq!(d.matches[1].name, "n-1");
        assert!(d.removed.is_empty());
        let m = measure_pair(nested, &renamed, 6000);
        assert!(m.heading_change_only);
        assert_eq!(m.extracted, 1);
        // The address-keyed scheme orphans the renamed section and its descendant.
        assert_eq!(m.heading_path_orphans, 2);
    }

    #[test]
    fn body_less_heading_rename_reextracts_nothing() {
        let doc = "# Decision\n## Options\n\nWe chose Z.\n";
        let d = run(doc, &doc.replace("# Decision", "# Choice"));
        // The only chunk is "## Options\n\nWe chose Z." — byte-identical.
        assert_eq!(classes(&d), vec![Unchanged]);
    }

    #[test]
    fn duplicate_boilerplate_never_collapses_or_double_retires() {
        let old = "# A\n\nSame.\n\n# B\n\nSame.\n\n# C\n\nOther.\n";
        // Drop one duplicate.
        let new = "# A\n\nSame.\n\n# C\n\nOther.\n";
        let d = run(old, new);
        assert_eq!(classes(&d), vec![Unchanged, Unchanged]);
        assert_eq!(d.removed, vec![1]);
        // Add a third copy.
        let new = "# A\n\nSame.\n\n# B\n\nSame.\n\n# A\n\nSame.\n\n# C\n\nOther.\n";
        let d = run(old, new);
        assert_eq!(d.count(Unchanged), 3);
        assert_eq!(d.count(Added) + d.count(Changed), 1);
        assert!(d.removed.is_empty());
    }

    #[test]
    fn added_section_gets_a_fresh_name() {
        let d = run(NOTE, &format!("{NOTE}\n## Next\n\nFollow up.\n"));
        assert_eq!(classes(&d)[5], Added);
        assert_eq!(d.matches[5].name, "fresh-1");
    }

    #[test]
    fn baseline_partitions_mode_transition_pending_and_stale() {
        let ep = |uuid: &str, name: &str, wp: Option<&str>, rev: Option<&str>, at: Option<i64>| {
            BaselineEpisode {
                uuid: uuid.into(),
                name: name.into(),
                write_path: wp.map(str::to_string),
                revision_id: rev.map(str::to_string),
                revision_committed_at_us: at,
                chunk_hash: Some("h".into()),
                ..Default::default()
            }
        };
        let b = Baseline::from_episodes(
            vec![
                ep("c1", "chunk-a", Some("chunk"), None, None),
                ep("c2", "chunk-b", None, None, None),
                ep("e1", "free", Some("episode"), None, None),
                ep("p1", "n1", Some("document"), Some("r3"), None),
                ep("old", "n1", Some("document"), Some("r1"), Some(10)),
                ep("new", "n1", Some("document"), Some("r2"), Some(20)),
            ],
            &["n1".to_string()],
        );
        assert_eq!(b.mode_transition_uuids, vec!["c1", "c2"]);
        assert_eq!(b.stale_uuids, vec!["old"]);
        assert_eq!(b.chunks.len(), 1);
        assert_eq!(b.chunks[0].episode_uuids, vec!["new"]);
        assert_eq!(b.chunks[0].revision_id, "r2");
    }
}
