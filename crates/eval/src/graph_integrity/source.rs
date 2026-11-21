//! Reading the graph for `lcg-eval graph-integrity` (#819) — the only module that touches disk.
//!
//! Opening a lbug DB can replay and checkpoint its WAL, so the original is never opened: the DB
//! files are copied into a temp dir and the *copy* is opened in-process through `lcg_core::db::Db`.
//! No service process, embedder, extraction provider or network is involved, and the workspace is
//! only ever read (ADR-0819).

use std::fmt;
use std::path::{Path, PathBuf};

use lcg_core::db::Db;
use lcg_core::ontology::{
    load_group_ontology, load_ontology, load_ontology_from_path, Ontology, OntologyMode,
};
use lcg_core::types::{EntityRow, RelatesToEdge};

use super::score::RelSig;

/// DB locations tried, in order, relative to the workspace root.
pub const DB_CANDIDATES: [&str; 2] = [".lcg/db/liminis.db", ".graphiti/db"];

#[derive(Debug)]
pub enum SourceError {
    /// The workspace path does not exist or is not a directory.
    WorkspaceMissing(String),
    /// A directory, but no DB at any known location.
    NotIngested(String),
    /// A DB exists but cannot be copied, opened or queried (corrupt, degraded, wrong schema).
    Unreadable(String),
    /// The DB opened but holds no nodes in the requested group.
    EmptyGroup(String),
    /// The explicit `--ontology` could not be loaded.
    Ontology(String),
}

impl SourceError {
    /// Exit code: 3 for an unusable workspace, 1 for an ontology load failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            SourceError::Ontology(_) => 1,
            _ => 3,
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::WorkspaceMissing(m)
            | SourceError::NotIngested(m)
            | SourceError::Unreadable(m)
            | SourceError::EmptyGroup(m)
            | SourceError::Ontology(m) => write!(f, "{m}"),
        }
    }
}

pub struct Graph {
    pub nodes: Vec<EntityRow>,
    pub edges: Vec<RelatesToEdge>,
    pub signatures: Option<Vec<RelSig>>,
    /// `none`, `explicit`, `group` or `workspace`.
    pub ontology_source: &'static str,
}

/// Locates the DB inside `workspace`. `db_path` (relative to the workspace, or absolute) overrides
/// the default candidates.
pub fn find_db(workspace: &Path, db_path: Option<&Path>) -> Result<PathBuf, SourceError> {
    if !workspace.is_dir() {
        return Err(SourceError::WorkspaceMissing(format!(
            "workspace {} does not exist or is not a directory",
            workspace.display()
        )));
    }
    if let Some(p) = db_path {
        let full = workspace.join(p);
        return if full.exists() {
            Ok(full)
        } else {
            Err(SourceError::NotIngested(format!(
                "no database at {} (from --db-path)",
                full.display()
            )))
        };
    }
    DB_CANDIDATES
        .iter()
        .map(|c| workspace.join(c))
        .find(|p| p.exists())
        .ok_or_else(|| {
            SourceError::NotIngested(format!(
                "{} is not an ingested workspace: none of {} exists",
                workspace.display(),
                DB_CANDIDATES.join(", ")
            ))
        })
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Copies the DB at `src` into `dst_dir` and returns the path of the copy. A DB that is a file is
/// copied with its sidecars (every sibling named `<file>.<suffix>`, e.g. `.wal`); one that is a
/// directory is copied recursively. `src` is only read.
pub fn copy_db(src: &Path, dst_dir: &Path) -> std::io::Result<PathBuf> {
    let name = src
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "db path has no name")
        })?
        .to_owned();
    let dst = dst_dir.join(&name);
    if src.is_dir() {
        copy_dir(src, &dst)?;
        return Ok(dst);
    }
    std::fs::copy(src, &dst)?;
    let prefix = format!("{}.", name.to_string_lossy());
    if let Some(parent) = src.parent() {
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let fname = entry.file_name();
            if fname.to_string_lossy().starts_with(&prefix) && entry.file_type()?.is_file() {
                std::fs::copy(entry.path(), dst_dir.join(&fname))?;
            }
        }
    }
    Ok(dst)
}

