//! `knowledge_move_document` (issue #903, ADR-0903): re-key a document's live episodes from one
//! identity to another **without re-extraction**.
//!
//! Two halves:
//!
//! * a **pure planner** ([`parse_moves`], [`plan_moves`]) that takes the batch, a snapshot of the
//!   group's episodes and each destination's [`Footprint`] (from [`destination_footprints`], the
//!   one place a destination is looked at) and returns either a [`MovePlan`] (the final column
//!   values per episode uuid) or the full list of [`Refusal`]s. Every refusal is decided here,
//!   before the first write, so a refused batch writes nothing — graph or WAL.
//! * [`apply_plan`], which writes the plan through the recording `Conn` helpers: one uuid-keyed,
//!   fixed-valued `SET` per episode, plus the document-mode and supersession state keyed by
//!   `doc_id` outside `Episodic` (`DocRevision`, `DocSupersession`, `Episodic.superseded_by`).
//!
//! A document is *identified* by its live episodes (an identity with none is an unknown source),
//! but its superseded revisions are history that belongs to it: they are re-keyed too, so
//! `knowledge_delete_by_source` on the old path finds nothing and lint citations of earlier
//! revisions name the new identity. A *destination* must be empty: no episodes of any liveness and
//! no document state of any kind (ADR-0903 decision 5).
//!
//! Origin, content, embeddings, entities, relationships and belief history are never touched.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::db::{Conn, DestDocState, EpisodeRekey, MoveCandidate};
use crate::error::Error;
use crate::source_identity::{episode_matches_source, episode_source_file};

/// A document identity: a `doc_id`, a source path, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub doc_id: Option<String>,
    pub source_path: Option<String>,
}

impl Identity {
    fn kinds(&self) -> (bool, bool) {
        (self.doc_id.is_some(), self.source_path.is_some())
    }

    fn describe(&self) -> String {
        match (&self.doc_id, &self.source_path) {
            (Some(d), Some(p)) => format!("doc_id '{d}' / source_path '{p}'"),
            (Some(d), None) => format!("doc_id '{d}'"),
            (None, Some(p)) => format!("source_path '{p}'"),
            (None, None) => "<empty identity>".to_string(),
        }
    }
}

/// One requested move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveSpec {
    pub from: Identity,
    pub to: Identity,
    /// Optional chunk-id prefix rewrite `(from, to)`.
    pub chunk_id_prefix: Option<(String, String)>,
}

/// Stable refusal reason codes (also the `code` of each entry in the error `data`).
pub mod code {
    pub const UNKNOWN_SOURCE: &str = "unknown_source";
    pub const OCCUPIED_DESTINATION: &str = "occupied_destination";
    pub const CROSS_GROUP: &str = "cross_group";
    pub const CHUNK_ID_COLLISION: &str = "chunk_id_collision";
    pub const IDENTICAL: &str = "identical";
    pub const DUPLICATE_SOURCE: &str = "duplicate_source";
    pub const DUPLICATE_DESTINATION: &str = "duplicate_destination";
    pub const DESTINATION_IS_SOURCE: &str = "destination_is_source";
    pub const DESTINATION_HAS_HISTORY: &str = "destination_has_history";
}

/// A refused move: the index into the request's `moves`, a stable `code` and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub index: usize,
    pub code: &'static str,
    pub message: String,
}

impl Refusal {
    pub fn to_json(&self) -> Value {
        serde_json::json!({ "index": self.index, "code": self.code, "message": self.message })
    }
}

/// One move, planned: the final values per episode and the summary the reply reports.
#[derive(Debug, Clone)]
pub struct PlannedMove {
    pub index: usize,
    pub spec: MoveSpec,
    /// `(uuid, changed columns)` for every matched live episode, in uuid order.
    pub episodes: Vec<(String, EpisodeRekey)>,
    /// Number of episodes whose chunk id (`name`) was rewritten.
    pub chunk_ids_rewritten: usize,
    /// Distinct old `doc_id`s of the matched episodes.
    pub old_doc_ids: BTreeSet<String>,
    /// Distinct chunk ids (`name`s) of the matched episodes, before the move — what an in-flight
    /// `knowledge_process_chunk` of this document would be keyed by.
    pub old_names: BTreeSet<String>,
}

/// A fully validated batch.
#[derive(Debug, Clone, Default)]
pub struct MovePlan {
    pub moves: Vec<PlannedMove>,
}

impl MovePlan {
    pub fn episodes_rekeyed(&self) -> usize {
        self.moves.iter().map(|m| m.episodes.len()).sum()
    }

    /// Every `doc_id` whose document-level lock a move must hold: each move's old and new ids.
    pub fn lock_doc_ids(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for m in &self.moves {
            out.extend(m.old_doc_ids.iter().cloned());
            if let Some(d) = &m.spec.to.doc_id {
                out.insert(d.clone());
            }
        }
        out
    }

    /// Every chunk id of every moving episode (see [`PlannedMove::old_names`]).
    pub fn chunk_names(&self) -> BTreeSet<String> {
        self.moves
            .iter()
            .flat_map(|m| m.old_names.iter().cloned())
            .collect()
    }

    /// `old doc_id -> new doc_id` for every document whose `doc_id` changes.
    fn doc_id_renames(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for m in &self.moves {
            if let Some(new) = &m.spec.to.doc_id {
                for old in &m.old_doc_ids {
                    if old != new {
                        out.insert(old.clone(), new.clone());
                    }
                }
            }
        }
        out
    }
}

fn ipc(msg: impl Into<String>) -> Error {
    Error::Ipc(msg.into())
}

fn optional_id(v: &Value, what: &str) -> Result<Option<String>, Error> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if !s.is_empty() => Ok(Some(s.clone())),
        _ => Err(ipc(format!(
            "{what} must be a non-empty string when provided"
        ))),
    }
}

fn parse_identity(v: &Value, what: &str) -> Result<Identity, Error> {
    if !v.is_object() {
        return Err(ipc(format!(
            "{what} must be an object with doc_id and/or source_path"
        )));
    }
    let id = Identity {
        doc_id: optional_id(&v["doc_id"], &format!("{what}.doc_id"))?,
        source_path: optional_id(&v["source_path"], &format!("{what}.source_path"))?,
    };
    if id.doc_id.is_none() && id.source_path.is_none() {
        return Err(ipc(format!(
            "{what} must name a doc_id, a source_path, or both"
        )));
    }
    Ok(id)
}

