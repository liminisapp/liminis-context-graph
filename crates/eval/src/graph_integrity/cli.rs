//! Argument parsing for `lcg-eval graph-integrity …`. Hand-rolled to match `retrieval/cli.rs`.

use std::path::PathBuf;

pub const DEFAULT_GROUP: &str = "retrieval-eval";
pub const DEFAULT_SKELETON: &str = "crates/eval/fixtures/retrieval-v2/skeleton.json";
pub const DEFAULT_MAPPING_FILE: &str = "graph-mapping.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphIntegrityArgs {
    pub workspace: PathBuf,
    pub group: String,
    pub skeleton: PathBuf,
    /// `None` = `graph-mapping.json` next to the skeleton.
    pub mapping: Option<PathBuf>,
    pub ontology: Option<PathBuf>,
    pub db_path: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub compare: Option<PathBuf>,
    /// Classify the not-stated value facts by cause (#823). Off by default.
    pub unstated_breakdown: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Help,
    Run(Box<GraphIntegrityArgs>),
}

pub fn usage() -> String {
    format!(
        "lcg-eval graph-integrity — score an ingested workspace's graph against a fixture skeleton

USAGE:
    lcg-eval graph-integrity --workspace <DIR> [OPTIONS]

OPTIONS:
    --workspace <DIR>    An ingested workspace (required). Never modified: the DB is read from a copy.
    --group <ID>         Group to read. Default: {DEFAULT_GROUP}
    --skeleton <FILE>    Skeleton JSON. Default: {DEFAULT_SKELETON}
    --mapping <FILE>     Skeleton→graph mapping. Default: {DEFAULT_MAPPING_FILE} next to the skeleton
    --ontology <FILE>    Ontology the graph was built with. Default: the workspace's own
    --db-path <PATH>     DB location relative to the workspace. Default: .lcg/db/liminis.db (then .graphiti/db)
    --out <DIR>          Also write report.md and report.json (and compare.md with --compare) here
    --compare <FILE>     An earlier report.json; print per-metric deltas against it
    --unstated-breakdown Also classify the value facts not stated on any edge of their entity by
                         cause (in the entity's summary / on another entity / in another surface
                         form / absent). Adds a report section and an `unstated_breakdown` JSON key
    -h, --help           Print this help

No network, extraction provider or LLM is used.

EXIT CODES:
    0 ok · 1 runtime error or invalid --compare file · 2 usage error or --compare against a
    different skeleton/mapping · 3 workspace missing, not ingested, unreadable, or group empty"
    )
}

pub fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut workspace = None;
    let mut a = GraphIntegrityArgs {
        workspace: PathBuf::new(),
        group: DEFAULT_GROUP.to_string(),
        skeleton: PathBuf::from(DEFAULT_SKELETON),
        mapping: None,
        ontology: None,
        db_path: None,
        out: None,
        compare: None,
        unstated_breakdown: false,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--workspace" => workspace = Some(PathBuf::from(val("--workspace")?)),
            "--group" => a.group = val("--group")?,
            "--skeleton" => a.skeleton = PathBuf::from(val("--skeleton")?),
            "--mapping" => a.mapping = Some(PathBuf::from(val("--mapping")?)),
            "--ontology" => a.ontology = Some(PathBuf::from(val("--ontology")?)),
            "--db-path" => a.db_path = Some(PathBuf::from(val("--db-path")?)),
            "--out" => a.out = Some(PathBuf::from(val("--out")?)),
            "--compare" => a.compare = Some(PathBuf::from(val("--compare")?)),
            "--unstated-breakdown" => a.unstated_breakdown = true,
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }
    a.workspace = workspace.ok_or("--workspace is required (see --help)")?;
    if a.group.trim().is_empty() {
        return Err("--group must not be empty".to_string());
    }
    Ok(Parsed::Run(Box::new(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Parsed, String> {
        parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn defaults() {
        let Parsed::Run(a) = p(&["--workspace", "/w"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.workspace, PathBuf::from("/w"));
        assert_eq!(a.group, "retrieval-eval");
        assert_eq!(a.skeleton, PathBuf::from(DEFAULT_SKELETON));
        assert!(
            a.mapping.is_none() && a.ontology.is_none() && a.out.is_none() && a.compare.is_none()
        );
    }

    #[test]
    fn all_flags() {
        let Parsed::Run(a) = p(&[
            "--workspace",
            "w",
            "--group",
            "g",
            "--skeleton",
            "s.json",
            "--mapping",
            "m.json",
            "--ontology",
            "o.yaml",
            "--db-path",
            "d",
            "--out",
            "out",
            "--compare",
            "c.json",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(a.group, "g");
        assert_eq!(a.mapping, Some(PathBuf::from("m.json")));
        assert_eq!(a.compare, Some(PathBuf::from("c.json")));
        assert_eq!(a.db_path, Some(PathBuf::from("d")));
        assert!(!a.unstated_breakdown, "off by default");
        let Parsed::Run(a) = p(&["--workspace", "w", "--unstated-breakdown"]).unwrap() else {
            panic!()
        };
        assert!(a.unstated_breakdown);
    }

    #[test]
    fn errors_and_help() {
        assert_eq!(p(&["--help"]).unwrap(), Parsed::Help);
        assert!(p(&[]).unwrap_err().contains("--workspace"));
        assert!(p(&["--workspace"])
            .unwrap_err()
            .contains("requires a value"));
        assert!(p(&["--workspace", "w", "--bogus"])
            .unwrap_err()
            .contains("unknown"));
        assert!(p(&["--workspace", "w", "--group", " "]).is_err());
    }
}
