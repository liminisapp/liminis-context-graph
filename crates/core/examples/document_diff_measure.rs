//! SC-009 measurement for document mode (issue #785, ADR-0785): replays this repository's own
//! markdown history as document revisions and reports how much the chosen matching strategy
//! re-extracts, against a heading-path-addressed scheme (the app's `doc::heading_path::index`
//! chunk ids). No LLM, no database — just the chunker and the diff.
//!
//! ```sh
//! cargo run -p lcg-core --example document_diff_measure            # docs/, ideas/, README.md
//! cargo run -p lcg-core --example document_diff_measure -- 200     # at most 200 pairs
//! ```
//!
//! Run from the repository root (it shells out to `git`).

use std::process::Command;

use lcg_core::document::diff::{measure_pair, PairMeasure};
use lcg_core::document::ingest::DOCUMENT_CHUNK_MAX_CHARS_DEFAULT;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn main() {
    let limit: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(usize::MAX);
    let log = git(&[
        "log",
        "--no-merges",
        "--diff-filter=M",
        "--format=@%H",
        "--name-only",
        "--",
        "docs",
        "ideas",
        "README.md",
    ])
    .expect("git log failed — run from the repository root");

    // (commit, path) for every modification of a markdown file.
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut commit = String::new();
    for line in log.lines() {
        if let Some(c) = line.strip_prefix('@') {
            commit = c.to_string();
        } else if line.ends_with(".md") {
            pairs.push((commit.clone(), line.to_string()));
        }
    }

    let mut measured: Vec<PairMeasure> = Vec::new();
    for (c, path) in pairs.iter().take(limit) {
        let (Some(old), Some(new)) = (
            git(&["show", &format!("{c}^:{path}")]),
            git(&["show", &format!("{c}:{path}")]),
        ) else {
            continue;
        };
        if old == new {
            continue;
        }
        measured.push(measure_pair(&old, &new, DOCUMENT_CHUNK_MAX_CHARS_DEFAULT));
    }

    let sum = |f: &dyn Fn(&PairMeasure) -> usize| measured.iter().map(f).sum::<usize>();
    let n = measured.len();
    let new_chunks = sum(&|m| m.new_chunks);
    let extracted = sum(&|m| m.extracted);
    let orphans = sum(&|m| m.heading_path_orphans);
    let add_remove = measured
        .iter()
        .filter(|m| m.added > 0 || m.removed > 0)
        .count();
    let orphaning = measured
        .iter()
        .filter(|m| m.heading_path_orphans > 0)
        .count();
    let renames: Vec<&PairMeasure> = measured.iter().filter(|m| m.heading_change_only).collect();

    println!(
        "document-mode diff measurement (chunker v{}, max {} chars/unit)",
        lcg_core::document::chunker::CHUNKER_VERSION,
        DOCUMENT_CHUNK_MAX_CHARS_DEFAULT
    );
    println!("revision pairs measured:              {n}");
    println!("chunks in new revisions:              {new_chunks}");
    println!(
        "chunks re-extracted (changed+added):  {extracted} ({:.1}%)",
        100.0 * extracted as f64 / new_chunks.max(1) as f64
    );
    println!(
        "chunks skipped as unchanged:          {}",
        new_chunks - extracted
    );
    println!("pairs with an added/removed chunk:    {add_remove}");
    println!(
        "pairs orphaning a heading-path address: {orphaning} ({:.1}%)",
        100.0 * orphaning as f64 / n.max(1) as f64
    );
    println!("heading-path orphans (address-keyed): {orphans}");
    let orph: Vec<&PairMeasure> = measured
        .iter()
        .filter(|m| m.heading_path_orphans > 0)
        .collect();
    println!(
        "  in those pairs: {} chunks re-extracted here, {} removed here; {} orphaned by address",
        orph.iter().map(|m| m.extracted).sum::<usize>(),
        orph.iter().map(|m| m.removed).sum::<usize>(),
        orph.iter().map(|m| m.heading_path_orphans).sum::<usize>()
    );
    println!("heading-change-only pairs:            {}", renames.len());
    if !renames.is_empty() {
        let ours: usize = renames.iter().map(|m| m.extracted).sum();
        let addr: usize = renames.iter().map(|m| m.heading_path_orphans).sum();
        println!("  re-extracted under this strategy:   {ours}");
        println!("  orphaned under address-keyed ids:   {addr}");
    }
}
