//! Report structs, markdown rendering and `--compare` for `lcg-eval graph-integrity` (#819).
//!
//! The JSON and the markdown are both rendered from the same [`Report`], so they carry the same
//! numbers (FR-011). Run-time metadata lives in the separate, optional `run_meta` block, which
//! `--compare` ignores (FR-014).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Ratio {
    pub num: usize,
    pub den: usize,
}

impl Ratio {
    pub fn new(num: usize, den: usize) -> Self {
        Self { num, den }
    }
}

impl std::fmt::Display for Ratio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Inputs {
    pub skeleton_sha256: String,
    pub mapping_sha256: String,
    pub group: String,
    /// `none`, `explicit`, `group` or `workspace` — where the ontology came from (informational;
    /// not part of the compare compatibility check).
    pub ontology_source: String,
    pub nodes: usize,
    pub edges: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Typing {
    pub entities: usize,
    pub found: Ratio,
    /// Skeleton entities matched by two or more nodes (count, not a ratio).
    pub split: usize,
    pub typed: Ratio,
    /// Skeleton entities with no matching node.
    pub absent: Vec<String>,
    /// Skeleton kinds with no mapping entry (excluded from the typed denominator).
    pub unmapped_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LinkCounts {
    pub present: Ratio,
    pub typed: Ratio,
    /// Denominator is the number of links with the expected relation type.
    pub direction_correct: Ratio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Links {
    pub total: LinkCounts,
    pub by_attribute: BTreeMap<String, LinkCounts>,
    /// Link-shaped skeleton attributes (value is a skeleton entity) with no mapping entry.
    pub unmapped_attributes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValueFacts {
    pub stated: Ratio,
    pub misattached: Ratio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Junk {
    pub nodes: usize,
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SigCounts {
    pub fit: usize,
    pub reversed: usize,
    pub neither: usize,
    /// Edges of a type whose ontology entry declares no endpoint types.
    pub unconstrained: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FitSection {
    pub by_type: BTreeMap<String, SigCounts>,
    /// Edge types present in the graph but not declared by the ontology, with edge counts.
    pub undeclared_types: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Signature {
    /// Key read from each edge's `attributes` JSON.
    pub mark_key: String,
    /// True when at least one edge carries the mark key. False means marks are absent from this
    /// graph (not that the graph is clean).
    pub marks_seen: bool,
    pub marked_total: usize,
    pub marked_by_type: BTreeMap<String, usize>,
    /// `None` (JSON `null`) = not applicable: no ontology, or one with no signatures.
    pub fit: Option<FitSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunMeta {
    pub workspace: String,
    pub tool_version: String,
    pub generated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Report {
    pub schema_version: u32,
    pub inputs: Inputs,
    pub typing: Typing,
    pub links: Links,
    pub value_facts: ValueFacts,
    pub junk: Junk,
    pub signature: Signature,
    /// Excluded from `--compare` and from the determinism guarantee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_meta: Option<RunMeta>,
}

// ── metric list (shared by compare) ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metric {
    pub name: String,
    pub num: usize,
    pub den: Option<usize>,
    pub better: Better,
}

fn ratio(name: impl Into<String>, r: Ratio, better: Better) -> Metric {
    Metric {
        name: name.into(),
        num: r.num,
        den: Some(r.den),
        better,
    }
}

fn count(name: impl Into<String>, n: usize, better: Better) -> Metric {
    Metric {
        name: name.into(),
        num: n,
        den: None,
        better,
    }
}

impl Report {
    /// Every metric, in a stable order, as `(name, num, den, better)`.
    pub fn metrics(&self) -> Vec<Metric> {
        use Better::{Higher, Lower};
        let mut m = vec![
            ratio("typing.found", self.typing.found, Higher),
            count("typing.split", self.typing.split, Lower),
            ratio("typing.typed", self.typing.typed, Higher),
        ];
        let mut link = |prefix: &str, c: &LinkCounts| {
            m.push(ratio(format!("{prefix}.present"), c.present, Higher));
            m.push(ratio(format!("{prefix}.typed"), c.typed, Higher));
            m.push(ratio(
                format!("{prefix}.direction_correct"),
                c.direction_correct,
                Higher,
            ));
        };
        link("links.total", &self.links.total);
        for (a, c) in &self.links.by_attribute {
            link(&format!("links.{a}"), c);
        }
        m.push(ratio("value_facts.stated", self.value_facts.stated, Higher));
        m.push(ratio(
            "value_facts.misattached",
            self.value_facts.misattached,
            Lower,
        ));
        m.push(count("junk.nodes", self.junk.nodes, Lower));
        m.push(count(
            "signature.marked_total",
            self.signature.marked_total,
            Lower,
        ));
        for (t, n) in &self.signature.marked_by_type {
            m.push(count(format!("signature.marked.{t}"), *n, Lower));
        }
        if let Some(f) = &self.signature.fit {
            for (t, c) in &f.by_type {
                m.push(count(format!("signature.fit.{t}.fit"), c.fit, Higher));
                m.push(count(
                    format!("signature.fit.{t}.reversed"),
                    c.reversed,
                    Lower,
                ));
                m.push(count(
                    format!("signature.fit.{t}.neither"),
                    c.neither,
                    Lower,
                ));
            }
        }
        m
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("report serializes")
    }

    pub fn from_json_str(text: &str) -> Result<Self, String> {
        let r: Report = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if r.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "unsupported report schema_version {} (expected {SCHEMA_VERSION})",
                r.schema_version
            ));
        }
        Ok(r)
    }

    pub fn render_markdown(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Graph integrity report\n");
        let i = &self.inputs;
        let _ = writeln!(s, "- Group: `{}`", i.group);
        let _ = writeln!(s, "- Graph: {} nodes, {} edges", i.nodes, i.edges);
        let _ = writeln!(s, "- Ontology: {}", i.ontology_source);
        let _ = writeln!(
            s,
            "- Skeleton sha256: `{}`",
            &i.skeleton_sha256[..16.min(i.skeleton_sha256.len())]
        );
        let _ = writeln!(
            s,
            "- Mapping sha256: `{}`",
            &i.mapping_sha256[..16.min(i.mapping_sha256.len())]
        );
        if let Some(m) = &self.run_meta {
            let _ = writeln!(
                s,
                "- Workspace: `{}`; lcg-eval {}; {}",
                m.workspace, m.tool_version, m.generated_at
            );
        }
        let t = &self.typing;
        let _ = writeln!(s, "\n## Typing\n");
        let _ = writeln!(s, "| Metric | Value |\n|---|---|");
        let _ = writeln!(s, "| Skeleton entities found | {} |", t.found);
        let _ = writeln!(s, "| Split across several nodes | {} |", t.split);
        let _ = writeln!(s, "| Typed as expected | {} |", t.typed);
        if !t.absent.is_empty() {
            let _ = writeln!(s, "\nNot found: {}", t.absent.join(", "));
        }
        if !t.unmapped_kinds.is_empty() {
            let _ = writeln!(
                s,
                "\nUnmapped kinds (excluded from typed): {}",
                t.unmapped_kinds.join(", ")
            );
        }
        let _ = writeln!(s, "\n## Links\n");
        let _ = writeln!(
            s,
            "| Attribute | Present (any edge) | Expected relation type | Direction correct (of typed) |\n|---|---|---|---|"
        );
        let mut row = |name: &str, c: &LinkCounts| {
            let _ = writeln!(
                s,
                "| {name} | {} | {} | {} |",
                c.present, c.typed, c.direction_correct
            );
        };
        for (a, c) in &self.links.by_attribute {
            row(a, c);
        }
        row("**total**", &self.links.total);
        if !self.links.unmapped_attributes.is_empty() {
            let _ = writeln!(
                s,
                "\nUnmapped link attributes (excluded): {}",
                self.links.unmapped_attributes.join(", ")
            );
        }
        let v = &self.value_facts;
        let _ = writeln!(s, "\n## Value facts\n");
        let _ = writeln!(s, "| Metric | Value |\n|---|---|");
        let _ = writeln!(
            s,
            "| Stated on an edge of the right entity | {} |",
            v.stated
        );
        let _ = writeln!(
            s,
            "| Misattached to another skeleton entity | {} |",
            v.misattached
        );
        let _ = writeln!(
            s,
            "\n\"Stated\" is an exact-string match, so restated values count as not stated: the not-stated count is an upper bound on true misses and the stated count a lower bound."
        );
        let _ = writeln!(s, "\n## Junk\n");
        let _ = writeln!(s, "Nodes named like note IDs: **{}**", self.junk.nodes);
        if !self.junk.names.is_empty() {
            let _ = writeln!(
                s,
                "\n{}",
                self.junk
                    .names
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let g = &self.signature;
        let _ = writeln!(s, "\n## Signature\n");
        if g.marks_seen {
            let _ = writeln!(
                s,
                "`{}` marks stored on {} edge(s):\n",
                g.mark_key, g.marked_total
            );
            for (t, n) in &g.marked_by_type {
                let _ = writeln!(s, "- {t}: {n}");
            }
        } else {
            let _ = writeln!(
                s,
                "No `{}` marks found on any edge: marks are absent from this graph (or this build predates them). Zero does not mean clean.",
                g.mark_key
            );
        }
        match &g.fit {
            None => {
                let _ = writeln!(
                    s,
                    "\nFit / reversed / neither: not applicable (no ontology signatures)."
                );
            }
            Some(f) => {
                let _ = writeln!(s, "\n| Edge type | Fit | Reversed | Neither | Unconstrained |\n|---|---|---|---|---|");
                for (t, c) in &f.by_type {
                    let _ = writeln!(
                        s,
                        "| {t} | {} | {} | {} | {} |",
                        c.fit, c.reversed, c.neither, c.unconstrained
                    );
                }
                if !f.undeclared_types.is_empty() {
                    let _ = writeln!(s, "\nEdge types not declared by the ontology:");
                    for (t, n) in &f.undeclared_types {
                        let _ = writeln!(s, "- {t}: {n}");
                    }
                }
            }
        }
        s
    }
}

// ── compare ──────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaRow {
    pub name: String,
    /// Value in the earlier (`--compare`) report.
    pub before: Option<(usize, Option<usize>)>,
    /// Value in this report.
    pub after: Option<(usize, Option<usize>)>,
    pub delta: Option<i64>,
    /// `improved`, `worse`, `unchanged`, or `n/a` when a side is missing.
    pub verdict: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompareError {
    /// Different skeleton or mapping (exit 2).
    Incompatible(String),
}

impl std::fmt::Display for CompareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompareError::Incompatible(m) => write!(f, "{m}"),
        }
    }
}

/// Per-metric values from both reports and the signed delta (`after - before`).
pub fn compare(before: &Report, after: &Report) -> Result<Vec<DeltaRow>, CompareError> {
    if before.inputs.skeleton_sha256 != after.inputs.skeleton_sha256 {
        return Err(CompareError::Incompatible(
            "reports were produced against different skeletons; deltas would be meaningless"
                .to_string(),
        ));
    }
    if before.inputs.mapping_sha256 != after.inputs.mapping_sha256 {
        return Err(CompareError::Incompatible(
            "reports were produced with different mappings; deltas would be meaningless"
                .to_string(),
        ));
    }
    let b: BTreeMap<String, Metric> = before
        .metrics()
        .into_iter()
        .map(|m| (m.name.clone(), m))
        .collect();
    let a: BTreeMap<String, Metric> = after
        .metrics()
        .into_iter()
        .map(|m| (m.name.clone(), m))
        .collect();
    // Keep this report's metric order first, then metrics only in the earlier one.
    let mut names: Vec<String> = after.metrics().into_iter().map(|m| m.name).collect();
    for n in b.keys() {
        if !a.contains_key(n) {
            names.push(n.clone());
        }
    }
    Ok(names
        .into_iter()
        .map(|name| {
            let bm = b.get(&name);
            let am = a.get(&name);
            let (delta, verdict) = match (bm, am) {
                (Some(x), Some(y)) => {
                    let d = y.num as i64 - x.num as i64;
                    // A ratio metric is judged by its rate, not its numerator: the denominator can
                    // differ between graphs (e.g. the number of typed links), so 35/35 -> 56/71 is
                    // a fall in rate even though the numerator grew.
                    let ord = match (x.den, y.den) {
                        (Some(xd), Some(yd)) if xd > 0 && yd > 0 => {
                            (y.num as u128 * xd as u128).cmp(&(x.num as u128 * yd as u128))
                        }
                        _ => y.num.cmp(&x.num),
                    };
                    let den_differs = x.den != y.den;
                    (Some(d), verdict_for(ord, y.better, den_differs))
                }
                _ => (None, "n/a"),
            };
            DeltaRow {
                name,
                before: bm.map(|m| (m.num, m.den)),
                after: am.map(|m| (m.num, m.den)),
                delta,
                verdict,
            }
        })
        .collect())
}

fn verdict_for(ord: std::cmp::Ordering, better: Better, den_differs: bool) -> &'static str {
    use std::cmp::Ordering::{Equal, Greater, Less};
    match (ord, better, den_differs) {
        (Equal, _, false) => "unchanged",
        (Equal, _, true) => "unchanged rate (denominators differ)",
        (Greater, Better::Higher, false) | (Less, Better::Lower, false) => "improved",
        (Greater, Better::Higher, true) | (Less, Better::Lower, true) => {
            "improved rate (denominators differ)"
        }
        (_, _, false) => "worse",
        (_, _, true) => "worse rate (denominators differ)",
    }
}

fn fmt_val(v: &Option<(usize, Option<usize>)>) -> String {
    match v {
        None => "—".to_string(),
        Some((n, Some(d))) => format!("{n}/{d}"),
        Some((n, None)) => n.to_string(),
    }
}

pub fn render_compare_markdown(rows: &[DeltaRow]) -> String {
    let mut s = String::from("# Graph integrity comparison\n\n");
    let _ = writeln!(
        s,
        "| Metric | Before | After | Delta | |\n|---|---|---|---|---|"
    );
    for r in rows {
        let d = r.delta.map_or("—".to_string(), |d| format!("{d:+}"));
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} |",
            r.name,
            fmt_val(&r.before),
            fmt_val(&r.after),
            d,
            r.verdict
        );
    }
    s
}