/// Ontology resolution: an explicit file wins; otherwise the group's own ontology, then the
/// workspace-wide one (the order the service uses); otherwise none.
pub fn resolve_ontology(
    workspace: &Path,
    group: &str,
    explicit: Option<&Path>,
) -> Result<(Option<Ontology>, &'static str), SourceError> {
    if let Some(p) = explicit {
        let o = load_ontology_from_path(p, OntologyMode::Open).map_err(SourceError::Ontology)?;
        return Ok((Some(o), "explicit"));
    }
    if let Some(o) = load_group_ontology(workspace, group) {
        return Ok((Some(o), "group"));
    }
    if let Some(o) = load_ontology(Some(workspace)) {
        return Ok((Some(o), "workspace"));
    }
    Ok((None, "none"))
}

pub fn signatures_of(o: &Ontology) -> Vec<RelSig> {
    o.relation_types
        .iter()
        .map(|r| {
            // Same resolution as the ingest-time check (#816): declared types that exist in the
            // ontology, and symmetric relations are unconstrained.
            let side = |v: Vec<&str>| -> Vec<String> {
                if r.symmetric {
                    Vec::new()
                } else {
                    v.into_iter().map(String::from).collect()
                }
            };
            RelSig {
                name: r.name.clone(),
                source_types: side(o.effective_source_types(r)),
                target_types: side(o.effective_target_types(r)),
            }
        })
        .collect()
}

