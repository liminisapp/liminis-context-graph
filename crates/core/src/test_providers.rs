//! Built-in deterministic, offline providers for consumers' tests (issue #851, ADR-0851).
//!
//! Selected only by the explicit `--embedder test` / `--extractor test` service flags — never
//! from the environment, a config file or a fallback. The contract (fact-line grammar and
//! embedder guarantees) is published in `docs/test-providers.md`, and every example in that
//! document is executed by `crates/core/tests/test_providers_contract.rs`.
//!
//! * [`TestEmbedder`] — a signed, hashed bag of words plus character trigrams, L2-normalised.
//!   Texts that share words are close; unrelated texts are not. Accumulation is exact integer
//!   arithmetic and the hash is explicit FNV-1a, so vectors are bit-identical on every platform.
//! * [`TestExtractor`] — derives entities and edges solely from *fact lines* in the episode body
//!   (`SRC -> DST: fact` or `SRC -[REL]-> DST: fact`), through [`parse_fact_lines`]. It
//!   implements [`Extractor`] directly and never touches a prompt.

use futures::future::BoxFuture;

use crate::{
    dedup_judge::{DedupVerdict, DuplicatePair},
    embedder::{EmbedRole, Embedder},
    error::Error,
    extractor::{ExtractOptions, Extractor},
    ontology::normalize_relation_type,
    types::{ExtractedEdge, ExtractedEntity, ExtractionOutcome, ExtractionResult},
};

/// Model identity reported by the test embedder. Distinct from any real model so a database
/// shared with a real embedder reports `embedding_model_status: "mismatch"`.
pub const TEST_EMBEDDER_MODEL: &str = "lcg-test-bow";

/// Embedding dimension used when none is configured. Frozen: the dimension is fixed per
/// database, so changing this would orphan existing test databases.
pub const TEST_EMBEDDING_DEFAULT_DIM: usize = 768;

/// Entity type given to every extracted entity (the engine's "no specific type" value, which
/// passes through strict ontology mode unchanged).
pub const TEST_ENTITY_TYPE: &str = "Entity";

/// Relation type of a plain `SRC -> DST: fact` line.
pub const TEST_DEFAULT_RELATION: &str = "RELATED_TO";

/// Weight of a whole-word feature; character trigrams weigh 1.
const WORD_WEIGHT: i64 = 2;

/// Sentinel token used for blank / token-less text. The tokeniser can never produce it (it
/// contains a NUL, which is not alphanumeric), so it cannot collide with a real word.
const BLANK_TOKEN: &str = "\0blank";

// ── TestEmbedder ──────────────────────────────────────────────────────────────

/// Deterministic, offline, similarity-preserving embedder. See the module docs.
#[derive(Debug, Clone)]
pub struct TestEmbedder {
    dim: usize,
}

impl TestEmbedder {
    /// Creates an embedder of dimension `dim` (clamped to at least 8).
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(8) }
    }

    /// Embeds `text` synchronously. Same result as [`Embedder::embed`].
    pub fn embed_sync(&self, text: &str) -> Vec<f32> {
        let mut acc = vec![0i64; self.dim];
        let tokens = tokenize(text);
        if tokens.is_empty() {
            self.add_feature(&mut acc, "w:", BLANK_TOKEN, WORD_WEIGHT);
        }
        for token in &tokens {
            self.add_feature(&mut acc, "w:", token, WORD_WEIGHT);
            let padded: Vec<char> = std::iter::once('^')
                .chain(token.chars())
                .chain(std::iter::once('$'))
                .collect();
            for tri in padded.windows(3) {
                let s: String = tri.iter().collect();
                self.add_feature(&mut acc, "t:", &s, 1);
            }
        }
        normalise(&acc)
    }

    fn add_feature(&self, acc: &mut [i64], tag: &str, body: &str, weight: i64) {
        let mut h = FNV_OFFSET;
        for b in tag.bytes().chain(body.bytes()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(FNV_PRIME);
        }
        let bucket = (h % self.dim as u64) as usize;
        if (h >> 63) & 1 == 1 {
            acc[bucket] -= weight;
        } else {
            acc[bucket] += weight;
        }
    }
}

