//! Per-group identity-bearing set stamp (issue #616, D1/D2).
//!
//! An ontology may mark entity types `identity: true`, so extraction assigns them as the
//! entity's `kind`. Kind is decided once at creation and never re-derived, so a change to a
//! group's identity-bearing set that would reinterpret existing data is refused (the #414
//! principle applied to identity). This module owns the durable record of the set that was in
//! force — the *stamp* — and the pure comparison logic. The DB-touching part (counting carriers)
//! lives in `app_state`.
//!
//! The stamp is a per-group sidecar file, `.lcg/identity-set/<encoded group>.json`, deliberately
//! outside the WAL and outside the lbug DB: it survives `knowledge_clear_all`, a DB reset and a
//! rebuild-from-WAL **into the same workspace root**, none of which a `WalPosition` column
//! would. It does *not* travel with the WAL: a host that keeps only the WAL (an ephemeral
//! container, a fresh clone) starts with no stamp, which reads as the empty set. Two things
//! cover that (#776, ADR-0776): the D1 carrier count is kind-aware, so a replayed group whose
//! entities already carry the identity kinds is not refused; and the WAL ontology sidecar
//! records the identity-bearing set, which [`restore_stamp_if_absent`] writes back as the stamp
//! after a rebuild. Since #778 (ADR-0778) the restore also runs on every first resolve of a
//! group with no stamp, and the stored entity kinds are checked ([`merge_stored_kinds`]) so an
//! absent stamp cannot hide an identity removal. See ADR-0616, ADR-0776 and ADR-0778.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Error;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct IdentityStamp {
    /// Sorted, normalized identity-bearing entity type names.
    pub identity_types: Vec<String>,
}

/// A refused identity-set change: the first label (alphabetical) whose identity status would
/// change while existing entities in the group carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRefusal {
    pub label: String,
    pub count: usize,
    /// `true` when the flag is being added, `false` when it is being removed.
    pub adding: bool,
}

impl IdentityRefusal {
    pub fn message(&self, group_id: &str) -> String {
        format!(
            "identity-bearing set change refused for group {group_id:?}: {} the `identity` flag \
             for type {:?} would reinterpret {} existing entit{} carrying that label. Identity is \
             decided once, at creation, and never inferred afterwards. Restore the group's \
             previously recorded identity-bearing set, or re-ingest the group from source.",
            if self.adding { "adding" } else { "removing" },
            self.label,
            self.count,
            if self.count == 1 { "y" } else { "ies" },
        )
    }

    pub fn into_error(self, group_id: &str) -> Error {
        let message = self.message(group_id);
        Error::IdentitySetChangeRefused {
            group_id: group_id.to_string(),
            label: self.label,
            count: self.count,
            message,
        }
    }
}

/// `{workspace_root}/.lcg/identity-set/<encoded_group_id>.json`, using the same group-dir
/// encoding as the drift sidecars.
pub fn stamp_path(workspace_root: &Path, group_id: &str) -> Result<PathBuf, Error> {
    let encoded = crate::wal_group::encode_group_dir_name(group_id)?;
    Ok(workspace_root
        .join(".lcg")
        .join("identity-set")
        .join(format!("{encoded}.json")))
}

/// Reads a group's recorded identity-bearing set. A missing file is `Ok(∅)` — every entity
/// extracted before #616 was created with the default kind, so absent ≡ empty. A file that
/// exists but cannot be read or parsed is `Err`: the recorded set is unknown, and guessing
/// would risk silently accepting a reinterpreting change.
pub fn read_stamp(workspace_root: &Path, group_id: &str) -> Result<BTreeSet<String>, String> {
    let path = stamp_path(workspace_root, group_id).map_err(|e| e.to_string())?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(format!("cannot read identity stamp {path:?}: {e}")),
    };
    serde_json::from_str::<IdentityStamp>(&text)
        .map(|s| s.identity_types.into_iter().collect())
        .map_err(|e| format!("cannot parse identity stamp {path:?}: {e}"))
}

