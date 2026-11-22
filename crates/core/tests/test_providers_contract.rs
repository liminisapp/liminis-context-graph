// Executes every example in `docs/test-providers.md` (issue #851, FR-020 / SC-007): the document
// is the stable consumer contract, so a parser change that alters documented behaviour must fail
// lcg's own CI. Each ```fact-lines block is paired with the ```expect block that follows it.

use lcg_core::{parse_fact_lines, TestEmbedder, TEST_EMBEDDER_MODEL, TEST_EMBEDDING_DEFAULT_DIM};

const DOC: &str = include_str!("../../../docs/test-providers.md");

/// Returns the bodies of fenced blocks with the given info string, in document order, paired with
/// the index of the line that opened each.
fn blocks(info: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut open: Option<(usize, String)> = None;
    for (n, line) in DOC.lines().enumerate() {
        match (&mut open, line.trim_end()) {
            (None, l) if l == format!("```{info}") => open = Some((n, String::new())),
            (Some(_), "```") => out.push(open.take().unwrap()),
            (Some((_, body)), l) => {
                body.push_str(l);
                body.push('\n');
            }
            _ => {}
        }
    }
    out
}

/// Renders the parser's output in the document's `expect` notation.
fn render(text: &str) -> String {
    let p = parse_fact_lines(text);
    let mut s = String::new();
    for e in &p.entities {
        s.push_str(&format!("entity: {}\n", e.name));
    }
    for e in &p.edges {
        s.push_str(&format!(
            "edge: {} -[{}]-> {} : {}\n",
            e.source, e.relation, e.target, e.fact
        ));
    }
    s
}

#[test]
fn every_documented_fact_line_example_produces_its_documented_output() {
    let inputs = blocks("fact-lines");
    let expects = blocks("expect");
    assert!(
        inputs.len() >= 10,
        "expected the documented examples, found {}",
        inputs.len()
    );
    assert_eq!(
        inputs.len(),
        expects.len(),
        "every `fact-lines` block needs exactly one `expect` block"
    );
    for ((in_line, input), (ex_line, expected)) in inputs.iter().zip(&expects) {
        assert!(
            ex_line > in_line,
            "`expect` block at line {ex_line} must follow its `fact-lines` block at line {in_line}"
        );
        assert_eq!(
            render(input),
            *expected,
            "docs/test-providers.md: example at line {} no longer produces its documented output",
            in_line + 1
        );
    }
}

#[test]
fn documented_embedder_constants_match_the_code() {
    assert!(DOC.contains(&format!("**{TEST_EMBEDDING_DEFAULT_DIM}**")));
    assert!(DOC.contains(&format!("`{TEST_EMBEDDER_MODEL}`")));
}

#[test]
fn documented_embedder_guarantees_hold() {
    let e = TestEmbedder::default();
    let cos = |a: &[f32], b: &[f32]| -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum()
    };
    // Unit length, configured dimension, case-folding, determinism.
    let v = e.embed_sync("Alice works at Acme");
    assert_eq!(v.len(), TEST_EMBEDDING_DEFAULT_DIM);
    let n: f64 = v.iter().map(|x| f64::from(*x).powi(2)).sum();
    assert!((n - 1.0).abs() < 1e-5);
    assert_eq!(e.embed_sync("Alice"), e.embed_sync("alice"));
    assert_eq!(v, e.embed_sync("Alice works at Acme"));
    // Blank text: fixed, non-zero, unit.
    let blank = e.embed_sync("  ...  ");
    assert_eq!(blank, e.embed_sync(""));
    assert!(blank.iter().any(|x| *x != 0.0));
    // Similarity: shared words beat none; Acme/Acmes related; near names under the merge threshold.
    let q = e.embed_sync("Alice Acme");
    assert!(cos(&q, &v) > cos(&q, &e.embed_sync("Bob plays cello")));
    assert!(cos(&e.embed_sync("Acme"), &e.embed_sync("Acmes")) > 0.3);
    assert!(cos(&e.embed_sync("Server A"), &e.embed_sync("Server B")) < 0.85);
}