/// Parses `params.moves`: a required, non-empty array of
/// `{from: {doc_id?, source_path?}, to: {doc_id?, source_path?}, chunk_id_prefix?: {from, to}}`.
/// `from` and `to` must name the same identity kinds.
pub fn parse_moves(params: &Value) -> Result<Vec<MoveSpec>, Error> {
    let Some(items) = params["moves"].as_array() else {
        return Err(ipc("moves is required and must be an array of moves"));
    };
    if items.is_empty() {
        return Err(ipc(
            "empty_batch: moves must contain at least one move; nothing was changed",
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let from = parse_identity(&item["from"], &format!("moves[{i}].from"))?;
        let to = parse_identity(&item["to"], &format!("moves[{i}].to"))?;
        if from.kinds() != to.kinds() {
            return Err(ipc(format!(
                "moves[{i}]: from and to must name the same identity kinds (from: {}; to: {})",
                from.describe(),
                to.describe()
            )));
        }
        let chunk_id_prefix = match &item["chunk_id_prefix"] {
            Value::Null => None,
            p @ Value::Object(_) => {
                let f = p["from"].as_str().filter(|s| !s.is_empty());
                let t = p["to"].as_str();
                match (f, t) {
                    (Some(f), Some(t)) => Some((f.to_string(), t.to_string())),
                    _ => {
                        return Err(ipc(format!(
                            "moves[{i}].chunk_id_prefix needs a non-empty string `from` and a \
                             string `to`"
                        )))
                    }
                }
            }
            _ => {
                return Err(ipc(format!(
                    "moves[{i}].chunk_id_prefix must be an object {{from, to}}"
                )))
            }
        };
        out.push(MoveSpec {
            from,
            to,
            chunk_id_prefix,
        });
    }
    Ok(out)
}

fn row_matches(row: &MoveCandidate, id: &Identity) -> bool {
    if let Some(d) = &id.doc_id {
        if row.doc_id.as_deref() != Some(d.as_str()) {
            return false;
        }
    }
    if let Some(p) = &id.source_path {
        if !episode_matches_source(
            row.source_file.as_deref(),
            &row.source_description,
            &row.name,
            row.write_path.as_deref(),
            p,
        ) {
            return false;
        }
    }
    true
}

/// Everything the group holds under one **destination** identity — the single answer to "is this
/// destination empty?" (ADR-0903 decision 5). A move is only ever into an empty destination.
///
/// * `episodes`: every `Episodic` row of the group, **of any liveness** (live, superseded, retired),
///   that matches the destination `doc_id`, the destination source path, or — when the move
///   rewrites chunk ids — a chunk id under the rewritten prefix. Same matching as
///   [`row_matches`] / `episode_matches_source`, but either named component is enough.
/// * `doc_state`: the destination `doc_id`'s `DocRevision` rows, its `DocSupersession` record on
///   either side, and the episodes whose `superseded_by` names it.
///
/// It is taken for the destination as it stands, so it still includes the moving document's own
/// episodes and document state when the destination shares an identity component with the source
/// (a path-only change keeps the `doc_id`); [`Footprint::residual`] subtracts those.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    /// The destination `doc_id`, when the move names one.
    pub doc_id: Option<String>,
    /// `(uuid, live)` of every matching episode.
    pub episodes: Vec<(String, bool)>,
    pub doc_state: DestDocState,
}

/// What a footprint holds once the moving document's own rows are set aside.
#[derive(Debug, Default, PartialEq, Eq)]
struct Residual {
    live: usize,
    history: usize,
    doc_state: DestDocState,
}

impl Residual {
    fn is_empty(&self) -> bool {
        self.live == 0 && self.history == 0 && self.doc_state == DestDocState::default()
    }

    /// What is in the way, for the refusal message.
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.history > 0 {
            parts.push(format!("{} retired or superseded episode(s)", self.history));
        }
        if self.doc_state.revisions > 0 {
            parts.push(format!(
                "{} document revision row(s)",
                self.doc_state.revisions
            ));
        }
        if self.doc_state.superseded_record {
            parts.push("a document supersession record".to_string());
        }
        if self.doc_state.superseding_records > 0 {
            parts.push(
                "a document supersession record naming it as the superseding document".into(),
            );
        }
        if self.doc_state.superseded_by_episodes > 0 {
            parts.push(format!(
                "{} episode(s) superseded by it",
                self.doc_state.superseded_by_episodes
            ));
        }
        parts.join(", ")
    }
}

impl Footprint {
    /// The footprint minus the moving document: its own episodes (by uuid) and, when the
    /// destination `doc_id` is one of its own, the document-level state that `doc_id` carries.
    fn residual(&self, own: &HashSet<&str>, own_doc_ids: &BTreeSet<String>) -> Residual {
        let mut r = Residual::default();
        for (uuid, live) in &self.episodes {
            if own.contains(uuid.as_str()) {
                continue;
            }
            if *live {
                r.live += 1;
            } else {
                r.history += 1;
            }
        }
        if !self
            .doc_id
            .as_ref()
            .is_some_and(|d| own_doc_ids.contains(d))
        {
            r.doc_state = self.doc_state.clone();
        }
        r
    }
}

/// The episodes of `snapshot` (every episode of the group, any liveness) under `spec`'s
/// destination: a pure function of the snapshot, used by [`destination_footprints`].
pub fn episode_footprint(snapshot: &[MoveCandidate], spec: &MoveSpec) -> Vec<(String, bool)> {
    let to_prefix = spec
        .chunk_id_prefix
        .as_ref()
        .map(|(_, to)| to.as_str())
        .filter(|t| !t.is_empty());
    // Every source path the move will *store* — the named one, and the one the planner derives for
    // a `doc_id`-only move of a document whose source file is its `doc_id` — is a destination
    // path: another document already using it would end up sharing it, and a later
    // `delete_by_source` of that path would delete both.
    let mut dest_paths: BTreeSet<String> = spec.to.source_path.iter().cloned().collect();
    for row in snapshot.iter().filter(|r| row_matches(r, &spec.from)) {
        let (old, new) = source_change(row, spec);
        if let Some(n) = new.filter(|n| Some(n.as_str()) != old) {
            dest_paths.insert(n);
        }
    }
    snapshot
        .iter()
        .filter(|row| {
            spec.to
                .doc_id
                .as_deref()
                .is_some_and(|d| row.doc_id.as_deref() == Some(d))
                || dest_paths.iter().any(|p| {
                    episode_matches_source(
                        row.source_file.as_deref(),
                        &row.source_description,
                        &row.name,
                        row.write_path.as_deref(),
                        p,
                    )
                })
                || to_prefix.is_some_and(|t| {
                    row.name.starts_with(t)
                        || row.chunk_id.as_deref().is_some_and(|c| c.starts_with(t))
                })
        })
        .map(|row| (row.uuid.clone(), row.live))
        .collect()
}

