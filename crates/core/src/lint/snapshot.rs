//! The in-memory view `knowledge_lint` analyses (issue #755, ADR-0755).
//!
//! Loading is the only phase that touches the database: chunked, keyset-paged scans (uuid order,
//! no content, no embeddings) fill a [`LintSnapshot`], and only the tables the selected categories
//! need are scanned. Everything after that is a pure function of the snapshot and the evaluated
//! instant, so a run is deterministic and cannot mutate anything.

use crate::belief::EdgeBeliefRow;
use crate::db::Conn;
use crate::error::Error;
use crate::origin::{doc_key, Origin};

use super::Category;

/// Rows per scan chunk. Keeps every query bounded and the keyset position cheap.
pub const DEFAULT_CHUNK: usize = 2000;

/// One `Episodic` row, without content or embedding.
#[derive(Debug, Clone)]
pub struct LintEpisode {
    pub uuid: String,
    pub group_id: String,
    pub doc_id: Option<String>,
    pub origin: Origin,
    pub superseded_at_us: Option<i64>,
    /// `episode` or `document` (ADR-0707).
    pub superseded_by_kind: Option<String>,
    pub supersession_reason: Option<String>,
    /// Raw JSON `attributes`; only loaded when the unlinked-mention category is selected.
    pub attributes: Option<String>,
}

impl LintEpisode {
    /// The same predicate `belief::evaluate` uses: a future-dated supersession still counts as live.
    pub fn is_live(&self, t_us: i64) -> bool {
        self.superseded_at_us.is_none_or(|s| s > t_us)
    }

    pub fn doc_key(&self) -> String {
        doc_key(self.doc_id.as_deref(), &self.uuid)
    }
}

/// One `RelatesToNode_` with its endpoints.
#[derive(Debug, Clone)]
pub struct LintEdge {
    pub belief: EdgeBeliefRow,
    pub group_id: String,
    pub source: String,
    pub target: String,
    pub source_group: String,
    pub target_group: String,
    pub name: String,
    pub relation_type: Option<String>,
}

impl LintEdge {
    pub fn uuid(&self) -> &str {
        &self.belief.uuid
    }

    /// A cross-group edge (ADR-0615 warrant) never takes part in a per-group finding.
    pub fn same_group(&self) -> bool {
        self.group_id == self.source_group && self.group_id == self.target_group
    }
}

/// One `Entity` row, without summary or embedding.
#[derive(Debug, Clone)]
pub struct LintEntity {
    pub uuid: String,
    pub group_id: String,
    pub name: String,
    pub kind: String,
}

/// Which tables a category selection needs loaded.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Needs {
    pub episodes: bool,
    pub attributes: bool,
    pub edges: bool,
    pub entities: bool,
    pub mentions: bool,
}

impl Needs {
    pub fn for_categories(cats: &[Category]) -> Needs {
        let mut n = Needs::default();
        for c in cats {
            match c {
                Category::Contradiction | Category::StaleDerivedPage | Category::WeakSupport => {
                    n.episodes = true;
                    n.edges = true;
                }
                Category::OrphanEntity => {
                    n.episodes = true;
                    n.edges = true;
                    n.entities = true;
                    n.mentions = true;
                }
                Category::NearDuplicateEntity => n.entities = true,
                Category::UnlinkedMention => {
                    n.episodes = true;
                    n.attributes = true;
                    n.entities = true;
                    n.mentions = true;
                }
            }
        }
        n
    }
}

#[derive(Debug, Default)]
pub struct LintSnapshot {
    pub episodes: Vec<LintEpisode>,
    pub edges: Vec<LintEdge>,
    pub entities: Vec<LintEntity>,
    /// `(episode uuid, entity uuid)` for every `Episodic-[:MENTIONS]->Entity` of a loaded episode.
    pub mentions: Vec<(String, String)>,
}

impl LintSnapshot {
    /// Loads what `needs` asks for, scoped to `groups` (`None` ⇒ every group). `chunk` is the scan
    /// chunk size (tests shrink it to exercise the chunk boundaries).
    pub fn load(
        conn: &Conn<'_>,
        groups: Option<&[String]>,
        needs: Needs,
        chunk: usize,
    ) -> Result<LintSnapshot, Error> {
        let chunk = chunk.max(1);
        let mut snap = LintSnapshot::default();
        if needs.episodes {
            let mut after: Option<String> = None;
            loop {
                let rows =
                    conn.lint_scan_episodes(groups, after.as_deref(), chunk, needs.attributes)?;
                let n = rows.len();
                after = rows.last().map(|r| r.uuid.clone());
                snap.episodes.extend(rows);
                if n < chunk {
                    break;
                }
            }
        }
        if needs.edges {
            let mut after: Option<String> = None;
            loop {
                let rows = conn.lint_scan_edges(groups, after.as_deref(), chunk)?;
                let n = rows.len();
                after = rows.last().map(|r| r.uuid().to_string());
                snap.edges.extend(rows);
                if n < chunk {
                    break;
                }
            }
        }
        if needs.entities {
            let mut after: Option<String> = None;
            loop {
                let rows = conn.lint_scan_entities(groups, after.as_deref(), chunk)?;
                let n = rows.len();
                after = rows.last().map(|r| r.uuid.clone());
                snap.entities.extend(rows);
                if n < chunk {
                    break;
                }
            }
        }
        if needs.mentions {
            let uuids: Vec<String> = snap.episodes.iter().map(|e| e.uuid.clone()).collect();
            for part in uuids.chunks(chunk) {
                snap.mentions.extend(conn.lint_scan_mentions(part)?);
            }
        }
        Ok(snap)
    }
}
