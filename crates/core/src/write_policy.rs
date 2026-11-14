//! Daemon-wide per-group write allow-list (`--writable-groups` / `LCG_WRITABLE_GROUPS`,
//! issue #742, ADR-0742).
//!
//! `--scope` decides which *tools* an MCP connection can see; it says nothing about *which
//! groups* a write-capable connection may change. This module adds a server-side allow-list,
//! enforced at the single point every request path reaches — `handlers::handle()` — so IPC,
//! standalone MCP and attached MCP are all subject to it.
//!
//! The guard runs **before any handler work**, so a refused call performs no embedding,
//! extraction, database write or WAL event. Startup recovery, auto-heal and WAL replay never go
//! through `handle()` and are therefore not blocked.
//!
//! Group resolution deliberately reuses the handlers' own extraction helpers so the guard cannot
//! drift from what a handler will actually write.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::{
    app_state::{load_db, AppState},
    db::Conn,
    error::Error,
    handlers::{extract_required_group_id, extract_required_group_ids, group_id_param},
    ipc::IpcRequest,
    pointer::{self, EndpointSide},
    wal_group::{self, DEFAULT_GROUP_ID},
};

/// Command-line flag name, used in error messages.
pub const FLAG: &str = "--writable-groups";
/// Environment variable equivalent of [`FLAG`].
pub const ENV_VAR: &str = "LCG_WRITABLE_GROUPS";

/// An exact-match, case-sensitive set of group identifiers clients may change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritableGroups {
    groups: BTreeSet<String>,
}

impl WritableGroups {
    /// Strict parse of a comma-separated list. Entries are trimmed; an empty value, an empty
    /// entry (`a,,b`, leading/trailing comma) and an entry containing whitespace or control
    /// characters are errors. Duplicates are collapsed.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.trim().is_empty() {
            return Err(
                "empty value: expected a comma-separated list of group ids (e.g. a,b)".to_string(),
            );
        }
        let mut groups = BTreeSet::new();
        for (i, entry) in raw.split(',').enumerate() {
            let entry = entry.trim();
            if entry.is_empty() {
                return Err(format!(
                    "empty group id at position {} (stray or doubled comma?)",
                    i + 1
                ));
            }
            if entry.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!(
                    "group id {entry:?} contains whitespace or control characters"
                ));
            }
            groups.insert(entry.to_string());
        }
        Ok(Self { groups })
    }

    /// Resolves the effective policy from the flag and the env var. The flag wins; when it is
    /// present the env var is not parsed. A set-but-empty value in either source is an error, not
    /// "unset". `None` means no restriction.
    pub fn resolve(flag: Option<&str>, env: Option<&str>) -> Result<Option<Self>, String> {
        if let Some(v) = flag {
            return Self::parse(v)
                .map(Some)
                .map_err(|e| format!("invalid {FLAG}={v:?}: {e}"));
        }
        if let Some(v) = env {
            return Self::parse(v)
                .map(Some)
                .map_err(|e| format!("invalid {ENV_VAR}={v:?}: {e}"));
        }
        Ok(None)
    }

    pub fn contains(&self, group_id: &str) -> bool {
        self.groups.contains(group_id)
    }

    /// The listed groups in sorted order.
    pub fn list(&self) -> Vec<String> {
        self.groups.iter().cloned().collect()
    }

    /// One-line startup-log description.
    pub fn describe(&self) -> String {
        format!(
            "writable groups: [{}] (all other groups are read-only)",
            self.list().join(", ")
        )
    }
}

static POLICY: OnceLock<WritableGroups> = OnceLock::new();

/// Installs the process-wide policy. Called once from `main` before the socket binds. A second
/// call is ignored (returns `false`).
pub fn install(policy: WritableGroups) -> bool {
    POLICY.set(policy).is_ok()
}

/// The process-wide policy, if one was installed.
pub fn current() -> Option<&'static WritableGroups> {
    POLICY.get()
}