/// The footprint of every move's destination, one per spec, in order. **The one place** the
/// destination is looked at: every move — whether it names a `doc_id`, a path or both — goes
/// through here, and the planner only ever sees the result. Episodes come from `snapshot` (the
/// group's episodes of any liveness, already loaded); the document-level state is read once for all
/// the batch's destinations. A failure to read it is an `Err`, never a refusal.
pub fn destination_footprints(
    conn: &Conn<'_>,
    group_id: &str,
    snapshot: &[MoveCandidate],
    specs: &[MoveSpec],
) -> Result<Vec<Footprint>, Error> {
    let ids: Vec<String> = specs
        .iter()
        .filter_map(|s| s.to.doc_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let state = conn.destination_doc_state(group_id, &ids)?;
    Ok(specs
        .iter()
        .map(|spec| Footprint {
            doc_id: spec.to.doc_id.clone(),
            episodes: episode_footprint(snapshot, spec),
            doc_state: spec
                .to
                .doc_id
                .as_ref()
                .and_then(|d| state.get(d).cloned())
                .unwrap_or_default(),
        })
        .collect())
}

/// The stored source file of `row` before a move and after it: the new source path when the move
/// names one; otherwise, for a document identified only by `doc_id` that carries it as its source
/// file too (document mode with no `source_path`), the new `doc_id`; otherwise unchanged. Shared by
/// the planner and the destination footprint, so the footprint covers exactly the source the
/// planner is about to write.
fn source_change<'a>(row: &'a MoveCandidate, spec: &MoveSpec) -> (Option<&'a str>, Option<String>) {
    let old_source = episode_source_file(
        row.source_file.as_deref(),
        &row.source_description,
        &row.name,
        row.write_path.as_deref(),
    );
    let new_source = match (&spec.to.source_path, &spec.to.doc_id) {
        (Some(p), _) => Some(p.clone()),
        (None, Some(d)) if old_source.is_some() && old_source == row.doc_id.as_deref() => {
            Some(d.clone())
        }
        _ => old_source.map(str::to_string),
    };
    (old_source, new_source)
}

fn rewrite_prefix(value: &str, prefix: &Option<(String, String)>) -> Option<String> {
    let (from, to) = prefix.as_ref()?;
    let rest = value.strip_prefix(from.as_str())?;
    let new = format!("{to}{rest}");
    (new != value).then_some(new)
}

/// The recomposed `source_description` after the source file and/or chunk id changed, or `None`
/// when it stays as is. `sd = "{source_file}:{chunk_id}"` for chunk and document episodes; a raw
/// `add_episode` description is the whole source.
fn recompose_description(
    sd: &str,
    old_source: Option<&str>,
    new_source: Option<&str>,
    old_name: &str,
    new_name: &str,
) -> Option<String> {
    let new = match (old_source, new_source) {
        (Some(o), Some(n)) if sd == o => n.to_string(),
        _ => {
            let base = sd
                .strip_suffix(old_name)
                .and_then(|s| s.strip_suffix(':'))
                .filter(|_| !old_name.is_empty());
            let base = base?;
            let base = match (old_source, new_source) {
                (Some(o), Some(n)) if base == o => n,
                _ => base,
            };
            format!("{base}:{new_name}")
        }
    };
    (new != sd).then_some(new)
}

