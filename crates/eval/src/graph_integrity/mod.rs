//! `lcg-eval graph-integrity` (#819, ADR-0819): a deterministic graph-quality report over an
//! ingested workspace, scored against a fixture skeleton. No LLM, no judge, no network, no
//! extraction provider, and the workspace is never written.
//!
//! Layers: [`source`] (the only disk access: reads a copy of the DB), [`score`] (pure),
//! [`report`] (markdown/JSON/compare). Matching rules live in [`rules`]; the skeleton→graph
//! mapping is data ([`mapping`]), kept next to the fixture.

pub mod cli;
pub mod mapping;
pub mod report;
pub mod rules;
pub mod score;
pub mod skeleton;
pub mod source;

use std::path::Path;

use cli::{parse_args, usage, GraphIntegrityArgs, Parsed, DEFAULT_MAPPING_FILE};
use report::{compare, render_compare_markdown, CompareError, Report, RunMeta};
use score::{score, ScoreInput};

pub const EXIT_OK: i32 = 0;
pub const EXIT_RUNTIME: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_WORKSPACE: i32 = 3;

/// Entry point for `lcg-eval graph-integrity <args…>`; returns the process exit code.
pub fn run_cli(args: &[String]) -> i32 {
    let parsed = match parse_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("lcg-eval graph-integrity: {e}");
            return EXIT_USAGE;
        }
    };
    match parsed {
        Parsed::Help => {
            println!("{}", usage());
            EXIT_OK
        }
        Parsed::Run(a) => match run(&a) {
            Ok(code) => code,
            Err((code, msg)) => {
                eprintln!("lcg-eval graph-integrity: {msg}");
                code
            }
        },
    }
}

fn write_out(dir: &Path, name: &str, text: &str) -> Result<(), (i32, String)> {
    std::fs::write(dir.join(name), text)
        .map_err(|e| (EXIT_RUNTIME, format!("cannot write {name}: {e}")))
}

