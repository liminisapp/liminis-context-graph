//! Ontology-constrained ingest and kept workspaces for `lcg-eval retrieval` (#821, ADR-0821).
//!
//! * **Ontology staging.** The service reads `<workspace>/.lcg/ontology.yaml` at startup and honours
//!   the *file's* declared `mode:`. So `--ontology F --ontology-mode M` writes a copy of F with its
//!   `mode:` set to M, and checks that what the service will load hashes the same as `F` loaded
//!   under `M` — a staging mismatch is a loud failure, never a silently mislabelled report.
//! * **Cassette sidecar.** A cassette's records carry no ontology, so the hash a cassette was
//!   recorded under lives next to it in `<cassette>.meta.json`. Replay checks it before ingest.
//!   Cassette records and keys are untouched (ADR-0044).
//! * **Kept workspace manifest.** `lcg-eval-workspace.json` records how a kept workspace was built
//!   (corpus, ontology, embedder, cassette), so `graph-qa` can refuse a mismatched embedder.

use std::path::{Path, PathBuf};

use lcg_core::ontology::{content_hash, load_ontology, load_ontology_from_path};
use lcg_core::OntologyMode;
use serde::{Deserialize, Serialize};

/// Where the service looks for the workspace-wide ontology.
pub const ONTOLOGY_FILE: &str = ".lcg/ontology.yaml";
/// The kept-workspace manifest, at the workspace root.
pub const MANIFEST_FILE: &str = "lcg-eval-workspace.json";
/// The service's socket, relative to the workspace; never copied into a kept workspace.
pub const SOCKET_FILE: &str = ".lcg/service.sock";

/// The hash and mode an ingest ran under. `hash` is [`content_hash`] (`"none"` without one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OntologyIdentity {
    pub hash: String,
    /// `Some("strict" | "open")`, `None` for a free-form ingest.
    pub mode: Option<String>,
}

impl OntologyIdentity {
    pub fn none() -> Self {
        Self {
            hash: content_hash(None),
            mode: None,
        }
    }
}

/// The identity of `--ontology F --ontology-mode M` (or none). Loads `F`, so a missing or
/// malformed file is an error here, before anything is started.
pub fn ontology_identity(
    file: Option<&Path>,
    mode: Option<&OntologyMode>,
) -> Result<OntologyIdentity, String> {
    let Some(f) = file else {
        return Ok(OntologyIdentity::none());
    };
    let mode = mode.cloned().unwrap_or(OntologyMode::Strict);
    let o = load_ontology_from_path(f, mode.clone())
        .map_err(|e| format!("--ontology {}: {e}", f.display()))?;
    Ok(OntologyIdentity {
        hash: content_hash(Some(&o)),
        mode: Some(mode.to_string()),
    })
}

/// Writes `src` into `ws` as `.lcg/ontology.yaml` with `mode:` set to `mode`, then checks that the
/// file the service will load hashes to `expected`.
pub fn stage_ontology(
    src: &Path,
    mode: &OntologyMode,
    ws: &Path,
    expected: &str,
) -> Result<(), String> {
    let text =
        std::fs::read_to_string(src).map_err(|e| format!("--ontology {}: {e}", src.display()))?;
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| format!("--ontology {}: not YAML: {e}", src.display()))?;
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| format!("--ontology {}: not a YAML mapping", src.display()))?;
    map.insert(
        serde_yaml::Value::String("mode".into()),
        serde_yaml::Value::String(mode.to_string()),
    );
    let out = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
    let dest = ws.join(ONTOLOGY_FILE);
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    std::fs::write(&dest, out).map_err(|e| format!("{}: {e}", dest.display()))?;
    let staged = content_hash(load_ontology(Some(ws)).as_ref());
    if staged != expected {
        return Err(format!(
            "the ontology staged in the workspace hashes to {staged}, not {expected} (the hash of \
             --ontology {} in mode {mode}); refusing to ingest under a mislabelled ontology",
            src.display()
        ));
    }
    Ok(())
}