/// Plans a batch against `live` (every episode of the group, flagged live or not) and the
/// destination `footprints` (one per spec, from [`destination_footprints`]) — or returns every
/// refusal. Pure: it reads no database.
///
/// The whole batch is validated against the state *before* the batch, so chains and swaps are
/// refused rather than ordered. A destination must be empty: if anything of any liveness is under
/// it, the move is `occupied_destination` (something live) or `destination_has_history` (nothing
/// live, but episodes or document state of any kind).
pub fn plan_moves(
    specs: &[MoveSpec],
    live: &[MoveCandidate],
    footprints: &[Footprint],
) -> Result<MovePlan, Vec<Refusal>> {
    debug_assert_eq!(specs.len(), footprints.len());
    let mut refusals: Vec<Refusal> = Vec::new();
    let refuse =
        |refusals: &mut Vec<Refusal>, index: usize, code: &'static str, message: String| {
            refusals.push(Refusal {
                index,
                code,
                message,
            });
        };

    // ── Per-move checks and matching ───────────────────────────────────────────────────────────
    let mut matched: Vec<Vec<usize>> = Vec::with_capacity(specs.len());
    for (i, spec) in specs.iter().enumerate() {
        if spec.from == spec.to {
            refuse(
                &mut refusals,
                i,
                code::IDENTICAL,
                format!(
                    "the old and new identity are identical ({})",
                    spec.from.describe()
                ),
            );
            matched.push(Vec::new());
            continue;
        }
        let rows: Vec<usize> = live
            .iter()
            .enumerate()
            .filter(|(_, r)| row_matches(r, &spec.from))
            .map(|(ix, _)| ix)
            .collect();
        if !rows.iter().any(|&ix| live[ix].live) {
            refuse(
                &mut refusals,
                i,
                code::UNKNOWN_SOURCE,
                format!("no live episode matches {}", spec.from.describe()),
            );
        } else {
            let own: HashSet<&str> = rows.iter().map(|&ix| live[ix].uuid.as_str()).collect();
            let own_docs: BTreeSet<String> = rows
                .iter()
                .filter_map(|&ix| live[ix].doc_id.clone())
                .collect();
            let left = footprints[i].residual(&own, &own_docs);
            if left.live > 0 {
                refuse(
                    &mut refusals,
                    i,
                    code::OCCUPIED_DESTINATION,
                    format!(
                        "{} already holds live episodes; moving into it would merge documents, \
                         which is not a move",
                        spec.to.describe()
                    ),
                );
            } else if !left.is_empty() {
                refuse(
                    &mut refusals,
                    i,
                    code::DESTINATION_HAS_HISTORY,
                    format!(
                        "{} holds no live episodes but is not empty: it still has {}; moving into \
                         it would merge two histories",
                        spec.to.describe(),
                        left.describe()
                    ),
                );
            }
        }
        matched.push(rows);
    }

    // ── Conflicts inside the batch ─────────────────────────────────────────────────────────────
    let mut claimed: HashMap<usize, usize> = HashMap::new();
    for (i, rows) in matched.iter().enumerate() {
        for &ix in rows {
            if let Some(&other) = claimed.get(&ix) {
                if other != i
                    && !refusals
                        .iter()
                        .any(|r| r.index == i && r.code == code::DUPLICATE_SOURCE)
                {
                    refuse(
                        &mut refusals,
                        i,
                        code::DUPLICATE_SOURCE,
                        format!(
                            "{} overlaps the source of moves[{other}]",
                            specs[i].from.describe()
                        ),
                    );
                }
            } else {
                claimed.insert(ix, i);
            }
        }
    }
    for i in 0..specs.len() {
        for j in 0..i {
            let (a, b) = (&specs[i], &specs[j]);
            let same = |x: &Option<String>, y: &Option<String>| x.is_some() && x == y;
            if same(&a.to.doc_id, &b.to.doc_id) || same(&a.to.source_path, &b.to.source_path) {
                refuse(
                    &mut refusals,
                    i,
                    code::DUPLICATE_DESTINATION,
                    format!("{} is also the destination of moves[{j}]", a.to.describe()),
                );
            }
            for (x, y, xi, yi) in [(a, b, i, j), (b, a, j, i)] {
                if same(&x.to.doc_id, &y.from.doc_id)
                    || same(&x.to.source_path, &y.from.source_path)
                {
                    refuse(
                        &mut refusals,
                        xi,
                        code::DESTINATION_IS_SOURCE,
                        format!(
                            "the destination {} is the source of moves[{yi}]; chains and swaps \
                             are not supported",
                            x.to.describe()
                        ),
                    );
                }
            }
        }
    }

    // ── Plan the column values ─────────────────────────────────────────────────────────────────
    let mut moves: Vec<PlannedMove> = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let mut episodes = Vec::new();
        let mut renamed: HashSet<String> = HashSet::new();
        let mut old_doc_ids = BTreeSet::new();
        let mut old_names = BTreeSet::new();
        for &ix in &matched[i] {
            let row = &live[ix];
            old_names.insert(row.name.clone());
            if let Some(d) = &row.doc_id {
                old_doc_ids.insert(d.clone());
            }
            let (old_source, new_source) = source_change(row, spec);
            let source_changed = new_source.as_deref() != old_source;
            let new_name = rewrite_prefix(&row.name, &spec.chunk_id_prefix);
            let new_chunk = row
                .chunk_id
                .as_deref()
                .and_then(|c| rewrite_prefix(c, &spec.chunk_id_prefix));
            // Units of one split chunk share a name: count chunk ids, not episodes.
            if let Some(n) = &new_name {
                renamed.insert(n.clone());
            }
            let change = EpisodeRekey {
                doc_id: spec
                    .to
                    .doc_id
                    .clone()
                    .filter(|d| row.doc_id.as_deref() != Some(d.as_str())),
                source_file: (spec.to.source_path.is_some() || source_changed)
                    .then(|| new_source.clone())
                    .flatten()
                    .filter(|s| row.source_file.as_deref() != Some(s.as_str())),
                source_description: if source_changed || new_name.is_some() {
                    recompose_description(
                        &row.source_description,
                        old_source,
                        new_source.as_deref(),
                        &row.name,
                        new_name.as_deref().unwrap_or(&row.name),
                    )
                } else {
                    None
                },
                name: new_name,
                chunk_id: new_chunk,
            };
            episodes.push((row.uuid.clone(), change));
        }
        moves.push(PlannedMove {
            index: i,
            spec: spec.clone(),
            episodes,
            chunk_ids_rewritten: renamed.len(),
            old_doc_ids,
            old_names,
        });
    }

    // ── Chunk-id collisions, group-wide, against the post-batch names ─────────────────────────
    // The units of one split chunk are separate live episodes that legitimately share one chunk id,
    // so a name is claimed by *owners*, not by episodes: a moving episode is owned by its move, any
    // other episode by "nobody". A collision is a final name claimed by two different moves, or by
    // a move and an episode that is not moving.
    let changing: HashMap<&str, (usize, &EpisodeRekey)> = moves
        .iter()
        .flat_map(|m| {
            m.episodes
                .iter()
                .map(move |(u, c)| (u.as_str(), (m.index, c)))
        })
        .collect();
    let mut final_names: HashMap<&str, HashSet<Option<usize>>> = HashMap::new();
    for r in live.iter().filter(|r| r.live) {
        let (owner, name) = match changing.get(r.uuid.as_str()) {
            Some((idx, c)) => (Some(*idx), c.name.as_deref().unwrap_or(&r.name)),
            None => (None, r.name.as_str()),
        };
        final_names.entry(name).or_default().insert(owner);
    }
    for m in &moves {
        // An existing episode under the rewritten prefix is already a footprint refusal (the
        // destination is not empty); this pass is for names the footprint cannot see: two moves of
        // one batch rewriting into the same chunk id.
        if refusals.iter().any(|r| {
            r.index == m.index
                && (r.code == code::OCCUPIED_DESTINATION || r.code == code::DESTINATION_HAS_HISTORY)
        }) {
            continue;
        }
        let collide = m.episodes.iter().find_map(|(_, c)| {
            c.name
                .as_deref()
                .filter(|n| final_names.get(n).is_some_and(|owners| owners.len() > 1))
        });
        if let Some(n) = collide {
            refuse(
                &mut refusals,
                m.index,
                code::CHUNK_ID_COLLISION,
                format!("rewriting chunk ids would collide with a live chunk id '{n}'"),
            );
        }
    }

    if refusals.is_empty() {
        Ok(MovePlan { moves })
    } else {
        refusals.sort_by(|a, b| (a.index, a.code).cmp(&(b.index, b.code)));
        refusals.dedup();
        Err(refusals)
    }
}

/// Upgrades `unknown_source` refusals whose identity lives in *another* group to `cross_group`.
/// `other` holds the episodes of every group (the caller filters nothing).
pub fn classify_cross_group(
    refusals: &mut [Refusal],
    specs: &[MoveSpec],
    group_id: &str,
    other: &[MoveCandidate],
) {
    for r in refusals
        .iter_mut()
        .filter(|r| r.code == code::UNKNOWN_SOURCE)
    {
        let spec = &specs[r.index];
        if let Some(g) = other
            .iter()
            .find(|c| c.live && c.group_id != group_id && row_matches(c, &spec.from))
            .map(|c| c.group_id.clone())
        {
            r.code = code::CROSS_GROUP;
            r.message = format!(
                "{} lives in group '{g}', not '{group_id}'; moving across groups is not supported",
                spec.from.describe()
            );
        }
    }
}

/// Test-only failure injection for [`apply_plan`]: fail the write after `remaining` more writes of
/// the named group. A `Mutex`, keyed by group, so tests running in parallel threads cannot trip
/// each other's moves. Disabled (`None`) in production.
static APPLY_FAILPOINT: std::sync::Mutex<Option<(String, usize)>> = std::sync::Mutex::new(None);

/// Arms the failpoint: the move's `after_writes + 1`-th write in `group_id` fails. Hidden: it
/// exists so the integration tests can prove a failure part-way through leaves nothing behind.
#[doc(hidden)]
pub fn set_apply_failpoint(group_id: &str, after_writes: usize) {
    *APPLY_FAILPOINT.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((group_id.to_string(), after_writes));
}