fn run(a: &GraphIntegrityArgs) -> Result<i32, (i32, String)> {
    let skeleton = skeleton::Skeleton::load(&a.skeleton).map_err(|e| (EXIT_RUNTIME, e))?;
    let mapping_path = a.mapping.clone().unwrap_or_else(|| {
        a.skeleton
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(DEFAULT_MAPPING_FILE)
    });
    let mapping = mapping::Mapping::load(&mapping_path).map_err(|e| (EXIT_RUNTIME, e))?;

    // Validate --compare before the (slower) graph read, so a bad file fails fast.
    let previous = match &a.compare {
        None => None,
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| {
                (
                    EXIT_RUNTIME,
                    format!("cannot read --compare file {}: {e}", p.display()),
                )
            })?;
            Some(Report::from_json_str(&text).map_err(|e| {
                (
                    EXIT_RUNTIME,
                    format!("--compare file {} is not a valid report: {e}", p.display()),
                )
            })?)
        }
    };

    let graph = source::read_graph(
        &a.workspace,
        &a.group,
        a.db_path.as_deref(),
        a.ontology.as_deref(),
    )
    .map_err(|e| (e.exit_code(), e.to_string()))?;
    let mut report = score(ScoreInput {
        skeleton: &skeleton,
        mapping: &mapping,
        nodes: &graph.nodes,
        edges: &graph.edges,
        signatures: graph.signatures.as_deref(),
        group: &a.group,
        ontology_source: graph.ontology_source,
        unstated_breakdown: a.unstated_breakdown,
    });
    report.run_meta = Some(RunMeta {
        workspace: a.workspace.display().to_string(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
    });

    let markdown = report.render_markdown();
    println!("{markdown}");
    // Write the report before comparing: an incompatible `--compare` must not discard the
    // freshly computed (expensive) report.
    if let Some(dir) = &a.out {
        std::fs::create_dir_all(dir).map_err(|e| {
            (
                EXIT_RUNTIME,
                format!("cannot create --out {}: {e}", dir.display()),
            )
        })?;
        write_out(dir, "report.md", &markdown)?;
        write_out(
            dir,
            "report.json",
            &serde_json::to_string_pretty(&report.to_json()).expect("report serializes"),
        )?;
        eprintln!(
            "lcg-eval graph-integrity: wrote report to {}",
            dir.display()
        );
    }
    if let Some(prev) = &previous {
        let rows = compare(prev, &report).map_err(|e| match e {
            CompareError::Incompatible(m) => (
                EXIT_USAGE,
                format!(
                    "cannot compare with {}: {m}",
                    a.compare
                        .as_deref()
                        .map_or_else(String::new, |p| p.display().to_string())
                ),
            ),
        })?;
        let md = render_compare_markdown(&rows);
        println!("{md}");
        if let Some(dir) = &a.out {
            write_out(dir, "compare.md", &md)?;
        }
    }
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use report::*;
    use std::collections::BTreeMap;

    fn base_report(dir: u32) -> Report {
        // direction_correct is the only number that moves between the two reports (SC-005 shape)
        let lc = |d: usize| LinkCounts {
            present: Ratio::new(72, 72),
            typed: Ratio::new(71, 72),
            direction_correct: Ratio::new(d, 71),
        };
        Report {
            schema_version: SCHEMA_VERSION,
            inputs: Inputs {
                skeleton_sha256: "s".into(),
                mapping_sha256: "m".into(),
                group: "g".into(),
                ontology_source: "explicit".into(),
                nodes: 10,
                edges: 10,
            },
            typing: Typing {
                entities: 48,
                found: Ratio::new(48, 48),
                split: 0,
                typed: Ratio::new(47, 48),
                absent: vec![],
                unmapped_kinds: vec![],
            },
            links: Links {
                total: lc(dir as usize),
                by_attribute: BTreeMap::new(),
                unmapped_attributes: vec![],
            },
            value_facts: ValueFacts {
                stated: Ratio::new(290, 428),
                misattached: Ratio::new(29, 428),
            },
            junk: Junk {
                nodes: 13,
                names: vec![],
            },
            signature: Signature {
                mark_key: "signature_mismatch".into(),
                marks_seen: false,
                marked_total: 0,
                marked_by_type: BTreeMap::new(),
                fit: None,
            },
            unstated_breakdown: None,
            run_meta: None,
        }
    }

    #[test]
    fn compare_shows_only_the_direction_improvement() {
        let (before, after) = (base_report(56), base_report(71));
        let rows = compare(&before, &after).unwrap();
        for r in &rows {
            if r.name == "links.total.direction_correct" {
                assert_eq!(r.before, Some((56, Some(71))));
                assert_eq!(r.after, Some((71, Some(71))));
                assert_eq!(r.delta, Some(15));
                assert_eq!(r.verdict, "improved");
            } else {
                assert_eq!(r.delta, Some(0), "{}", r.name);
                assert_eq!(r.verdict, "unchanged");
            }
        }
        let md = render_compare_markdown(&rows);
        assert!(md.contains("| links.total.direction_correct | 56/71 | 71/71 | +15 | improved |"));
        // direction of "better" for lower-is-better metrics
        let mut worse = base_report(71);
        worse.junk.nodes = 20;
        let r = compare(&base_report(71), &worse).unwrap();
        let junk = r.iter().find(|r| r.name == "junk.nodes").unwrap();
        assert_eq!((junk.delta, junk.verdict), (Some(7), "worse"));
    }

    #[test]
    fn compare_judges_ratios_by_rate_not_numerator() {
        // free-form 35/35 (100%) vs ontology 56/71 (79%): numerator grew, rate fell.
        let mut before = base_report(71);
        before.links.total.direction_correct = Ratio::new(35, 35);
        let mut after = base_report(71);
        after.links.total.direction_correct = Ratio::new(56, 71);
        let rows = compare(&before, &after).unwrap();
        let r = rows
            .iter()
            .find(|r| r.name == "links.total.direction_correct")
            .unwrap();
        assert_eq!(r.delta, Some(21));
        assert_eq!(r.verdict, "worse rate (denominators differ)");
        // same numerator change with a denominator that grew in step: rate unchanged
        let mut before = base_report(71);
        before.links.total.direction_correct = Ratio::new(1, 2);
        let mut after = base_report(71);
        after.links.total.direction_correct = Ratio::new(2, 4);
        let rows = compare(&before, &after).unwrap();
        let r = rows
            .iter()
            .find(|r| r.name == "links.total.direction_correct")
            .unwrap();
        assert_eq!(r.verdict, "unchanged rate (denominators differ)");
    }

    #[test]
    fn compare_refuses_different_skeleton_or_mapping() {
        let mut other = base_report(71);
        other.inputs.skeleton_sha256 = "x".into();
        assert!(matches!(
            compare(&base_report(71), &other),
            Err(CompareError::Incompatible(_))
        ));
        let mut other = base_report(71);
        other.inputs.mapping_sha256 = "x".into();
        assert!(matches!(
            compare(&base_report(71), &other),
            Err(CompareError::Incompatible(_))
        ));
    }

    #[test]
    fn json_roundtrips_and_run_meta_is_segregated() {
        let mut r = base_report(71);
        let plain = serde_json::to_string(&r).unwrap();
        assert!(!plain.contains("run_meta"));
        r.run_meta = Some(RunMeta {
            workspace: "/w".into(),
            tool_version: "0".into(),
            generated_at: "t".into(),
        });
        let with = serde_json::to_string(&r).unwrap();
        let back = Report::from_json_str(&with).unwrap();
        assert_eq!(back, r);
        // compare ignores run_meta
        assert!(compare(&r, &base_report(71)).is_ok());
        assert!(Report::from_json_str("{}").is_err());
        assert!(Report::from_json_str(
            &with.replace("\"schema_version\":1", "\"schema_version\":9")
        )
        .is_err());
    }

    #[test]
    fn markdown_and_json_carry_the_same_numbers() {
        let r = base_report(56);
        let md = r.render_markdown();
        let j = r.to_json();
        assert_eq!(j["links"]["total"]["direction_correct"]["num"], 56);
        assert!(md.contains("56/71"));
        assert!(md.contains("290/428"));
        assert!(md.contains("29/428"));
        assert!(md.contains("marks are absent"));
        assert!(md.contains("not applicable"));
        assert!(md.contains("upper bound on true misses"));
    }

    #[test]
    fn exit_codes_for_bad_inputs() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(run_cli(&s(&["--bogus"])), EXIT_USAGE);
        assert_eq!(run_cli(&s(&["--help"])), EXIT_OK);
        // missing skeleton file → runtime error
        assert_eq!(
            run_cli(&s(&[
                "--workspace",
                "/nonexistent-ws",
                "--skeleton",
                "/nonexistent/skeleton.json"
            ])),
            EXIT_RUNTIME
        );
    }

    /// The relation types the v2 ontology (v3.1) declares for the five skeleton link attributes.
    /// A mapping that lists only free-form names for an attribute scores that attribute 0/N and
    /// hides its direction defects (it did for Account manager and Exception approver), so every
    /// attribute must name at least one of these. Keep in step with the ontology the v2 graphs are
    /// built with.
    #[test]
    fn shipped_v2_mapping_covers_the_ontology_relation_types() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval-v2");
        let m = mapping::Mapping::load(&dir.join("graph-mapping.json")).unwrap();
        use mapping::Direction::{ObjectToSubject, SubjectToObject};
        // (attribute, ontology relation type, direction relative to the fact's subject)
        let ontology_relations = [
            ("Owner", "OWNS", ObjectToSubject),
            ("Supplier", "SUPPLIED_BY", SubjectToObject),
            ("Account manager", "CONTACT_FOR", ObjectToSubject),
            ("Reports to", "REPORTS_TO", SubjectToObject),
            ("Exception approver", "APPROVES", ObjectToSubject),
        ];
        for (attr, rel, dir) in ontology_relations {
            let got = m.links[attr]
                .relation_types
                .iter()
                .find(|r| rules::normalize(&r.name) == rules::normalize(rel))
                .unwrap_or_else(|| panic!("{attr}: ontology relation {rel} is not in the mapping"));
            assert_eq!(got.direction, dir, "{attr}: direction of {rel}");
        }
    }

    #[test]
    fn shipped_v2_mapping_validates_against_shipped_skeleton() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/retrieval-v2");
        let sk = skeleton::Skeleton::load(&dir.join("skeleton.json")).unwrap();
        let m = mapping::Mapping::load(&dir.join("graph-mapping.json")).unwrap();
        for e in &sk.entities {
            assert!(m.kinds.contains_key(&e.kind), "kind {} unmapped", e.kind);
        }
        for attr in [
            "Owner",
            "Supplier",
            "Account manager",
            "Reports to",
            "Exception approver",
        ] {
            assert!(m.links.contains_key(attr), "link {attr} unmapped");
        }
        // every link-shaped fact in the skeleton is covered by the mapping
        let names: std::collections::HashSet<_> =
            sk.entities.iter().map(|e| e.name.as_str()).collect();
        for f in &sk.facts {
            if names.contains(f.value.as_str()) {
                assert!(
                    m.links.contains_key(&f.attribute),
                    "{} unmapped",
                    f.attribute
                );
            }
        }
        // the unscored graph of nothing: scoring an empty graph yields the skeleton's own denominators
        let r = score(ScoreInput {
            skeleton: &sk,
            mapping: &m,
            nodes: &[],
            edges: &[],
            signatures: None,
            group: "g",
            ontology_source: "none",
            unstated_breakdown: false,
        });
        assert_eq!(r.typing.entities, 48);
        assert_eq!(r.typing.typed.den, 48);
        assert_eq!(r.links.total.present.den, 60);
        assert_eq!(r.value_facts.stated.den, 440);
        assert!(r.links.unmapped_attributes.is_empty());
    }
}