/// How a method's written-group set is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteClass {
    /// Never changes group data; never checked.
    ReadOnly,
    /// Client-invocable but deliberately not gated; the string says why.
    Exempt(&'static str),
    /// `params.group_id`; absent / null / non-string resolves to the default group.
    GroupParam,
    /// `params.group_id`, required and non-empty (extraction errors pass through).
    GroupRequired,
    /// `params.group_ids`, required non-empty array.
    GroupsRequired,
    /// `knowledge_rebuild_from_wal`: `group_id` (empty maps to default) plus foreign pointer owners.
    RebuildFromWal,
    /// `knowledge_delete_episode`: the group is looked up from the episode uuid.
    EpisodeOwner,
    /// `knowledge_rebind_pointers`: owning groups of pointer rows into `source_group_id`.
    PointerRebind,
    /// `knowledge_delete_by_group`: target groups plus foreign pointer owners.
    DeleteByGroup,
    /// Changes every group present.
    AllGroups,
    /// `knowledge_recover`: exempt while the DB is degraded (a degraded database must stay
    /// recoverable, ADR-0009), but on a healthy DB its strategies (`restore_from_backup`,
    /// `rebuild_from_workspace_wal`, `drop_lbug_wal`) replace or rewind every group's data, so it
    /// is treated like [`WriteClass::AllGroups`].
    Recover,
    /// Raw Cypher cannot be scoped by group: refused outright.
    RawCypher,
}

const RECOVERY: &str = "a no-op on a healthy database; on a degraded one it restores previously \
                        accepted state, and a degraded database must stay recoverable (ADR-0009)";
const FILE_LEVEL: &str = "touches only WAL/checkpoint files, indices or process state, never \
                          group rows";

/// Classification of **every** method `handlers::handle()` dispatches, read-only and exempt ones
/// included. A method missing from this table is refused (`unclassified_method`) whenever a list
/// is set, and the service crate's registry test fails if an MCP tool is missing.
pub const CLASSIFICATION: &[(&str, WriteClass)] = &[
    // ── read-only ──
    ("health_check", WriteClass::ReadOnly),
    ("knowledge_status", WriteClass::ReadOnly),
    ("knowledge_find_entities", WriteClass::ReadOnly),
    ("knowledge_resolve_entity", WriteClass::ReadOnly),
    ("knowledge_find_relationships", WriteClass::ReadOnly),
    ("knowledge_get_episodes", WriteClass::ReadOnly),
    ("knowledge_get_nodes_by_group", WriteClass::ReadOnly),
    ("knowledge_get_edges_by_group", WriteClass::ReadOnly),
    ("knowledge_get_edges_by_uuids", WriteClass::ReadOnly),
    ("knowledge_get_correction_changes", WriteClass::ReadOnly),
    ("knowledge_get_affected_work", WriteClass::ReadOnly),
    ("knowledge_rebuild_status", WriteClass::ReadOnly),
    ("knowledge_search_passages", WriteClass::ReadOnly),
    ("knowledge_list_entities", WriteClass::ReadOnly),
    ("knowledge_list_relationships", WriteClass::ReadOnly),
    ("knowledge_get_entity_neighbors", WriteClass::ReadOnly),
    ("knowledge_get_entities_by_source", WriteClass::ReadOnly),
    ("knowledge_validate_corrections", WriteClass::ReadOnly),
    // ── single group from params.group_id (omitted → default group) ──
    ("knowledge_add_episode", WriteClass::GroupParam),
    ("knowledge_process_chunk", WriteClass::GroupParam),
    ("knowledge_assert_entity", WriteClass::GroupParam),
    ("knowledge_assert_relationship", WriteClass::GroupParam),
    ("knowledge_merge_entities", WriteClass::GroupParam),
    // The edge's own group_id; `source`/`target` `source_group_id` are read-only pointers.
    ("knowledge_add_cross_group_edge", WriteClass::GroupParam),
    ("knowledge_reprocess_entity_types", WriteClass::GroupParam),
    ("knowledge_reprocess_relation_types", WriteClass::GroupParam),
    // ── single required group ──
    ("knowledge_supersede_document", WriteClass::GroupRequired),
    (
        "knowledge_canonicalize_relations",
        WriteClass::GroupRequired,
    ),
    (
        "knowledge_backfill_relation_types",
        WriteClass::GroupRequired,
    ),
    (
        "knowledge_backfill_summary_embeddings",
        WriteClass::GroupRequired,
    ),
    ("knowledge_reload_ontology", WriteClass::GroupRequired),
    // ── required group array ──
    ("knowledge_delete_by_source", WriteClass::GroupsRequired),
    ("knowledge_delete_chunk_episode", WriteClass::GroupsRequired),
    ("knowledge_mark_chunk_removed", WriteClass::GroupsRequired),
    // ── DB-derived / multi-group ──
    ("knowledge_rebuild_from_wal", WriteClass::RebuildFromWal),
    ("knowledge_delete_episode", WriteClass::EpisodeOwner),
    ("knowledge_rebind_pointers", WriteClass::PointerRebind),
    ("knowledge_delete_by_group", WriteClass::DeleteByGroup),
    ("knowledge_clear_all", WriteClass::AllGroups),
    // No group in the request and name resolution spans all groups → fail closed.
    ("knowledge_apply_corrections", WriteClass::AllGroups),
    // ── raw Cypher ──
    ("knowledge_query_cypher", WriteClass::RawCypher),
    // ── exempt ──
    ("knowledge_recover", WriteClass::Recover),
    ("knowledge_recover_full", WriteClass::Exempt(RECOVERY)),
    ("knowledge_close", WriteClass::Exempt(FILE_LEVEL)),
    (
        "knowledge_strip_wal_embeddings",
        WriteClass::Exempt(FILE_LEVEL),
    ),
    ("knowledge_build_indices", WriteClass::Exempt(FILE_LEVEL)),
    (
        "knowledge_prepare_checkpoint",
        WriteClass::Exempt(FILE_LEVEL),
    ),
    ("knowledge_dump_wal", WriteClass::Exempt(FILE_LEVEL)),
    ("knowledge_wal_mark_create", WriteClass::Exempt(FILE_LEVEL)),
    ("knowledge_wal_mark_list", WriteClass::Exempt(FILE_LEVEL)),
    ("knowledge_wal_mark_delete", WriteClass::Exempt(FILE_LEVEL)),
];