/// Reads every node and edge of `group` from a copy of the workspace DB.
pub fn read_graph(
    workspace: &Path,
    group: &str,
    db_path: Option<&Path>,
    ontology: Option<&Path>,
) -> Result<Graph, SourceError> {
    let db_file = find_db(workspace, db_path)?;
    let (onto, ontology_source) = resolve_ontology(workspace, group, ontology)?;
    let tmp = tempfile::tempdir()
        .map_err(|e| SourceError::Unreadable(format!("cannot create temp dir: {e}")))?;
    let copy = copy_db(&db_file, tmp.path()).map_err(|e| {
        SourceError::Unreadable(format!("cannot read database {}: {e}", db_file.display()))
    })?;
    let unreadable = |what: &str, e: &dyn fmt::Display| {
        SourceError::Unreadable(format!(
            "cannot {what} the database in {} (degraded or corrupt workspaces are not repaired by this command): {e}",
            workspace.display()
        ))
    };
    let db = Db::open(&copy.to_string_lossy()).map_err(|e| unreadable("open", &e))?;
    let conn = db.connect().map_err(|e| unreadable("open", &e))?;
    let gids = [group];
    let nodes = conn
        .get_entities_by_group_ids(Some(&gids))
        .map_err(|e| unreadable("read nodes from", &e))?;
    if nodes.is_empty() {
        return Err(SourceError::EmptyGroup(format!(
            "no nodes in group {group:?} in {}",
            workspace.display()
        )));
    }
    let edges = conn
        .get_edges_by_group_ids(Some(&gids))
        .map_err(|e| unreadable("read edges from", &e))?;
    Ok(Graph {
        nodes,
        edges,
        signatures: onto.as_ref().map(signatures_of),
        ontology_source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every file under `root` as (relative path → bytes), for byte-identity checks (SC-003).
    pub(crate) fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
                if p.is_dir() {
                    out.insert(format!("{rel}/"), Vec::new());
                    walk(&p, root, out);
                } else {
                    out.insert(rel, std::fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    #[test]
    fn missing_workspace_and_non_workspace_are_exit_3() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let e = read_graph(&missing, "g", None, None).err().unwrap();
        assert!(matches!(e, SourceError::WorkspaceMissing(_)));
        assert_eq!(e.exit_code(), 3);
        assert!(e.to_string().contains("does not exist"));

        std::fs::write(dir.path().join("note.md"), "x").unwrap();
        let before = snapshot(dir.path());
        let e = read_graph(dir.path(), "g", None, None).err().unwrap();
        assert!(matches!(e, SourceError::NotIngested(_)));
        assert_eq!(e.exit_code(), 3);
        assert!(e.to_string().contains("not an ingested workspace"));
        assert_eq!(
            snapshot(dir.path()),
            before,
            "failed run must not touch the workspace"
        );
    }

    #[test]
    fn corrupt_db_is_unreadable_exit_3_and_workspace_untouched() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".lcg/db")).unwrap();
        std::fs::write(dir.path().join(".lcg/db/liminis.db"), b"not a database").unwrap();
        let before = snapshot(dir.path());
        let e = read_graph(dir.path(), "g", None, None).err().unwrap();
        assert!(matches!(e, SourceError::Unreadable(_)), "{e}");
        assert_eq!(e.exit_code(), 3);
        assert_eq!(snapshot(dir.path()), before);
    }

    #[test]
    fn copy_db_copies_file_and_sidecars_only() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("liminis.db"), b"db").unwrap();
        std::fs::write(src.path().join("liminis.db.wal"), b"wal").unwrap();
        std::fs::write(src.path().join("other.db"), b"other").unwrap();
        let before = snapshot(src.path());
        let copy = copy_db(&src.path().join("liminis.db"), dst.path()).unwrap();
        assert_eq!(copy, dst.path().join("liminis.db"));
        let got = snapshot(dst.path());
        assert_eq!(
            got.keys().cloned().collect::<Vec<_>>(),
            vec!["liminis.db", "liminis.db.wal"]
        );
        assert_eq!(got["liminis.db.wal"], b"wal");
        assert_eq!(snapshot(src.path()), before);
    }

    #[test]
    fn copy_db_copies_directory_dbs_recursively() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join("db/sub")).unwrap();
        std::fs::write(src.path().join("db/a"), b"a").unwrap();
        std::fs::write(src.path().join("db/sub/b"), b"b").unwrap();
        copy_db(&src.path().join("db"), dst.path()).unwrap();
        assert_eq!(std::fs::read(dst.path().join("db/sub/b")).unwrap(), b"b");
    }

    #[test]
    fn explicit_db_path_override() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            find_db(dir.path(), Some(Path::new("x.db"))),
            Err(SourceError::NotIngested(_))
        ));
        std::fs::write(dir.path().join("x.db"), b"").unwrap();
        assert_eq!(
            find_db(dir.path(), Some(Path::new("x.db"))).unwrap(),
            dir.path().join("x.db")
        );
    }

    #[test]
    fn explicit_ontology_failure_is_exit_1() {
        let dir = tempfile::tempdir().unwrap();
        let e = resolve_ontology(dir.path(), "g", Some(&dir.path().join("missing.yaml")))
            .err()
            .unwrap();
        assert_eq!(e.exit_code(), 1);
        let (o, src) = resolve_ontology(dir.path(), "g", None).unwrap();
        assert!(o.is_none());
        assert_eq!(src, "none");
    }

    /// Real lbug round trip: build a tiny ingested workspace, read it back through `read_graph`,
    /// and verify the workspace is byte-identical afterwards (SC-003).
    #[test]
    fn reads_a_real_workspace_without_modifying_it() {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path().join(".lcg/db");
        std::fs::create_dir_all(&db_dir).unwrap();
        {
            let db = Db::open(db_dir.join("liminis.db").to_str().unwrap()).unwrap();
            let conn = db.connect().unwrap();
            conn.init_schema(4).unwrap();
            for (uuid, name, label, group) in [
                ("u1", "Cedar Core", "Project", "g"),
                ("u2", "Lena Moreau", "Person", "g"),
                ("u3", "Other Group Node", "Person", "other"),
            ] {
                conn.insert_entity(&EntityRow {
                    uuid: uuid.into(),
                    name: name.into(),
                    group_id: group.into(),
                    labels: vec!["Entity".into(), label.into()],
                    created_at: "2025-01-01 00:00:00".into(),
                    name_embedding: vec![0.0; 4],
                    attributes: "{}".into(),
                    ..Default::default()
                })
                .unwrap();
            }
            conn.insert_relates_to_edge(&RelatesToEdge {
                uuid: "e1".into(),
                name: "OWNED_BY".into(),
                source_node_uuid: "u1".into(),
                target_node_uuid: "u2".into(),
                group_id: "g".into(),
                fact: "Lena Moreau owns Cedar Core".into(),
                fact_embedding: vec![0.0; 4],
                created_at: "2025-01-01 00:00:00".into(),
                attributes: "{}".into(),
                relation_type: Some("OWNED_BY".into()),
                ..Default::default()
            })
            .unwrap();
        }
        let before = snapshot(dir.path());
        let g = read_graph(dir.path(), "g", None, None).unwrap();
        assert_eq!(g.nodes.len(), 2, "only the requested group is read");
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].fact, "Lena Moreau owns Cedar Core");
        assert_eq!(g.ontology_source, "none");
        assert_eq!(
            snapshot(dir.path()),
            before,
            "workspace must be byte-identical"
        );

        let e = read_graph(dir.path(), "absent-group", None, None)
            .err()
            .unwrap();
        assert!(matches!(e, SourceError::EmptyGroup(_)));
        assert_eq!(snapshot(dir.path()), before);
    }
}