/// Disarms [`set_apply_failpoint`].
#[doc(hidden)]
pub fn clear_apply_failpoint() {
    *APPLY_FAILPOINT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

fn failpoint_tick(group_id: &str) -> Result<(), Error> {
    let mut g = APPLY_FAILPOINT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((gid, remaining)) = g.as_mut() {
        if gid == group_id {
            if *remaining == 0 {
                *g = None;
                return Err(Error::QueryFailed(
                    "injected failure while applying a move".to_string(),
                ));
            }
            *remaining -= 1;
        }
    }
    Ok(())
}

/// Writes a validated plan **as one transaction**: either every statement commits or none does. On
/// any failure the transaction is rolled back and the recorded WAL statements are discarded, so
/// neither the graph nor the WAL has a trace of the move and re-sending the same batch simply
/// works. (A transaction cannot use the statement-level FTS backstop, ADR-0649; a graph whose
/// index is stale fails the whole move with an index error rather than half-applying it.)
///
/// All statements are uuid- or key-keyed and fixed-valued, so replay (also over an overlapping
/// tail) converges. The caller flushes `conn.drain_mutations()` to the WAL only on `Ok`.
pub fn apply_plan(conn: &Conn<'_>, group_id: &str, plan: &MovePlan) -> Result<(), Error> {
    conn.exec_transaction_control("BEGIN TRANSACTION")?;
    let result = apply_statements(conn, group_id, plan);
    match result {
        Ok(()) => match conn.exec_transaction_control("COMMIT") {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = conn.exec_transaction_control("ROLLBACK");
                let _ = conn.drain_mutations();
                Err(e)
            }
        },
        Err(e) => {
            // An execute-time failure may already have rolled the engine back; a ROLLBACK then
            // errors, which is harmless.
            let _ = conn.exec_transaction_control("ROLLBACK");
            let _ = conn.drain_mutations();
            Err(e)
        }
    }
}