/// Looks up a method's classification.
pub fn classify(method: &str) -> Option<WriteClass> {
    CLASSIFICATION
        .iter()
        .find(|(m, _)| *m == method)
        .map(|(_, c)| *c)
}

fn not_writable(policy: &WritableGroups, method: &str, offending: Vec<String>) -> Error {
    let names = offending
        .iter()
        .map(|g| format!("'{g}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if offending.len() == 1 {
        "group"
    } else {
        "groups"
    };
    Error::WriteRefused {
        reason: "group_not_writable",
        message: format!(
            "{method} refused: {noun} {names} not writable: not in {FLAG} ({ENV_VAR}); \
             writable groups: [{}]",
            policy.list().join(", ")
        ),
        group_ids: offending,
    }
}

/// Returns `Err(WriteRefused)` for every group in `needed` that is not listed.
fn require_all<I: IntoIterator<Item = String>>(
    policy: &WritableGroups,
    method: &str,
    needed: I,
) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    let offending: Vec<String> = needed
        .into_iter()
        .filter(|g| !policy.contains(g) && seen.insert(g.clone()))
        .collect();
    if offending.is_empty() {
        Ok(())
    } else {
        Err(not_writable(policy, method, offending))
    }
}

/// Owning groups of every pointer row whose pointer targets one of `targets` as its
/// `source_group_id` (ADR-0385: those rows are rewritten by purges, rebuilds and rebinds even
/// though they belong to another group). Includes owners that are themselves in `targets`.
pub fn foreign_pointer_owners(
    conn: &Conn<'_>,
    targets: &BTreeSet<String>,
) -> Result<BTreeSet<String>, Error> {
    let mut owners = BTreeSet::new();
    for (_uuid, _name, rn_group_id, attrs) in conn.list_cross_group_pointer_candidates()? {
        let pointers = pointer::read_pointers(&attrs);
        let hits = [EndpointSide::Src, EndpointSide::Dst]
            .into_iter()
            .filter_map(|s| pointers.get(s))
            .any(|p| targets.contains(&p.source_group_id));
        if hits {
            owners.insert(rn_group_id);
        }
    }
    Ok(owners)
}

