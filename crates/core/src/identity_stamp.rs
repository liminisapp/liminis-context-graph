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
//! rebuild-from-WAL into a fresh directory, none of which a `WalPosition` column would (an
//! absent record reads as the empty set, which would wrongly accept a flag flip on a group
//! whose entities already carry extracted identity kinds). See ADR-0616.

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

/// Pure D1 decision. `changed` is [`changed_labels`]; `count` returns how many existing entities
/// in the group carry a label. The first changed label with any carrier refuses; otherwise the
/// change is accepted (`Ok`) and the caller records `current`.
pub fn check_identity_change<E>(
    changed: &[(String, bool)],
    mut count: impl FnMut(&str) -> Result<usize, E>,
) -> Result<Option<IdentityRefusal>, E> {
    for (label, adding) in changed {
        let n = count(label)?;
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
    fn equal_sets_have_no_changed_labels() {
        assert!(changed_labels(&set(&["Person"]), &set(&["Person"])).is_empty());
        assert!(changed_labels(&set(&[]), &set(&[])).is_empty());
    }

    #[test]
    fn uncarried_change_is_accepted() {
        let ch = changed_labels(&set(&[]), &set(&["Organization"]));
        let r = check_identity_change::<()>(&ch, |_| Ok(0)).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn carried_add_and_remove_are_refused() {
        let add = changed_labels(&set(&[]), &set(&["Person"]));
        let r = check_identity_change::<()>(&add, |_| Ok(3))
            .unwrap()
            .unwrap();
        assert_eq!((r.label.as_str(), r.count, r.adding), ("Person", 3, true));

        let rem = changed_labels(&set(&["Person"]), &set(&[]));
        let r = check_identity_change::<()>(&rem, |_| Ok(1))
            .unwrap()
            .unwrap();
        assert!(!r.adding);
        let msg = r.message("g1");
        assert!(msg.contains("Person"), "{msg}");
        assert!(msg.contains("1 existing entity"), "{msg}");
        assert!(msg.contains("re-ingest"), "{msg}");
    }

    #[test]
    fn count_error_propagates() {
        let ch = changed_labels(&set(&[]), &set(&["Person"]));
        assert!(check_identity_change(&ch, |_| Err("db down")).is_err());
    }
}