// ── cassette sidecar ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CassetteMeta {
    pub format: u32,
    /// [`content_hash`] of the ontology the cassette was recorded under (`"none"` without one).
    pub ontology_hash: String,
    pub ontology_mode: Option<String>,
    pub lcg_build: String,
    /// `--ingest-concurrency` the cassette was recorded at (#900). Absent for a sequential
    /// recording, so an N = 1 sidecar is byte-identical to one written before the flag existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingest_concurrency: Option<usize>,
    /// Recorded with more than one chunk in flight (#900). The `consolidate_summary` and
    /// `judge_duplicates` keys hash graph state, which depends on commit order, so a replay can
    /// miss them; the service swallows those misses and degrades silently.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub order_dependent: bool,
}

/// `<cassette>.meta.json`.
pub fn cassette_meta_path(cassette: &Path) -> PathBuf {
    let mut s = cassette.as_os_str().to_owned();
    s.push(".meta.json");
    PathBuf::from(s)
}

pub fn write_cassette_meta(
    cassette: &Path,
    id: &OntologyIdentity,
    lcg_build: &str,
    ingest_concurrency: usize,
) -> Result<(), String> {
    let concurrent = ingest_concurrency > 1;
    let m = CassetteMeta {
        format: 1,
        ontology_hash: id.hash.clone(),
        ontology_mode: id.mode.clone(),
        lcg_build: lcg_build.to_string(),
        ingest_concurrency: concurrent.then_some(ingest_concurrency),
        order_dependent: concurrent,
    };
    let p = cassette_meta_path(cassette);
    std::fs::write(
        &p,
        serde_json::to_string_pretty(&m).expect("serializes") + "\n",
    )
    .map_err(|e| format!("cannot write {}: {e}", p.display()))
}