/// Atomically writes a group's identity-bearing set (unique temp name, like
/// `ontology_sidecar::write_group_sidecar`, because concurrent first-resolutions may race).
pub fn write_stamp(
    workspace_root: &Path,
    group_id: &str,
    set: &BTreeSet<String>,
) -> std::io::Result<()> {
    let path = stamp_path(workspace_root, group_id).map_err(std::io::Error::other)?;
    let dir = path.parent().expect("stamp_path always has a parent");
    std::fs::create_dir_all(dir)?;
    let stamp = IdentityStamp {
        identity_types: set.iter().cloned().collect(),
    };
    let json = serde_json::to_string_pretty(&stamp).map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.flush()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Removes a group's stamp (`knowledge_delete_by_group`). Missing is fine.
pub fn remove_stamp(workspace_root: &Path, group_id: &str) {
    if let Ok(path) = stamp_path(workspace_root, group_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Removes every group's stamp (`knowledge_clear_all`).
pub fn remove_all_stamps(workspace_root: &Path) {
    let _ = std::fs::remove_dir_all(workspace_root.join(".lcg").join("identity-set"));
}

/// Labels whose identity status differs between `recorded` and `current` (the symmetric
/// difference), sorted; each is tagged `true` if it is being added.
pub fn changed_labels(
    recorded: &BTreeSet<String>,
    current: &BTreeSet<String>,
) -> Vec<(String, bool)> {
    let mut v: Vec<(String, bool)> = current
        .difference(recorded)
        .map(|l| (l.clone(), true))
        .chain(recorded.difference(current).map(|l| (l.clone(), false)))
        .collect();
    v.sort();
    v
}

/// Unions stored entity kinds into `changed` as removals (#778). Used when the stamp is absent:
/// every stored non-default kind that is not in `current` was identity-bearing when the entity
/// was created, so dropping it from the set is a removal, whatever the stamp says. A kind
/// already in `changed` is not duplicated. The result is sorted.
pub fn merge_stored_kinds(
    mut changed: Vec<(String, bool)>,
    stored_kinds: &[String],
    current: &BTreeSet<String>,
) -> Vec<(String, bool)> {
    for kind in stored_kinds {
        if !current.contains(kind) && !changed.iter().any(|(l, _)| l == kind) {
            changed.push((kind.clone(), false));
        }
    }
    changed.sort();
    changed
}

/// Restores a missing stamp from the group's WAL ontology sidecar (#776 after a
/// rebuild-from-WAL; #778 on every first resolve of the group). Never overwrites: any existing
/// stamp file — even an unparseable one — is left untouched. Restores nothing (and raises nothing) when the sidecar is missing, predates the
/// `identity_types` field, or records an empty set (a flagless group has no stamp file).
/// Returns `true` only when a stamp was written.
pub fn restore_stamp_if_absent(workspace_root: &Path, wal_dir: &Path, group_id: &str) -> bool {
    let Ok(path) = stamp_path(workspace_root, group_id) else {
        return false;
    };
    if path.exists() {
        return false;
    }
    let Some(types) =
        crate::ontology_sidecar::read_wal_ontology_sidecar(wal_dir).and_then(|s| s.identity_types)
    else {
        return false;
    };
    if types.is_empty() {
        return false;
    }
    let set: BTreeSet<String> = types.into_iter().collect();
    match write_stamp(workspace_root, group_id, &set) {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "liminis-context-graph: identity-stamp: cannot restore stamp for group {group_id:?}: {e}"
            );
            false
        }
    }
}

/// Pure D1 decision. `changed` is [`changed_labels`]; `count(label, adding)` returns how many
/// existing entities in the group would be reinterpreted by that label's change (kind-aware,
/// #776). The first changed label with any carrier refuses; otherwise the change is accepted
/// (`Ok`) and the caller records `current`.
pub fn check_identity_change<E>(
    changed: &[(String, bool)],
    mut count: impl FnMut(&str, bool) -> Result<usize, E>,
) -> Result<Option<IdentityRefusal>, E> {
    for (label, adding) in changed {
        let n = count(label, *adding)?;
        if n > 0 {
            return Ok(Some(IdentityRefusal {
                label: label.clone(),
                count: n,
                adding: *adding,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn roundtrip_and_absent_is_empty() {
        let d = TempDir::new().unwrap();
        assert_eq!(read_stamp(d.path(), "g").unwrap(), set(&[]));
        write_stamp(d.path(), "g", &set(&["Person", "Organization"])).unwrap();
        assert_eq!(
            read_stamp(d.path(), "g").unwrap(),
            set(&["Organization", "Person"])
        );
        assert_eq!(read_stamp(d.path(), "other").unwrap(), set(&[]));
        remove_stamp(d.path(), "g");
        assert_eq!(read_stamp(d.path(), "g").unwrap(), set(&[]));
    }

    #[test]
    fn corrupt_stamp_is_err_not_empty() {
        let d = TempDir::new().unwrap();
        let p = stamp_path(d.path(), "g").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        assert!(read_stamp(d.path(), "g").is_err());
    }

    #[test]
    fn merge_stored_kinds_unions_removals() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Empty input leaves `changed` unchanged.
        assert!(merge_stored_kinds(vec![], &[], &set(&["Person"])).is_empty());
        // A kind still in `current` is skipped.
        assert!(merge_stored_kinds(vec![], &s(&["Person"]), &set(&["Person"])).is_empty());
        // A kind absent from `current` becomes a removal; result sorted.
        let m = merge_stored_kinds(
            vec![("Zeta".into(), true)],
            &s(&["Person", "Organization"]),
            &set(&["Zeta"]),
        );
        assert_eq!(
            m,
            vec![
                ("Organization".to_string(), false),
                ("Person".to_string(), false),
                ("Zeta".to_string(), true),
            ]
        );
        // A kind already in `changed` is not duplicated.
        let m = merge_stored_kinds(vec![("Person".into(), false)], &s(&["Person"]), &set(&[]));
        assert_eq!(m, vec![("Person".to_string(), false)]);
    }

    #[test]
    fn equal_sets_have_no_changed_labels() {
        assert!(changed_labels(&set(&["Person"]), &set(&["Person"])).is_empty());
        assert!(changed_labels(&set(&[]), &set(&[])).is_empty());
    }

    #[test]
    fn uncarried_change_is_accepted() {
        let ch = changed_labels(&set(&[]), &set(&["Organization"]));
        let r = check_identity_change::<()>(&ch, |_, _| Ok(0)).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn carried_add_and_remove_are_refused() {
        let add = changed_labels(&set(&[]), &set(&["Person"]));
        let r = check_identity_change::<()>(&add, |_, _| Ok(3))
            .unwrap()
            .unwrap();
        assert_eq!((r.label.as_str(), r.count, r.adding), ("Person", 3, true));

        let rem = changed_labels(&set(&["Person"]), &set(&[]));
        let r = check_identity_change::<()>(&rem, |_, _| Ok(1))
            .unwrap()
            .unwrap();
        assert!(!r.adding);
        let msg = r.message("g1");
        assert!(msg.contains("Person"), "{msg}");
        assert!(msg.contains("1 existing entity"), "{msg}");
        assert!(msg.contains("re-ingest"), "{msg}");
    }

    #[test]
    fn count_receives_direction() {
        let ch = changed_labels(&set(&["Old"]), &set(&["New"]));
        let mut seen = vec![];
        let r = check_identity_change::<()>(&ch, |l, adding| {
            seen.push((l.to_string(), adding));
            Ok(0)
        })
        .unwrap();
        assert!(r.is_none());
        assert_eq!(
            seen,
            vec![("New".to_string(), true), ("Old".to_string(), false)]
        );
    }

    fn write_wal_sidecar(wal: &Path, identity: Option<Vec<&str>>) {
        let json = match identity {
            Some(v) => {
                serde_json::json!({"hash":"h","mode":null,"entity_types":[],"relation_types":[],"identity_types":v})
            }
            None => {
                serde_json::json!({"hash":"h","mode":null,"entity_types":[],"relation_types":[]})
            }
        };
        std::fs::create_dir_all(wal).unwrap();
        std::fs::write(
            crate::ontology_sidecar::wal_ontology_path(wal),
            json.to_string(),
        )
        .unwrap();
    }

    #[test]
    fn restore_writes_absent_stamp_from_sidecar() {
        let root = TempDir::new().unwrap();
        let wal = TempDir::new().unwrap();
        write_wal_sidecar(wal.path(), Some(vec!["Person", "Organization"]));
        assert!(restore_stamp_if_absent(root.path(), wal.path(), "g"));
        assert_eq!(
            read_stamp(root.path(), "g").unwrap(),
            set(&["Organization", "Person"])
        );
    }

    #[test]
    fn restore_never_overwrites_existing_stamp() {
        let root = TempDir::new().unwrap();
        let wal = TempDir::new().unwrap();
        write_wal_sidecar(wal.path(), Some(vec!["Person"]));
        write_stamp(root.path(), "g", &set(&["Other"])).unwrap();
        let p = stamp_path(root.path(), "g").unwrap();
        let before = std::fs::read(&p).unwrap();
        assert!(!restore_stamp_if_absent(root.path(), wal.path(), "g"));
        assert_eq!(std::fs::read(&p).unwrap(), before);
    }

    #[test]
    fn restore_leaves_corrupt_stamp_untouched() {
        let root = TempDir::new().unwrap();
        let wal = TempDir::new().unwrap();
        write_wal_sidecar(wal.path(), Some(vec!["Person"]));
        let p = stamp_path(root.path(), "g").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        assert!(!restore_stamp_if_absent(root.path(), wal.path(), "g"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "not json");
    }

    #[test]
    fn restore_is_noop_for_old_sidecar_missing_sidecar_and_empty_set() {
        let root = TempDir::new().unwrap();
        let wal = TempDir::new().unwrap();
        // No sidecar at all.
        assert!(!restore_stamp_if_absent(root.path(), wal.path(), "g"));
        // Older sidecar without the field.
        write_wal_sidecar(wal.path(), None);
        assert!(!restore_stamp_if_absent(root.path(), wal.path(), "g"));
        // Empty recorded set: no stamp file for a flagless group.
        write_wal_sidecar(wal.path(), Some(vec![]));
        assert!(!restore_stamp_if_absent(root.path(), wal.path(), "g"));
        assert!(!stamp_path(root.path(), "g").unwrap().exists());
    }

    #[test]
    fn count_error_propagates() {
        let ch = changed_labels(&set(&[]), &set(&["Person"]));
        assert!(check_identity_change(&ch, |_, _| Err("db down")).is_err());
    }
}