/// Every group present: distinct `group_id`s in the DB node tables plus group WAL directories.
fn all_groups_present(state: &AppState, conn: &Conn<'_>) -> Result<BTreeSet<String>, Error> {
    let mut groups = conn.list_distinct_group_ids()?;
    if let Some(root) = state.wal_root.as_deref() {
        for (gid, _) in wal_group::list_group_wal_dirs(root)? {
            groups.insert(gid);
        }
    }
    Ok(groups)
}

/// The guard. Returns `Ok(())` when the call may proceed.
///
/// Call only when a policy is set. Runs before any handler work.
pub async fn check_write_allowed(
    state: &Arc<AppState>,
    req: &IpcRequest,
    policy: &WritableGroups,
) -> Result<(), Error> {
    let method = req.method.as_str();
    let p = &req.params;
    let Some(class) = classify(method) else {
        return Err(Error::WriteRefused {
            reason: "unclassified_method",
            group_ids: Vec::new(),
            message: format!(
                "{method} refused: method has no write classification, so it cannot be \
                 proven safe under {FLAG} ({ENV_VAR})"
            ),
        });
    };

    match class {
        WriteClass::ReadOnly | WriteClass::Exempt(_) => Ok(()),
        WriteClass::GroupParam => {
            let gid = p["group_id"].as_str().unwrap_or(DEFAULT_GROUP_ID);
            require_all(policy, method, [gid.to_string()])
        }
        WriteClass::GroupRequired => {
            // An extraction error is passed through to the handler's own (identical) error.
            match extract_required_group_id(&p["group_id"]) {
                Ok(gid) => require_all(policy, method, [gid]),
                Err(_) => Ok(()),
            }
        }
        WriteClass::GroupsRequired => match extract_required_group_ids(&p["group_ids"]) {
            Ok(gids) => require_all(policy, method, gids),
            Err(_) => Ok(()),
        },
        WriteClass::RawCypher => Err(Error::WriteRefused {
            reason: "raw_cypher_disabled",
            group_ids: Vec::new(),
            message: format!(
                "{method} refused: raw Cypher cannot be restricted by group, so it is disabled \
                 while {FLAG} ({ENV_VAR}) is set; use the structured knowledge_* tools"
            ),
        }),
        WriteClass::RebuildFromWal => {
            let gid = group_id_param(p);
            let mut needed: BTreeSet<String> = BTreeSet::new();
            needed.insert(gid.clone());
            // Cheap pre-check first: the target group alone needs no DB read.
            require_all(policy, method, [gid])?;
            let owners = pointer_owners_of(state, needed).await?;
            require_all(policy, method, owners)
        }
        WriteClass::DeleteByGroup => match extract_required_group_ids(&p["group_ids"]) {
            Ok(gids) => {
                require_all(policy, method, gids.clone())?;
                let owners = pointer_owners_of(state, gids.into_iter().collect()).await?;
                require_all(policy, method, owners)
            }
            Err(_) => Ok(()),
        },
        WriteClass::PointerRebind => match p["source_group_id"].as_str() {
            // `source_group_id` itself is not written (ADR-0385); only the owners are.
            Some(src) => {
                let owners = pointer_owners_of(state, BTreeSet::from([src.to_string()])).await?;
                require_all(policy, method, owners)
            }
            None => Ok(()),
        },
        WriteClass::EpisodeOwner => {
            let uuid = p["episode_uuid"].as_str().unwrap_or("").to_string();
            let db = load_db(state)?;
            let owner = tokio::task::spawn_blocking(move || -> Result<Option<String>, Error> {
                db.connect()?.get_episode_group_id(&uuid)
            })
            .await??;
            // A missing episode is a no-op delete: nothing to protect.
            match owner {
                Some(gid) => require_all(policy, method, [gid]),
                None => Ok(()),
            }
        }
        WriteClass::AllGroups => require_all_groups_writable(state, method, policy).await,
        WriteClass::Recover => {
            // Degraded: the DB is unavailable, so there is no group data to protect and the
            // recovery path must stay open (ADR-0009). Healthy: whole-database replacement.
            if state.db.load_full().is_none() {
                Ok(())
            } else {
                require_all_groups_writable(state, method, policy).await
            }
        }
    }
}