impl Default for TestEmbedder {
    fn default() -> Self {
        Self::new(TEST_EMBEDDING_DEFAULT_DIM)
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Splits on non-alphanumeric characters and folds ASCII case only (so Rust's Unicode table
/// version can never change a vector). Distinct tokens only, first-appearance order.
fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        let tok = raw.to_ascii_lowercase();
        if !out.contains(&tok) {
            out.push(tok);
        }
    }
    out
}

/// L2-normalises exact integer accumulators. Sum of squares is exact in `f64` (well below
/// 2^53); `sqrt` and division are correctly rounded IEEE operations, so the result is
/// bit-identical across platforms.
fn normalise(acc: &[i64]) -> Vec<f32> {
    let sumsq: i64 = acc.iter().map(|v| v * v).sum();
    if sumsq == 0 {
        // Every feature cancelled (astronomically unlikely). Fall back to a fixed unit vector.
        let mut v = vec![0.0f32; acc.len()];
        v[0] = 1.0;
        return v;
    }
    let norm = (sumsq as f64).sqrt();
    acc.iter().map(|&v| (v as f64 / norm) as f32).collect()
}

impl Embedder for TestEmbedder {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        _role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, Error>> {
        Box::pin(async move { Ok(self.embed_sync(text)) })
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn is_test_provider(&self) -> bool {
        true
    }
}

// ── Fact-line grammar ─────────────────────────────────────────────────────────

/// One recognised fact line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactLine {
    pub source: String,
    /// Normalised relation type (`RELATED_TO` for a plain `->` line).
    pub relation: String,
    pub target: String,
    pub fact: String,
}

/// An entity named by at least one fact line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactEntity {
    pub name: String,
    /// Fact text of the first line that mentions the name.
    pub summary: String,
}

/// The parse of a whole text: entities in first-appearance order, edges in line order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedFacts {
    pub entities: Vec<FactEntity>,
    pub edges: Vec<FactLine>,
}

/// Parses fact lines out of `text`. See `docs/test-providers.md` for the grammar.
pub fn parse_fact_lines(text: &str) -> ParsedFacts {
    let mut out = ParsedFacts::default();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let Some(fl) = parse_line(line) else { continue };
        if out.edges.contains(&fl) {
            continue;
        }
        for name in [&fl.source, &fl.target] {
            if !out.entities.iter().any(|e| &e.name == name) {
                out.entities.push(FactEntity {
                    name: name.clone(),
                    summary: fl.fact.clone(),
                });
            }
        }
        out.edges.push(fl);
    }
    out
}

/// Strips leading whitespace and any run of Markdown prefixes (`>`, `-`, `*`, `+`, `1.`, `1)`),
/// each of which must be followed by whitespace.
fn strip_prefixes(line: &str) -> &str {
    let mut s = line.trim_start();
    loop {
        let before = s;
        if let Some(c) = s.chars().next() {
            if matches!(c, '>' | '-' | '*' | '+') {
                let rest = &s[c.len_utf8()..];
                if c == '>' || rest.starts_with(char::is_whitespace) {
                    s = rest.trim_start();
                }
            }
        }
        if s == before {
            let digits = s.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 {
                let rest = &s[digits..];
                if let Some(r) = rest.strip_prefix(['.', ')']) {
                    if r.starts_with(char::is_whitespace) {
                        s = r.trim_start();
                    }
                }
            }
        }
        if s == before {
            return s;
        }
    }
}

fn parse_line(line: &str) -> Option<FactLine> {
    let s = strip_prefixes(line);
    let arrow = s.find("->");
    let named = s.find("-[");
    let (source, relation, rest) = match (named, arrow) {
        (Some(n), a) if a.is_none_or(|a| n < a) => {
            let after = &s[n + 2..];
            let close = after.find(']')?;
            let rel = normalize_relation_type(after[..close].trim());
            let rest = after[close + 1..].strip_prefix("->")?;
            (&s[..n], rel, rest)
        }
        (_, Some(a)) => (&s[..a], TEST_DEFAULT_RELATION.to_string(), &s[a + 2..]),
        _ => return None,
    };
    let colon = rest.find(':')?;
    let source = source.trim();
    let target = rest[..colon].trim();
    let fact = rest[colon + 1..].trim();
    if source.is_empty() || target.is_empty() || fact.is_empty() || relation.is_empty() {
        return None;
    }
    Some(FactLine {
        source: source.to_string(),
        relation,
        target: target.to_string(),
        fact: fact.to_string(),
    })
}