pub fn read_cassette_meta(cassette: &Path) -> Result<Option<CassetteMeta>, String> {
    let p = cassette_meta_path(cassette);
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t)
            .map(Some)
            .map_err(|e| format!("{}: unreadable cassette sidecar: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// What replaying `cassette` under `run` means (FR-004, FR-005): `Err` names both hashes when the
/// sidecar disagrees; `Ok(Some(warning))` when the hash cannot be verified; `Ok(None)` otherwise.
pub fn preflight_cassette(
    cassette: &Path,
    run: &OntologyIdentity,
) -> Result<Option<String>, String> {
    match read_cassette_meta(cassette)? {
        Some(m) if m.ontology_hash != run.hash => Err(format!(
            "--replay-cassette {} was recorded under ontology hash {} but this run's ontology hash \
             is {} ({}); every extraction call would miss. Re-record the cassette, or replay it \
             with the ontology it was recorded under",
            cassette.display(),
            m.ontology_hash,
            run.hash,
            match &run.mode {
                Some(mode) => format!("--ontology in mode {mode}"),
                None => "no --ontology".to_string(),
            }
        )),
        Some(m) if m.order_dependent => Ok(Some(format!(
            "--replay-cassette {} was recorded with --ingest-concurrency {} and is order-dependent: \
             its consolidate_summary and judge_duplicates keys hash graph state that depended on \
             the order chunks committed in, so some may miss on replay, and the service degrades \
             those misses silently (no error, a possibly different graph). Replay always runs at \
             ingest concurrency 1",
            cassette.display(),
            m.ingest_concurrency
                .map_or_else(|| "> 1".to_string(), |n| n.to_string()),
        ))),
        Some(_) => Ok(None),
        None if run.mode.is_some() => Ok(Some(format!(
            "--replay-cassette {} has no {} sidecar (recorded before ontology hashes were \
             recorded), so its ontology cannot be verified against this run's ({})",
            cassette.display(),
            cassette_meta_path(cassette)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            run.hash
        ))),
        None => Ok(None),
    }
}

// ── kept-workspace manifest ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkspaceManifest {
    pub format: u32,
    pub group_id: String,
    pub corpus_version: String,
    pub corpus_hash: String,
    pub ontology_hash: String,
    pub ontology_mode: Option<String>,
    /// The embedder the workspace's vectors were made with (URL or socket as configured).
    pub embedder: String,
    pub embedding_profile: Option<String>,
    pub extractor: String,
    pub cassette: Option<String>,
    pub lcg_build: String,
}

pub fn write_manifest(ws: &Path, m: &WorkspaceManifest) -> Result<(), String> {
    let p = ws.join(MANIFEST_FILE);
    std::fs::write(
        &p,
        serde_json::to_string_pretty(m).expect("serializes") + "\n",
    )
    .map_err(|e| format!("cannot write {}: {e}", p.display()))
}

pub fn read_manifest(ws: &Path) -> Result<Option<WorkspaceManifest>, String> {
    let p = ws.join(MANIFEST_FILE);
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t)
            .map(Some)
            .map_err(|e| format!("{}: unreadable workspace manifest: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

// ── copying ─────────────────────────────────────────────────────────────────────

/// Recursively copies `src` into `dst` (created), skipping the service socket. `src` is only read.
pub fn copy_workspace(src: &Path, dst: &Path) -> std::io::Result<()> {
    copy_tree(src, dst, &src.join(SOCKET_FILE))
}

fn copy_tree(src: &Path, dst: &Path, skip: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path == skip {
            continue;
        }
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_tree(&path, &to, skip)?;
        } else if ft.is_file() {
            std::fs::copy(&path, &to)?;
        }
        // Sockets, FIFOs and symlinks are not workspace state.
    }
    Ok(())
}

/// `addr` with any userinfo, query and fragment removed when it is an `http(s)` URL, so an endpoint
/// that carries a credential (`http://user:token@host/…`, `…?key=…`) is never written to disk or
/// printed. Anything else (a socket path) is returned trimmed.
pub fn redact_endpoint(addr: &str) -> String {
    let s = addr.trim();
    let Some((scheme, rest)) = s.split_once("://") else {
        return s.to_string();
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return s.to_string();
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    format!("{scheme}://{host}{path}")
}

/// Moves the workspace at `src` to `dest` (which must not exist, or be an empty directory):
/// `rename` when possible, a copy then removal across devices. The socket is never kept.
///
/// On failure the workspace is left at `src` and the error names it, so an expensive ingest is
/// never stranded in an unnamed temp dir; a partial copy at `dest` is removed.
pub fn persist_workspace(src: &Path, dest: &Path) -> Result<(), String> {
    move_workspace(src, dest).map_err(|e| {
        format!(
            "{e}; the ingested workspace is still at {} (move it there by hand)",
            src.display()
        )
    })
}

fn move_workspace(src: &Path, dest: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(src.join(SOCKET_FILE));
    if dest.exists() {
        // `check_keep_dir` already refused a non-empty one; an empty one is replaced.
        std::fs::remove_dir(dest).map_err(|e| {
            format!(
                "--keep-workspace {}: cannot replace the directory: {e}",
                dest.display()
            )
        })?;
    }
    if let Some(p) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    if std::fs::rename(src, dest).is_ok() {
        return Ok(());
    }
    if let Err(e) = copy_workspace(src, dest) {
        // `dest` did not exist (or was an empty dir we removed), so everything in it is ours.
        let _ = std::fs::remove_dir_all(dest);
        return Err(format!(
            "cannot copy the workspace to --keep-workspace {}: {e}",
            dest.display()
        ));
    }
    let _ = std::fs::remove_dir_all(src);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONTO: &str = "mode: strict\nentity_types:\n  - name: Person\n    description: A person.\n  - name: Project\n    description: A project.\nrelation_types:\n  - name: OWNS\n    description: A person owns a project.\n    source_type: Person\n    target_type: Project\n";

    fn onto_file(dir: &Path) -> PathBuf {
        let p = dir.join("onto.yaml");
        std::fs::write(&p, ONTO).unwrap();
        p
    }

    #[test]
    fn staged_ontology_hashes_like_the_source_in_the_requested_mode() {
        let d = tempfile::tempdir().unwrap();
        let f = onto_file(d.path());
        for mode in [OntologyMode::Strict, OntologyMode::Open] {
            let ws = tempfile::tempdir().unwrap();
            let id = ontology_identity(Some(&f), Some(&mode)).unwrap();
            stage_ontology(&f, &mode, ws.path(), &id.hash).unwrap();
            assert_eq!(
                content_hash(load_ontology(Some(ws.path())).as_ref()),
                id.hash
            );
            assert_eq!(id.mode.as_deref(), Some(mode.to_string().as_str()));
        }
        let strict = ontology_identity(Some(&f), Some(&OntologyMode::Strict)).unwrap();
        let open = ontology_identity(Some(&f), Some(&OntologyMode::Open)).unwrap();
        assert_ne!(strict.hash, open.hash, "the mode is part of the hash");
        assert_eq!(ontology_identity(None, None).unwrap().hash, "none");
        // A wrong expectation is refused.
        let ws = tempfile::tempdir().unwrap();
        let e = stage_ontology(&f, &OntologyMode::Open, ws.path(), &strict.hash).unwrap_err();
        assert!(e.contains("mislabelled"), "{e}");
    }

    #[test]
    fn a_missing_ontology_file_is_an_error() {
        let e = ontology_identity(Some(Path::new("/nonexistent/o.yaml")), None).unwrap_err();
        assert!(e.contains("--ontology"), "{e}");
    }

    #[test]
    fn cassette_preflight_names_both_hashes_in_both_directions() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("c.jsonl");
        let h1 = OntologyIdentity {
            hash: "aaaa".into(),
            mode: Some("strict".into()),
        };
        let none = OntologyIdentity::none();
        // No sidecar: warn with an ontology, silent without.
        assert!(preflight_cassette(&c, &h1)
            .unwrap()
            .unwrap()
            .contains("cannot be verified"));
        assert!(preflight_cassette(&c, &none).unwrap().is_none());
        // Recorded under h1: same passes, different / none fails naming both.
        write_cassette_meta(&c, &h1, "lcg 1", 1).unwrap();
        assert!(cassette_meta_path(&c).ends_with("c.jsonl.meta.json"));
        assert!(preflight_cassette(&c, &h1).unwrap().is_none());
        let e = preflight_cassette(&c, &none).unwrap_err();
        assert!(e.contains("aaaa") && e.contains("none"), "{e}");
        let h2 = OntologyIdentity {
            hash: "bbbb".into(),
            mode: Some("open".into()),
        };
        let e = preflight_cassette(&c, &h2).unwrap_err();
        assert!(e.contains("aaaa") && e.contains("bbbb"), "{e}");
        // Recorded with no ontology: an ontology run fails.
        write_cassette_meta(&c, &none, "lcg 1", 1).unwrap();
        let e = preflight_cassette(&c, &h1).unwrap_err();
        assert!(e.contains("none") && e.contains("aaaa"), "{e}");
    }

    #[test]
    fn sequential_sidecar_bytes_are_unchanged_and_old_sidecars_still_parse() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("c.jsonl");
        let id = OntologyIdentity::none();
        write_cassette_meta(&c, &id, "lcg 1", 1).unwrap();
        let text = std::fs::read_to_string(cassette_meta_path(&c)).unwrap();
        // Exactly the pre-#900 shape: neither new key is written at N = 1.
        assert!(
            !text.contains("ingest_concurrency") && !text.contains("order_dependent"),
            "{text}"
        );
        // A sidecar written before the keys existed reads back with the defaults.
        let old = r#"{"format":1,"ontology_hash":"none","ontology_mode":null,"lcg_build":"x"}"#;
        std::fs::write(cassette_meta_path(&c), old).unwrap();
        let m = read_cassette_meta(&c).unwrap().unwrap();
        assert!(!m.order_dependent && m.ingest_concurrency.is_none());
        assert!(preflight_cassette(&c, &id).unwrap().is_none());
    }

    #[test]
    fn a_concurrent_recording_is_marked_order_dependent_and_replay_warns() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path().join("c.jsonl");
        let id = OntologyIdentity::none();
        write_cassette_meta(&c, &id, "lcg 1", 4).unwrap();
        let m = read_cassette_meta(&c).unwrap().unwrap();
        assert_eq!((m.ingest_concurrency, m.order_dependent), (Some(4), true));
        let w = preflight_cassette(&c, &id).unwrap().unwrap();
        assert!(
            w.contains("order-dependent") && w.contains("--ingest-concurrency 4"),
            "{w}"
        );
        // An ontology mismatch is still an error, not a warning.
        let other = OntologyIdentity {
            hash: "zzzz".into(),
            mode: Some("open".into()),
        };
        assert!(preflight_cassette(&c, &other).is_err());
    }

    #[test]
    fn manifest_round_trips_and_is_optional() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_manifest(d.path()).unwrap().is_none());
        let m = WorkspaceManifest {
            format: 1,
            group_id: "retrieval-eval".into(),
            embedder: "http://e".into(),
            ontology_hash: "h".into(),
            ..Default::default()
        };
        write_manifest(d.path(), &m).unwrap();
        assert_eq!(read_manifest(d.path()).unwrap(), Some(m));
    }

    #[test]
    fn copy_and_persist_skip_the_socket() {
        let src = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join(".lcg/db")).unwrap();
        std::fs::write(src.path().join(".lcg/db/liminis.db"), "db").unwrap();
        std::fs::write(src.path().join(SOCKET_FILE), "").unwrap();
        let out = tempfile::tempdir().unwrap();
        let copy = out.path().join("copy");
        copy_workspace(src.path(), &copy).unwrap();
        assert!(copy.join(".lcg/db/liminis.db").exists());
        assert!(!copy.join(SOCKET_FILE).exists());
        assert!(
            src.path().join(SOCKET_FILE).exists(),
            "the source is only read"
        );
        let kept = out.path().join("kept");
        std::fs::create_dir(&kept).unwrap(); // an empty dir is replaced
        persist_workspace(&copy, &kept).unwrap();
        assert!(kept.join(".lcg/db/liminis.db").exists());
        assert!(!copy.exists());
    }

    #[test]
    fn a_failed_move_names_where_the_workspace_still_is() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("ingested");
        std::fs::create_dir_all(src.join(".lcg/db")).unwrap();
        std::fs::write(src.join(".lcg/db/liminis.db"), b"db").unwrap();
        // Something filled `dest` after the parse-time check, so it cannot be replaced.
        let dest = d.path().join("kept");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("other"), b"x").unwrap();
        let e = persist_workspace(&src, &dest).unwrap_err();
        assert!(e.contains(&src.display().to_string()), "{e}");
        assert!(
            src.join(".lcg/db/liminis.db").exists(),
            "the workspace is left in place"
        );
        assert!(
            dest.join("other").exists(),
            "what was at dest is not touched"
        );
    }

    #[test]
    fn endpoints_are_stored_without_credentials() {
        assert_eq!(
            redact_endpoint("http://user:tok@127.0.0.1:8080/v1/embeddings?key=s3cret#x"),
            "http://127.0.0.1:8080/v1/embeddings"
        );
        assert_eq!(redact_endpoint("https://h/e"), "https://h/e");
        assert_eq!(redact_endpoint(" /tmp/e.sock "), "/tmp/e.sock");
    }
}