fn apply_statements(conn: &Conn<'_>, group_id: &str, plan: &MovePlan) -> Result<(), Error> {
    for m in &plan.moves {
        for (uuid, change) in &m.episodes {
            failpoint_tick(group_id)?;
            conn.rekey_episode(uuid, change)?;
        }
    }
    let renames = plan.doc_id_renames();

    // `DocRevision` rows: the next `knowledge_process_document` at the new `doc_id` reads the
    // baseline from them, so they must follow the episodes or that ingest would re-extract.
    for m in &plan.moves {
        for old in &m.old_doc_ids {
            let new = renames.get(old).map(String::as_str).unwrap_or(old);
            for rev in conn.doc_revision_rows_for_doc(group_id, old)? {
                let (rev_uuid, source_path, chunk_names) =
                    (rev.uuid, rev.source_path, rev.chunk_names);
                let new_sp = match (&m.spec.to.source_path, &source_path) {
                    (Some(p), _) => Some(p.clone()),
                    (None, Some(sp)) if sp == old && new != old.as_str() => Some(new.to_string()),
                    (None, sp) => sp.clone(),
                };
                let new_names: Vec<String> = chunk_names
                    .iter()
                    .map(|n| {
                        rewrite_prefix(n, &m.spec.chunk_id_prefix).unwrap_or_else(|| n.clone())
                    })
                    .collect();
                if new == old.as_str() && new_sp == source_path && new_names == chunk_names {
                    continue;
                }
                failpoint_tick(group_id)?;
                conn.rekey_doc_revision(&rev_uuid, new, new_sp.as_deref(), &new_names)?;
            }
        }
    }

    // Supersession keyed by `doc_id`, on both sides, from the pre-batch state.
    if !renames.is_empty() {
        let mut records = BTreeMap::new();
        for old in renames.keys() {
            if let Some(rec) = conn.doc_supersession_for(group_id, old)? {
                records.insert(rec.superseded_doc_id.clone(), rec);
            }
            for superseded in conn.doc_supersessions_superseded_by(group_id, old)? {
                if let Some(rec) = conn.doc_supersession_for(group_id, &superseded)? {
                    records.insert(rec.superseded_doc_id.clone(), rec);
                }
            }
        }
        let rn = |d: &str| renames.get(d).cloned().unwrap_or_else(|| d.to_string());
        for rec in records.values() {
            let (new_s, new_t) = (rn(&rec.superseded_doc_id), rn(&rec.superseding_doc_id));
            if new_s != rec.superseded_doc_id {
                // The primary key derives from the superseded doc id and cannot be `SET`.
                conn.delete_doc_supersession(group_id, &rec.superseded_doc_id)?;
                let moved = crate::belief::DocSupersessionRecord {
                    superseded_doc_id: new_s,
                    superseding_doc_id: new_t,
                    ..rec.clone()
                };
                conn.create_doc_supersession(
                    &moved,
                    &crate::belief::format_us(moved.effective_at_us),
                    &crate::belief::format_us(moved.recorded_at_us),
                )?;
            } else if new_t != rec.superseding_doc_id {
                conn.set_doc_supersession_superseding(group_id, &rec.superseded_doc_id, &new_t)?;
            }
        }
        for (old, new) in &renames {
            let uuids = conn.episode_uuids_superseded_by_doc(group_id, old)?;
            conn.set_superseded_by_doc(&uuids, new)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(uuid: &str, doc: Option<&str>, sf: &str, name: &str) -> MoveCandidate {
        MoveCandidate {
            uuid: uuid.into(),
            live: true,
            group_id: "g".into(),
            doc_id: doc.map(str::to_string),
            source_file: Some(sf.into()),
            source_description: format!("{sf}:{name}"),
            name: name.into(),
            chunk_id: Some(name.into()),
            write_path: Some("chunk".into()),
        }
    }

    fn id(doc: Option<&str>, path: Option<&str>) -> Identity {
        Identity {
            doc_id: doc.map(str::to_string),
            source_path: path.map(str::to_string),
        }
    }

    fn mv(from: Identity, to: Identity) -> MoveSpec {
        MoveSpec {
            from,
            to,
            chunk_id_prefix: None,
        }
    }

    /// `plan_moves` with the episode part of each footprint derived from the snapshot and no
    /// document-level state (the database half of a footprint is covered in `footprint_table`).
    fn planned(specs: &[MoveSpec], live: &[MoveCandidate]) -> Result<MovePlan, Vec<Refusal>> {
        let fps: Vec<Footprint> = specs
            .iter()
            .map(|s| Footprint {
                doc_id: s.to.doc_id.clone(),
                episodes: episode_footprint(live, s),
                doc_state: DestDocState::default(),
            })
            .collect();
        plan_moves(specs, live, &fps)
    }

    fn planned_with(
        specs: &[MoveSpec],
        live: &[MoveCandidate],
        state: DestDocState,
    ) -> Result<MovePlan, Vec<Refusal>> {
        let fps: Vec<Footprint> = specs
            .iter()
            .map(|s| Footprint {
                doc_id: s.to.doc_id.clone(),
                episodes: episode_footprint(live, s),
                doc_state: state.clone(),
            })
            .collect();
        plan_moves(specs, live, &fps)
    }

    fn codes(r: &[Refusal]) -> Vec<&'static str> {
        r.iter().map(|r| r.code).collect()
    }

    #[test]
    fn parse_requires_non_empty_moves_and_matching_kinds() {
        assert!(parse_moves(&json!({})).is_err());
        assert!(parse_moves(&json!({"moves": []})).is_err());
        let bad = json!({"moves": [{"from": {"doc_id": "a"}, "to": {"source_path": "b"}}]});
        assert!(parse_moves(&bad).is_err());
        let ok = json!({"moves": [{"from": {"doc_id": "a", "source_path": "p"},
            "to": {"doc_id": "b", "source_path": "q"},
            "chunk_id_prefix": {"from": "p::", "to": "q::"}}]});
        let m = parse_moves(&ok).unwrap();
        assert_eq!(m[0].chunk_id_prefix, Some(("p::".into(), "q::".into())));
        let empty_prefix = json!({"moves": [{"from": {"doc_id": "a"}, "to": {"doc_id": "b"},
            "chunk_id_prefix": {"from": "", "to": "x"}}]});
        assert!(parse_moves(&empty_prefix).is_err());
    }

    #[test]
    fn plans_path_move_with_prefix_rewrite() {
        let live = vec![
            row("u1", Some("a.md"), "a.md", "a.md::intro::0"),
            row("u2", Some("a.md"), "a.md", "a.md::body::1"),
            row("u3", Some("z.md"), "z.md", "z.md::x::0"),
        ];
        let mut spec = mv(
            id(Some("a.md"), Some("a.md")),
            id(Some("b.md"), Some("b.md")),
        );
        spec.chunk_id_prefix = Some(("a.md::".into(), "b.md::".into()));
        let plan = planned(&[spec], &live).unwrap();
        assert_eq!(plan.episodes_rekeyed(), 2);
        let (_, c) = &plan.moves[0].episodes[0];
        assert_eq!(c.doc_id.as_deref(), Some("b.md"));
        assert_eq!(c.source_file.as_deref(), Some("b.md"));
        assert_eq!(c.name.as_deref(), Some("b.md::intro::0"));
        assert_eq!(c.chunk_id.as_deref(), Some("b.md::intro::0"));
        assert_eq!(c.source_description.as_deref(), Some("b.md:b.md::intro::0"));
        assert_eq!(plan.moves[0].chunk_ids_rewritten, 2);
    }

    #[test]
    fn without_prefix_names_stay_and_description_follows_the_path() {
        let live = vec![row("u1", None, "a.md", "c1")];
        let plan = planned(&[mv(id(None, Some("a.md")), id(None, Some("b.md")))], &live).unwrap();
        let (_, c) = &plan.moves[0].episodes[0];
        assert_eq!(c.name, None);
        assert_eq!(c.doc_id, None);
        assert_eq!(c.source_file.as_deref(), Some("b.md"));
        assert_eq!(c.source_description.as_deref(), Some("b.md:c1"));
    }

    #[test]
    fn doc_only_move_keeps_doc_id_as_source_file_consistent() {
        // Document mode without a source_path stores the doc_id as the source file.
        let live = vec![row("u1", Some("d1"), "d1", "d1::abc")];
        let plan = planned(&[mv(id(Some("d1"), None), id(Some("d2"), None))], &live).unwrap();
        let (_, c) = &plan.moves[0].episodes[0];
        assert_eq!(c.doc_id.as_deref(), Some("d2"));
        assert_eq!(c.source_file.as_deref(), Some("d2"));
        assert_eq!(c.source_description.as_deref(), Some("d2:d1::abc"));
    }

    #[test]
    fn legacy_row_without_source_file_is_migrated_to_a_stored_one() {
        let mut r = row("u1", None, "a.md", "c1");
        r.source_file = None;
        let plan = planned(&[mv(id(None, Some("a.md")), id(None, Some("b.md")))], &[r]).unwrap();
        let (_, c) = &plan.moves[0].episodes[0];
        assert_eq!(c.source_file.as_deref(), Some("b.md"));
        assert_eq!(c.source_description.as_deref(), Some("b.md:c1"));
    }

    #[test]
    fn raw_episode_description_is_the_whole_new_source() {
        let mut r = row("u1", None, "a.md", "free form name");
        r.source_file = None;
        r.source_description = "a.md".into();
        r.write_path = Some("episode".into());
        let plan = planned(&[mv(id(None, Some("a.md")), id(None, Some("b.md")))], &[r]).unwrap();
        let (_, c) = &plan.moves[0].episodes[0];
        assert_eq!(c.source_description.as_deref(), Some("b.md"));
    }

    #[test]
    fn refuses_unknown_occupied_identical() {
        let live = vec![
            row("u1", Some("a"), "a.md", "a::1"),
            row("u2", Some("b"), "b.md", "b::1"),
        ];
        let e = planned(&[mv(id(Some("nope"), None), id(Some("x"), None))], &live).unwrap_err();
        assert_eq!(codes(&e), [code::UNKNOWN_SOURCE]);
        let e = planned(&[mv(id(Some("a"), None), id(Some("b"), None))], &live).unwrap_err();
        assert_eq!(codes(&e), [code::OCCUPIED_DESTINATION]);
        let e = planned(&[mv(id(None, Some("a.md")), id(None, Some("b.md")))], &live).unwrap_err();
        assert_eq!(codes(&e), [code::OCCUPIED_DESTINATION]);
        let e = planned(&[mv(id(Some("a"), None), id(Some("a"), None))], &live).unwrap_err();
        assert_eq!(codes(&e), [code::IDENTICAL]);
    }

    #[test]
    fn path_change_with_unchanged_doc_id_is_not_occupied_by_itself() {
        let live = vec![row("u1", Some("a"), "a.md", "a::1")];
        let plan = planned(
            &[mv(id(Some("a"), Some("a.md")), id(Some("a"), Some("c.md")))],
            &live,
        )
        .unwrap();
        let (_, c) = &plan.moves[0].episodes[0];
        assert_eq!(c.doc_id, None);
        assert_eq!(c.source_file.as_deref(), Some("c.md"));
    }

    #[test]
    fn refuses_batch_conflicts() {
        let live = vec![
            row("u1", Some("a"), "a.md", "a::1"),
            row("u2", Some("b"), "b.md", "b::1"),
            row("u3", Some("c"), "c.md", "c::1"),
        ];
        // Duplicate source.
        let e = planned(
            &[
                mv(id(Some("a"), None), id(Some("x"), None)),
                mv(id(Some("a"), None), id(Some("y"), None)),
            ],
            &live,
        )
        .unwrap_err();
        assert!(codes(&e).contains(&code::DUPLICATE_SOURCE));
        // Duplicate destination.
        let e = planned(
            &[
                mv(id(Some("a"), None), id(Some("x"), None)),
                mv(id(Some("b"), None), id(Some("x"), None)),
            ],
            &live,
        )
        .unwrap_err();
        assert!(codes(&e).contains(&code::DUPLICATE_DESTINATION));
        // Chain: a -> b while b -> x.
        let e = planned(
            &[
                mv(id(Some("a"), None), id(Some("b"), None)),
                mv(id(Some("b"), None), id(Some("x"), None)),
            ],
            &live,
        )
        .unwrap_err();
        assert!(codes(&e).contains(&code::DESTINATION_IS_SOURCE));
        // Swap.
        let e = planned(
            &[
                mv(id(Some("a"), None), id(Some("b"), None)),
                mv(id(Some("b"), None), id(Some("a"), None)),
            ],
            &live,
        )
        .unwrap_err();
        assert!(codes(&e).contains(&code::DESTINATION_IS_SOURCE));
    }

    #[test]
    fn chunk_id_collisions_are_the_footprint_or_a_clash_within_the_batch() {
        let live = vec![
            row("u1", Some("a"), "a.md", "a.md::0"),
            row("u2", Some("z"), "z.md", "b.md::0"), // already holds the rewritten name
        ];
        let mut spec = mv(id(Some("a"), Some("a.md")), id(Some("b"), Some("b.md")));
        spec.chunk_id_prefix = Some(("a.md::".into(), "b.md::".into()));
        // An existing live chunk id under the rewritten prefix: the destination is not empty.
        let e = planned(&[spec], &live).unwrap_err();
        assert_eq!(codes(&e), [code::OCCUPIED_DESTINATION]);

        // Two moves rewriting into the same name.
        let live = vec![
            row("u1", Some("a"), "a.md", "p::0"),
            row("u2", Some("b"), "b.md", "q::0"),
        ];
        let mut m1 = mv(id(Some("a"), None), id(Some("x"), None));
        m1.chunk_id_prefix = Some(("p::".into(), "n::".into()));
        let mut m2 = mv(id(Some("b"), None), id(Some("y"), None));
        m2.chunk_id_prefix = Some(("q::".into(), "n::".into()));
        let e = planned(&[m1, m2], &live).unwrap_err();
        assert!(codes(&e).contains(&code::CHUNK_ID_COLLISION));
    }

    #[test]
    fn cross_group_is_told_apart_from_unknown() {
        let specs = [mv(id(Some("a"), None), id(Some("x"), None))];
        let mut e = planned(&specs, &[]).unwrap_err();
        let mut other = row("u9", Some("a"), "a.md", "a::1");
        other.group_id = "other".into();
        classify_cross_group(&mut e, &specs, "g", &[other]);
        assert_eq!(codes(&e), [code::CROSS_GROUP]);
        let mut e = planned(&specs, &[]).unwrap_err();
        classify_cross_group(&mut e, &specs, "g", &[]);
        assert_eq!(codes(&e), [code::UNKNOWN_SOURCE]);
    }

    #[test]
    fn history_moves_with_the_document_but_never_identifies() {
        let mut old_rev = row("u0", Some("a"), "a.md", "a::1");
        old_rev.live = false;
        let live = vec![old_rev.clone(), row("u1", Some("a"), "a.md", "a::1")];
        let plan = planned(&[mv(id(Some("a"), None), id(Some("b"), None))], &live).unwrap();
        // The superseded revision is re-keyed with its live sibling; the shared name is no collision.
        assert_eq!(plan.episodes_rekeyed(), 2);
        // A document that only has history is an unknown source.
        let e = planned(
            &[mv(id(Some("a"), None), id(Some("b"), None))],
            &[old_rev.clone()],
        )
        .unwrap_err();
        assert_eq!(codes(&e), [code::UNKNOWN_SOURCE]);
    }

    /// The destination must be empty (ADR-0903 decision 5): one table, every kind of thing that can
    /// be under a destination, run against a `doc_id` destination, a path-only destination and a
    /// combined one — the same rule and the same code every time.
    #[test]
    fn footprint_table() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        enum Dest {
            DocId,
            Path,
            Both,
        }
        let spec = |d: Dest, prefix: bool| {
            let (from, to) = match d {
                Dest::DocId => (id(Some("a"), None), id(Some("b"), None)),
                Dest::Path => (id(None, Some("a.md")), id(None, Some("b.md"))),
                Dest::Both => (id(Some("a"), Some("a.md")), id(Some("b"), Some("b.md"))),
            };
            MoveSpec {
                from,
                to,
                chunk_id_prefix: prefix.then(|| ("a::".to_string(), "b::".to_string())),
            }
        };
        let source = || row("src", Some("a"), "a.md", "a::1");
        // A row *at* the destination: matches by `doc_id` b and by path b.md.
        let at_dest = |uuid: &str, live: bool| {
            let mut r = row(uuid, Some("b"), "b.md", "b::existing");
            r.live = live;
            r
        };
        let state = |f: fn(&mut DestDocState)| {
            let mut st = DestDocState::default();
            f(&mut st);
            st
        };

        // (label, extra episodes, document state, destination kinds it can be seen under,
        //  expected refusal — `None` = the move is allowed)
        let all = [Dest::DocId, Dest::Path, Dest::Both];
        let with_doc = [Dest::DocId, Dest::Both];
        type Case<'a> = (
            &'a str,
            Vec<MoveCandidate>,
            DestDocState,
            &'a [Dest],
            Option<&'a str>,
        );
        let cases: Vec<Case> = vec![
            (
                "empty destination",
                vec![],
                DestDocState::default(),
                &all,
                None,
            ),
            (
                "a live episode",
                vec![at_dest("d1", true)],
                DestDocState::default(),
                &all,
                Some(code::OCCUPIED_DESTINATION),
            ),
            (
                "a superseded or retired episode (no live one)",
                vec![at_dest("d1", false)],
                DestDocState::default(),
                &all,
                Some(code::DESTINATION_HAS_HISTORY),
            ),
            (
                "live and retired episodes together",
                vec![at_dest("d1", true), at_dest("d2", false)],
                DestDocState::default(),
                &all,
                Some(code::OCCUPIED_DESTINATION),
            ),
            (
                "a DocRevision row",
                vec![],
                state(|s| s.revisions = 1),
                &with_doc,
                Some(code::DESTINATION_HAS_HISTORY),
            ),
            (
                "a supersession record as the superseded document",
                vec![],
                state(|s| s.superseded_record = true),
                &with_doc,
                Some(code::DESTINATION_HAS_HISTORY),
            ),
            (
                "a supersession record as the superseding document",
                vec![],
                state(|s| s.superseding_records = 1),
                &with_doc,
                Some(code::DESTINATION_HAS_HISTORY),
            ),
            (
                "an episode superseded_by the destination doc_id",
                vec![],
                state(|s| s.superseded_by_episodes = 1),
                &with_doc,
                Some(code::DESTINATION_HAS_HISTORY),
            ),
            (
                "an episode of another doc at the destination path only",
                vec![{
                    let mut r = row("d1", Some("other"), "b.md", "other::1");
                    r.live = false;
                    r
                }],
                DestDocState::default(),
                &[Dest::Path, Dest::Both],
                Some(code::DESTINATION_HAS_HISTORY),
            ),
            (
                "an episode of the destination doc_id at another path only",
                vec![{
                    let mut r = row("d1", Some("b"), "elsewhere.md", "b::1");
                    r.live = true;
                    r
                }],
                DestDocState::default(),
                &[Dest::DocId, Dest::Both],
                Some(code::OCCUPIED_DESTINATION),
            ),
            (
                "an unrelated document",
                vec![row("d1", Some("z"), "z.md", "z::1")],
                DestDocState::default(),
                &all,
                None,
            ),
        ];
        for (label, extra, st, kinds, expect) in &cases {
            for &kind in *kinds {
                let mut live = vec![source()];
                live.extend(extra.iter().cloned());
                let res = planned_with(&[spec(kind, false)], &live, st.clone());
                match (res, expect) {
                    (Ok(_), None) => {}
                    (Err(e), Some(code)) => {
                        assert_eq!(codes(&e), [*code], "{label} under {kind:?}")
                    }
                    (got, want) => panic!("{label} under {kind:?}: got {got:?}, want {want:?}"),
                }
            }
        }

        // Chunk ids under the rewritten prefix are part of the footprint, whatever else matches.
        for kind in all {
            for live_flag in [true, false] {
                let mut other = row("d1", Some("z"), "z.md", "b::x");
                other.live = live_flag;
                let e = planned(&[spec(kind, true)], &[source(), other]).unwrap_err();
                let want = if live_flag {
                    code::OCCUPIED_DESTINATION
                } else {
                    code::DESTINATION_HAS_HISTORY
                };
                assert_eq!(codes(&e), [want], "prefix footprint under {kind:?}");
            }
        }

        // The moving document's own rows and own document state are never in the way.
        let own_state = state(|s| {
            s.revisions = 2;
            s.superseded_record = true;
            s.superseding_records = 1;
            s.superseded_by_episodes = 3;
        });
        let live = vec![source()];
        let to_same_doc = MoveSpec {
            from: id(Some("a"), Some("a.md")),
            to: id(Some("a"), Some("c.md")),
            chunk_id_prefix: None,
        };
        assert!(planned_with(&[to_same_doc], &live, own_state).is_ok());
    }

    /// A failure to read the document state is an error, not a refusal (`destination_footprints`
    /// returns `Err`); the planner never sees it.
    #[test]
    fn residual_describes_what_is_in_the_way() {
        let fp = Footprint {
            doc_id: Some("b".into()),
            episodes: vec![("e1".into(), false)],
            doc_state: DestDocState {
                revisions: 1,
                superseded_record: true,
                superseding_records: 0,
                superseded_by_episodes: 2,
            },
        };
        let r = fp.residual(&HashSet::new(), &BTreeSet::new());
        let d = r.describe();
        assert!(d.contains("retired or superseded episode"), "{d}");
        assert!(d.contains("revision row"), "{d}");
        assert!(d.contains("supersession record"), "{d}");
        assert!(d.contains("superseded by it"), "{d}");
    }

    #[test]
    fn a_doc_less_episode_moves_by_source_path_alone() {
        let live = vec![row("u1", None, "a.md", "c1")];
        let plan = planned(&[mv(id(None, Some("a.md")), id(None, Some("b.md")))], &live).unwrap();
        assert!(plan.lock_doc_ids().is_empty());
    }

    #[test]
    fn units_of_one_split_chunk_share_a_name_without_colliding() {
        // Two live units of one chunk: same name, same document.
        let live = vec![
            row("u1", Some("a"), "a.md", "a::1"),
            row("u2", Some("a"), "a.md", "a::1"),
            row("u3", Some("a"), "a.md", "a::2"),
        ];
        let mut spec = mv(id(Some("a"), Some("a.md")), id(Some("b"), Some("b.md")));
        spec.chunk_id_prefix = Some(("a::".into(), "b::".into()));
        let plan = planned(&[spec], &live).unwrap();
        assert_eq!(plan.episodes_rekeyed(), 3);
        // Chunk ids, not episodes.
        assert_eq!(plan.moves[0].chunk_ids_rewritten, 2);
    }

    #[test]
    fn two_moves_rewriting_into_one_name_still_collide() {
        let live = vec![
            row("u1", Some("a"), "a.md", "p::0"),
            row("u2", Some("a"), "a.md", "p::0"),
            row("u3", Some("b"), "b.md", "q::0"),
        ];
        let mut m1 = mv(id(Some("a"), None), id(Some("x"), None));
        m1.chunk_id_prefix = Some(("p::".into(), "n::".into()));
        let mut m2 = mv(id(Some("b"), None), id(Some("y"), None));
        m2.chunk_id_prefix = Some(("q::".into(), "n::".into()));
        let e = planned(&[m1, m2], &live).unwrap_err();
        assert!(codes(&e).contains(&code::CHUNK_ID_COLLISION), "{e:?}");
    }

    #[test]
    fn a_doc_id_only_move_is_blocked_by_another_documents_source_path() {
        // `A` has no source path, so its source file is its doc_id; moving it to doc_id `B` stores
        // source file "B", which document `x` already uses.
        let live = vec![
            row("u1", Some("A"), "A", "A::1"),
            row("u2", Some("x"), "B", "x::1"),
        ];
        let e = planned(&[mv(id(Some("A"), None), id(Some("B"), None))], &live).unwrap_err();
        assert_eq!(codes(&e), [code::OCCUPIED_DESTINATION]);
        // With nothing at that path, the same move is fine and the stored source follows the doc_id.
        let live = vec![
            row("u1", Some("A"), "A", "A::1"),
            row("u2", Some("x"), "z", "x::1"),
        ];
        let plan = planned(&[mv(id(Some("A"), None), id(Some("B"), None))], &live).unwrap();
        assert_eq!(
            plan.moves[0].episodes[0].1.source_file.as_deref(),
            Some("B")
        );
    }
}