/// Refuses unless every group present (DB groups plus WAL directories) is listed.
async fn require_all_groups_writable(
    state: &Arc<AppState>,
    method: &str,
    policy: &WritableGroups,
) -> Result<(), Error> {
    let db = load_db(state)?;
    let st = Arc::clone(state);
    let present = tokio::task::spawn_blocking(move || -> Result<BTreeSet<String>, Error> {
        let conn = db.connect()?;
        all_groups_present(&st, &conn)
    })
    .await??;
    let offending: Vec<String> = present
        .into_iter()
        .filter(|g| !policy.contains(g))
        .collect();
    if offending.is_empty() {
        Ok(())
    } else {
        Err(Error::WriteRefused {
            reason: "all_groups_required",
            message: format!(
                "{method} refused: it changes every group, and groups {} are not in \
                 {FLAG} ({ENV_VAR}); writable groups: [{}]",
                offending
                    .iter()
                    .map(|g| format!("'{g}'"))
                    .collect::<Vec<_>>()
                    .join(", "),
                policy.list().join(", ")
            ),
            group_ids: offending,
        })
    }
}

async fn pointer_owners_of(
    state: &Arc<AppState>,
    targets: BTreeSet<String>,
) -> Result<BTreeSet<String>, Error> {
    let db = load_db(state)?;
    tokio::task::spawn_blocking(move || -> Result<BTreeSet<String>, Error> {
        foreign_pointer_owners(&db.connect()?, &targets)
    })
    .await?
}

/// JSON `data` payload for a refused call (code -32004).
pub fn refusal_data(reason: &str, group_ids: &[String], policy: Option<&WritableGroups>) -> Value {
    serde_json::json!({
        "reason": reason,
        "group_ids": group_ids,
        "setting": FLAG,
        "env": ENV_VAR,
        "writable_groups": policy.map(|p| p.list()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_trims_and_dedupes() {
        let w = WritableGroups::parse(" a, b ,a").unwrap();
        assert_eq!(w.list(), vec!["a", "b"]);
        assert!(w.contains("a") && !w.contains("A") && !w.contains("c"));
    }

    #[test]
    fn parse_rejects_empty_and_malformed() {
        for bad in [
            "", "   ", "a,,b", ",a", "a,", ",", "a b", "a,b\tc", "a\u{0}b",
        ] {
            assert!(WritableGroups::parse(bad).is_err(), "{bad:?} must fail");
        }
    }

    #[test]
    fn resolve_flag_wins_and_env_not_parsed() {
        // The env value is malformed but never parsed because the flag wins.
        let w = WritableGroups::resolve(Some("a"), Some("a,,b"))
            .unwrap()
            .unwrap();
        assert_eq!(w.list(), vec!["a"]);
    }

    #[test]
    fn resolve_env_and_unset_and_empty() {
        assert!(WritableGroups::resolve(None, None).unwrap().is_none());
        let w = WritableGroups::resolve(None, Some("x,y")).unwrap().unwrap();
        assert_eq!(w.list(), vec!["x", "y"]);
        // Set-but-empty is an error, never "unset".
        assert!(WritableGroups::resolve(None, Some("")).is_err());
        assert!(WritableGroups::resolve(Some(""), None).is_err());
        let e = WritableGroups::resolve(None, Some("a,,b")).unwrap_err();
        assert!(e.contains(ENV_VAR), "{e}");
    }

    #[test]
    fn describe_lists_groups() {
        let w = WritableGroups::parse("b,a").unwrap();
        assert!(w.describe().contains("[a, b]"));
    }

    #[test]
    fn classification_has_no_duplicates() {
        let mut seen = BTreeSet::new();
        for (m, _) in CLASSIFICATION {
            assert!(seen.insert(*m), "duplicate classification for {m}");
        }
    }
}