// ── TestExtractor ─────────────────────────────────────────────────────────────

/// Deterministic, offline extractor driven solely by fact lines in the episode body.
#[derive(Debug, Clone, Copy, Default)]
pub struct TestExtractor;

fn dedup_key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

impl Extractor for TestExtractor {
    fn extract<'a>(
        &'a self,
        opts: ExtractOptions<'a>,
    ) -> BoxFuture<'a, Result<ExtractionOutcome, Error>> {
        let parsed = parse_fact_lines(opts.episode_body);
        Box::pin(async move {
            Ok(ExtractionResult {
                entities: parsed
                    .entities
                    .into_iter()
                    .map(|e| ExtractedEntity {
                        name: e.name,
                        entity_type: TEST_ENTITY_TYPE.to_string(),
                        summary: e.summary,
                        original_entity_type: None,
                    })
                    .collect(),
                edges: parsed
                    .edges
                    .into_iter()
                    .map(|f| ExtractedEdge {
                        source_name: f.source,
                        target_name: f.target,
                        fact: f.fact,
                        relation_type: Some(f.relation),
                        valid_at: None,
                        invalid_at: None,
                        original_relation_type: None,
                    })
                    .collect(),
            }
            .into())
        })
    }

    fn classify_entities<'a>(
        &'a self,
        entities: &'a [(&'a str, &'a str)],
        _allowed_types: Option<&'a [String]>,
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = entities.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }

    fn classify_relations<'a>(
        &'a self,
        edges: &'a [(&'a str, &'a str)],
        _allowed_types: &'a [(String, Option<String>)],
    ) -> BoxFuture<'a, Result<Vec<String>, Error>> {
        let n = edges.len();
        Box::pin(async move { Ok(vec![String::new(); n]) })
    }

    fn consolidate_summary<'a>(
        &'a self,
        _entity_name: &'a str,
        existing: &'a str,
        incoming: &'a str,
    ) -> BoxFuture<'a, Result<String, Error>> {
        let (existing, incoming) = (existing.trim(), incoming.trim());
        let merged = if incoming.is_empty() || existing.contains(incoming) {
            existing.to_string()
        } else if existing.is_empty() {
            incoming.to_string()
        } else {
            format!("{existing} {incoming}")
        };
        Box::pin(async move { Ok(merged) })
    }

    fn judge_duplicates<'a>(
        &'a self,
        pairs: &'a [DuplicatePair],
    ) -> BoxFuture<'a, Result<Vec<DedupVerdict>, Error>> {
        let verdicts = pairs
            .iter()
            .map(|p| {
                if dedup_key(&p.existing.name) == dedup_key(&p.incoming.name) {
                    DedupVerdict::Duplicate
                } else {
                    DedupVerdict::Distinct
                }
            })
            .collect();
        Box::pin(async move { Ok(verdicts) })
    }

    fn is_test_provider(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cos(a: &[f32], b: &[f32]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum()
    }

    fn emb(t: &str) -> Vec<f32> {
        TestEmbedder::default().embed_sync(t)
    }

    #[test]
    fn unit_length_and_dimension() {
        for t in ["Alice works at Acme", "x", "日本語 テキスト", "", "   "] {
            let v = emb(t);
            assert_eq!(v.len(), 768);
            let n: f64 = v.iter().map(|x| f64::from(*x).powi(2)).sum();
            assert!((n - 1.0).abs() < 1e-5, "norm² {n} for {t:?}");
        }
        assert_eq!(TestEmbedder::new(64).embed_sync("hello").len(), 64);
        assert_eq!(TestEmbedder::new(64).dim(), 64);
    }

    #[test]
    fn deterministic_and_case_folded() {
        assert_eq!(emb("Alice Smith"), emb("Alice Smith"));
        assert_eq!(emb("Alice Smith"), emb("alice, SMITH!"));
        assert_ne!(emb("Alice"), emb("Bob"));
    }

    #[test]
    fn golden_vector_is_pinned() {
        // Pinned so any change to tokenisation, hashing or normalisation fails loudly: the
        // vector is a published contract (docs/test-providers.md).
        let v = TestEmbedder::new(32).embed_sync("alice");
        let nonzero: Vec<(usize, f32)> = v
            .iter()
            .enumerate()
            .filter(|(_, x)| **x != 0.0)
            .map(|(i, x)| (i, *x))
            .collect();
        let n: f64 = v.iter().map(|x| f64::from(*x).powi(2)).sum();
        assert!((n - 1.0).abs() < 1e-6);
        assert_eq!(GOLDEN_ALICE_32.len(), nonzero.len());
        for ((i, x), (gi, gx)) in nonzero.iter().zip(GOLDEN_ALICE_32) {
            assert_eq!(i, gi);
            assert_eq!(x.to_bits(), gx.to_bits());
        }
    }

    const GOLDEN_ALICE_32: &[(usize, f32)] = &[
        (2, -0.33333334),
        (9, 0.33333334),
        (20, -0.6666667),
        (21, 0.33333334),
        (29, -0.33333334),
        (30, 0.33333334),
    ];

    #[test]
    fn blank_text_is_unit_nonzero_and_stable() {
        let a = emb("");
        assert_eq!(a, emb("   \n\t"));
        assert_eq!(a, emb("!!! ???"));
        assert!(a.iter().any(|x| *x != 0.0));
        assert_ne!(a, emb("blank"));
    }

    #[test]
    fn overlap_is_closer_than_unrelated() {
        let q = emb("where does Alice work");
        let hit = emb("Alice works at Acme");
        let miss = emb("Bob plays the cello");
        assert!(cos(&q, &hit) > cos(&q, &miss));
        assert!(cos(&q, &hit) > 0.3);
        assert!(cos(&emb("Acme"), &emb("Acmes")) > 0.3);
    }

    #[test]
    fn overlapping_query_and_fact_clear_the_search_floor() {
        for (q, f) in [
            ("Alice works at Acme", "Alice works at Acme"),
            ("Alice Acme", "Alice works at Acme"),
            ("Wiki hosted ServerA", "Wiki runs on ServerA hosted"),
        ] {
            let c = cos(&emb(q), &emb(f));
            assert!(c > 0.5, "{q:?} vs {f:?} = {c}");
        }
    }

    #[test]
    fn near_names_stay_below_merge_threshold() {
        for (a, b) in [
            ("Server A", "Server B"),
            ("Alice Smith", "Alice Jones"),
            ("Project Alpha", "Project Beta"),
            ("Acme", "Apex"),
        ] {
            let c = cos(&emb(a), &emb(b));
            assert!(c < 0.85, "{a:?} vs {b:?} = {c}");
        }
    }

    #[test]
    fn unrelated_single_word_names_never_approach_merge_threshold() {
        // Deterministic pseudo-words with no shared stems.
        let consonants = [
            'b', 'd', 'f', 'g', 'k', 'l', 'm', 'n', 'p', 'r', 's', 't', 'v', 'z',
        ];
        let vowels = ['a', 'e', 'i', 'o', 'u'];
        let mut names = Vec::new();
        let mut state = 12345u64;
        while names.len() < 200 {
            let mut w = String::new();
            for _ in 0..3 {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                w.push(consonants[((state >> 33) as usize) % consonants.len()]);
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                w.push(vowels[((state >> 33) as usize) % vowels.len()]);
            }
            if !names.contains(&w) {
                names.push(w);
            }
        }
        let vecs: Vec<Vec<f32>> = names.iter().map(|n| emb(n)).collect();
        let mut max = 0.0f64;
        for i in 0..vecs.len() {
            for j in i + 1..vecs.len() {
                max = max.max(cos(&vecs[i], &vecs[j]));
            }
        }
        assert!(max < 0.6, "max pairwise cosine {max}");
    }

    fn facts(text: &str) -> Vec<(String, String, String, String)> {
        parse_fact_lines(text)
            .edges
            .into_iter()
            .map(|e| (e.source, e.relation, e.target, e.fact))
            .collect()
    }

    fn one(s: &str, r: &str, d: &str, f: &str) -> (String, String, String, String) {
        (s.into(), r.into(), d.into(), f.into())
    }

    #[test]
    fn plain_and_named_lines() {
        assert_eq!(
            facts("Alice -> Acme: Alice works at Acme"),
            vec![one("Alice", "RELATED_TO", "Acme", "Alice works at Acme")]
        );
        assert_eq!(
            facts("Wiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA"),
            vec![one("Wiki", "HOSTED_ON", "ServerA", "Wiki runs on ServerA")]
        );
    }

    #[test]
    fn no_fact_lines_extract_nothing() {
        let p = parse_fact_lines("# Heading\n\nSome prose: with a colon.\n- a bullet\n");
        assert!(p.entities.is_empty() && p.edges.is_empty());
        assert!(parse_fact_lines("").edges.is_empty());
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            facts("   A   ->   B   :   some fact   "),
            vec![one("A", "RELATED_TO", "B", "some fact")]
        );
        assert_eq!(
            facts("A -[ hosted on ]-> B : f"),
            vec![one("A", "HOSTED_ON", "B", "f")]
        );
    }

    #[test]
    fn malformed_lines_are_ignored() {
        for l in [
            "-> B: x",
            "A -> : x",
            "A -> B:",
            "A -> B",
            "A -> B:   ",
            "A -[]-> B: x",
            "A -[---]-> B: x",
            "A -[REL B: x",
            "A -[REL]- B: x",
            " : x",
        ] {
            assert!(facts(l).is_empty(), "{l:?} should be ignored");
        }
    }

    #[test]
    fn first_colon_after_arrow_splits_fact() {
        assert_eq!(
            facts("A -> B: ratio 3:1 via x -> y"),
            vec![one("A", "RELATED_TO", "B", "ratio 3:1 via x -> y")]
        );
    }

    #[test]
    fn crlf_and_markdown_prefixes() {
        let text = "A -> B: one\r\n  C -> D: two\r\n- E -> F: three\n> G -> H: four\n\
                    * I -> J: five\n1. K -> L: six\n2) M -> N: seven\n> - O -> P: eight\n";
        let got = facts(text);
        let facts_only: Vec<&str> = got.iter().map(|t| t.3.as_str()).collect();
        assert_eq!(
            facts_only,
            ["one", "two", "three", "four", "five", "six", "seven", "eight"]
        );
        assert_eq!(got[0].2, "B");
    }

    #[test]
    fn fenced_code_block_lines_are_scanned() {
        assert_eq!(
            facts("```\nA -> B: f\n```"),
            vec![one("A", "RELATED_TO", "B", "f")]
        );
    }

    #[test]
    fn repeated_lines_deduplicated_and_ordered() {
        let p = parse_fact_lines("A -> B: x\nB -> C: y\nA -> B: x\nA -> B: other\nC -> A: z");
        assert_eq!(p.edges.len(), 4);
        let names: Vec<&str> = p.entities.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
        assert_eq!(p.entities[0].summary, "x");
        assert_eq!(p.entities[2].summary, "y");
    }

    #[test]
    fn self_edges_are_emitted_by_the_parser() {
        let p = parse_fact_lines("A -> A: self");
        assert_eq!(p.edges.len(), 1);
        assert_eq!(p.entities.len(), 1);
    }

    #[test]
    fn names_are_case_sensitive_and_unicode_safe() {
        let p = parse_fact_lines("alice -> Zoë: x\nAlice -> Zoë: x");
        assert_eq!(p.entities.len(), 3);
        assert_eq!(p.edges.len(), 2);
        assert_eq!(p.edges[0].target, "Zoë");
    }

    #[test]
    fn many_lines_are_processed() {
        let text: String = (0..2000)
            .map(|i| format!("N{i} -> M{i}: fact {i}\n"))
            .collect();
        let p = parse_fact_lines(&text);
        assert_eq!(p.edges.len(), 2000);
        assert_eq!(p.entities.len(), 4000);
    }

    fn opts(body: &str) -> ExtractOptions<'_> {
        ExtractOptions {
            episode_body: body,
            group_id: "g",
            source_type: crate::types::SourceType::Text,
            custom_instructions: None,
            reference_time: "2026-01-01T00:00:00Z",
            ontology: None,
            chunk_key: None,
            document_context: None,
        }
    }

    #[tokio::test]
    async fn extractor_maps_facts_to_entities_and_edges() {
        let out = TestExtractor
            .extract(opts(
                "prose\nAlice -> Acme: Alice works at Acme\nWiki -[HOSTED_ON]-> S: on S",
            ))
            .await
            .unwrap();
        let r = out.result;
        assert_eq!(r.entities.len(), 4);
        assert!(r.entities.iter().all(|e| e.entity_type == "Entity"));
        assert_eq!(r.entities[0].summary, "Alice works at Acme");
        assert_eq!(r.edges.len(), 2);
        assert_eq!(r.edges[0].relation_type.as_deref(), Some("RELATED_TO"));
        assert_eq!(r.edges[1].relation_type.as_deref(), Some("HOSTED_ON"));
        let none = TestExtractor.extract(opts("nothing here")).await.unwrap();
        assert!(none.result.entities.is_empty() && none.result.edges.is_empty());
    }

    #[tokio::test]
    async fn extractor_helpers_are_deterministic() {
        let ex = TestExtractor;
        assert!(ex.is_configured() && ex.is_test_provider());
        assert_eq!(
            ex.classify_entities(&[("a", "b"), ("c", "d")], None)
                .await
                .unwrap(),
            vec![String::new(); 2]
        );
        assert_eq!(
            ex.classify_relations(&[("f", "t")], &[("X".into(), None)])
                .await
                .unwrap(),
            vec![String::new()]
        );
        assert_eq!(ex.consolidate_summary("n", "a", "").await.unwrap(), "a");
        assert_eq!(ex.consolidate_summary("n", "", "b").await.unwrap(), "b");
        assert_eq!(
            ex.consolidate_summary("n", "a b", "b").await.unwrap(),
            "a b"
        );
        assert_eq!(ex.consolidate_summary("n", "a", "b").await.unwrap(), "a b");
        let side = |n: &str| crate::dedup_judge::DedupSide {
            name: n.to_string(),
            entity_type: String::new(),
            summary: String::new(),
        };
        let pairs = [
            DuplicatePair {
                existing: side("Alice"),
                incoming: side(" alice "),
            },
            DuplicatePair {
                existing: side("Alice"),
                incoming: side("Alicia"),
            },
        ];
        assert_eq!(
            ex.judge_duplicates(&pairs).await.unwrap(),
            vec![DedupVerdict::Duplicate, DedupVerdict::Distinct]
        );
    }
    #[test]
    fn embedder_wrappers_delegate_test_mode() {
        use crate::embedder::{CountingEmbedder, EffectiveInstructions, InstructedEmbedder};
        use std::sync::Arc;
        let t: Arc<dyn Embedder> = Arc::new(TestEmbedder::default());
        assert!(t.is_test_provider());
        assert!(CountingEmbedder::new(t.clone()).is_test_provider());
        assert!(InstructedEmbedder::new(t, EffectiveInstructions::none()).is_test_provider());
        let real: Arc<dyn Embedder> = Arc::new(crate::embedder::MockEmbedder::new(8));
        assert!(!real.is_test_provider());
        assert!(!CountingEmbedder::new(real.clone()).is_test_provider());
        assert!(!InstructedEmbedder::new(real, EffectiveInstructions::none()).is_test_provider());
    }

    #[test]
    fn extractor_wrappers_delegate_test_mode() {
        use crate::cassette::{CassetteWriter, RecordingExtractor};
        use crate::llm_router::LlmRouter;
        use crate::telemetry::NoopSink;
        use std::sync::Arc;
        let dir = tempfile::TempDir::new().unwrap();
        let writer = Arc::new(CassetteWriter::open(dir.path().join("c.jsonl")).unwrap());
        let t: Arc<dyn Extractor> = Arc::new(TestExtractor);
        let rec = RecordingExtractor::new(t.clone(), "p", "m", writer.clone());
        assert!(rec.is_test_provider());
        let router = LlmRouter::new(t, "m".into(), None, String::new(), Arc::new(NoopSink));
        assert!(router.is_test_provider());
        let real: Arc<dyn Extractor> = Arc::new(crate::extractor::MockExtractor);
        assert!(!RecordingExtractor::new(real.clone(), "p", "m", writer).is_test_provider());
        let router = LlmRouter::new(real, "m".into(), None, String::new(), Arc::new(NoopSink));
        assert!(!router.is_test_provider());
    }
}
