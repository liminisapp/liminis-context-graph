//! Retrieval-eval fixture loading and validation (FR-008..FR-012, FR-019).
//!
//! Layout of a fixture directory:
//!
//! ```text
//! manifest.json        corpus_version, chunking, notes (ingest order), supersedes
//! notes/<id>.md        one markdown note per manifest entry
//! questions.jsonl      one JSON question per line
//! questions.meta.json  {"questions_version": "..."}   (optional; "unversioned" if absent)
//! ```
//!
//! Every validation error names the offending question id (FR-019).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// The four question categories (FR-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Direct,
    Paraphrase,
    Temporal,
    Answer,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Direct,
        Category::Paraphrase,
        Category::Temporal,
        Category::Answer,
    ];

    pub fn parse(s: &str) -> Option<Category> {
        match s {
            "direct" => Some(Category::Direct),
            "paraphrase" => Some(Category::Paraphrase),
            "temporal" => Some(Category::Temporal),
            "answer" => Some(Category::Answer),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Direct => "direct",
            Category::Paraphrase => "paraphrase",
            Category::Temporal => "temporal",
            Category::Answer => "answer",
        }
    }
}

/// One graded-relevant note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relevant {
    pub note: String,
    /// 1..=3. nDCG uses `2^gain - 1`.
    pub gain: u32,
}

/// A fact a grounded answer needs, attributed to a note (judged by an LLM against returned text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceSpec {
    pub note: String,
    pub key_fact: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub category: Category,
    pub text: String,
    pub relevant: Vec<Relevant>,
    pub evidence: Vec<EvidenceSpec>,
    pub current: Vec<String>,
    pub stale: Vec<String>,
    /// A question with no relevant note: scored on false-positive rate instead of recall/MRR/nDCG.
    pub negative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEntry {
    pub id: String,
    /// Optional RFC 3339 event time used as the episode `reference_time` at ingest.
    pub reference_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supersession {
    pub old_note: String,
    pub new_note: String,
    pub effective_at: String,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub corpus_version: String,
    pub max_chunk_tokens: usize,
    pub notes: Vec<NoteEntry>,
    pub supersedes: Vec<Supersession>,
}

#[derive(Debug, Clone)]
pub struct Fixture {
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub questions_version: String,
    pub questions: Vec<Question>,
    /// note id -> markdown body.
    pub notes: BTreeMap<String, String>,
    pub corpus_hash: String,
    pub questions_hash: String,
}

impl Fixture {
    /// Directory holding `<note id>.md` files (what qmd is pointed at).
    pub fn notes_dir(&self) -> PathBuf {
        self.dir.join("notes")
    }
}

pub const DEFAULT_MAX_CHUNK_TOKENS: usize = 900;

#[derive(Deserialize)]
struct RawManifest {
    corpus_version: String,
    #[serde(default)]
    chunking: Option<RawChunking>,
    notes: Vec<RawNote>,
    #[serde(default)]
    supersedes: Vec<RawSupersession>,
}
#[derive(Deserialize)]
struct RawChunking {
    max_tokens: Option<usize>,
}
#[derive(Deserialize)]
struct RawNote {
    id: String,
    #[serde(default)]
    reference_time: Option<String>,
}
#[derive(Deserialize)]
struct RawSupersession {
    old_note: String,
    new_note: String,
    effective_at: String,
}

#[derive(Deserialize, Default)]
struct RawQuestion {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    relevant_notes: Vec<RawRelevant>,
    #[serde(default)]
    evidence: Vec<RawEvidence>,
    #[serde(default)]
    current: Vec<String>,
    #[serde(default)]
    stale: Vec<String>,
    #[serde(default)]
    negative: bool,
}
#[derive(Deserialize)]
struct RawRelevant {
    note: String,
    #[serde(default = "default_gain")]
    gain: u32,
}
fn default_gain() -> u32 {
    1
}
#[derive(Deserialize)]
struct RawEvidence {
    note: String,
    key_fact: String,
}

/// Note ids double as qmd file stems, so they are restricted to a slug alphabet that survives
/// qmd's path normalisation unchanged.
pub fn valid_note_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

fn sha_hex(parts: &[&[u8]]) -> String {
    let mut buf = Vec::new();
    for p in parts {
        buf.extend_from_slice(&(p.len() as u64).to_le_bytes());
        buf.extend_from_slice(p);
    }
    lcg_core::sha256_hex::sha256_hex(&buf)[..16].to_string()
}

/// Loads `dir` and validates everything. `Err` messages name the offending question or note.
pub fn load(dir: &Path) -> Result<Fixture, String> {
    let manifest_path = dir.join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let manifest =
        parse_manifest(&manifest_bytes).map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    let mut notes = BTreeMap::new();
    let mut hash_parts: Vec<Vec<u8>> = vec![manifest_bytes.clone()];
    for n in &manifest.notes {
        let p = dir.join("notes").join(format!("{}.md", n.id));
        let body = std::fs::read_to_string(&p)
            .map_err(|e| format!("manifest note '{}': cannot read {}: {e}", n.id, p.display()))?;
        hash_parts.push(body.as_bytes().to_vec());
        notes.insert(n.id.clone(), body);
    }
    let corpus_hash = sha_hex(&hash_parts.iter().map(|v| v.as_slice()).collect::<Vec<_>>());

    let qpath = dir.join("questions.jsonl");
    let qtext = std::fs::read_to_string(&qpath)
        .map_err(|e| format!("cannot read {}: {e}", qpath.display()))?;
    let known: HashSet<&str> = manifest.notes.iter().map(|n| n.id.as_str()).collect();
    let questions = parse_questions(&qtext, &known)?;
    let questions_hash = sha_hex(&[qtext.as_bytes()]);

    let questions_version = match std::fs::read_to_string(dir.join("questions.meta.json")) {
        Ok(s) => serde_json::from_str::<Value>(&s)
            .ok()
            .and_then(|v| v["questions_version"].as_str().map(str::to_string))
            .unwrap_or_else(|| "unversioned".to_string()),
        Err(_) => "unversioned".to_string(),
    };

    Ok(Fixture {
        dir: dir.to_path_buf(),
        manifest,
        questions_version,
        questions,
        notes,
        corpus_hash,
        questions_hash,
    })
}

pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let raw: RawManifest = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if raw.corpus_version.trim().is_empty() {
        return Err("corpus_version must be non-empty".into());
    }
    let mut seen = BTreeSet::new();
    for n in &raw.notes {
        if !valid_note_id(&n.id) {
            return Err(format!(
                "note id '{}' is invalid (lowercase letters, digits and '-' only)",
                n.id
            ));
        }
        if !seen.insert(n.id.clone()) {
            return Err(format!("duplicate note id '{}'", n.id));
        }
        if let Some(t) = &n.reference_time {
            chrono::DateTime::parse_from_rfc3339(t)
                .map_err(|_| format!("note '{}': reference_time is not RFC 3339: {t}", n.id))?;
        }
    }
    for s in &raw.supersedes {
        for (label, id) in [("old_note", &s.old_note), ("new_note", &s.new_note)] {
            if !seen.contains(id) {
                return Err(format!("supersedes {label} '{id}' is not a manifest note"));
            }
        }
        chrono::DateTime::parse_from_rfc3339(&s.effective_at).map_err(|_| {
            format!(
                "supersedes {}→{}: effective_at is not RFC 3339: {}",
                s.old_note, s.new_note, s.effective_at
            )
        })?;
    }
    Ok(Manifest {
        corpus_version: raw.corpus_version,
        max_chunk_tokens: raw
            .chunking
            .and_then(|c| c.max_tokens)
            .unwrap_or(DEFAULT_MAX_CHUNK_TOKENS),
        notes: raw
            .notes
            .into_iter()
            .map(|n| NoteEntry {
                id: n.id,
                reference_time: n.reference_time,
            })
            .collect(),
        supersedes: raw
            .supersedes
            .into_iter()
            .map(|s| Supersession {
                old_note: s.old_note,
                new_note: s.new_note,
                effective_at: s.effective_at,
            })
            .collect(),
    })
}

/// Parses and validates `questions.jsonl` text against the set of known note ids.
pub fn parse_questions(text: &str, known_notes: &HashSet<&str>) -> Result<Vec<Question>, String> {
    let mut out: Vec<Question> = Vec::new();
    let mut ids = HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let lineno = i + 1;
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("questions.jsonl line {lineno}: malformed JSON: {e}"))?;
        let id = v["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("questions.jsonl line {lineno}: question has no \"id\""))?
            .to_string();
        let q = build_question(&id, v, known_notes).map_err(|e| format!("question '{id}': {e}"))?;
        if !ids.insert(id.clone()) {
            return Err(format!("question '{id}': duplicate question id"));
        }
        out.push(q);
    }
    if out.is_empty() {
        return Err("questions.jsonl contains no questions".into());
    }
    Ok(out)
}

fn build_question(id: &str, v: Value, known: &HashSet<&str>) -> Result<Question, String> {
    let raw: RawQuestion =
        serde_json::from_value(v).map_err(|e| format!("malformed entry: {e}"))?;
    let cat_s = raw.category.ok_or("missing \"category\"")?;
    let category = Category::parse(&cat_s).ok_or_else(|| {
        format!("unknown category '{cat_s}' (expected direct|paraphrase|temporal|answer)")
    })?;
    let text = raw
        .text
        .filter(|t| !t.trim().is_empty())
        .ok_or("missing or empty \"text\"")?;
    let check = |note: &str, what: &str| -> Result<(), String> {
        if known.contains(note) {
            Ok(())
        } else {
            Err(format!(
                "{what} references note '{note}' which is not in the corpus"
            ))
        }
    };

    let mut relevant = Vec::new();
    let mut seen = HashSet::new();
    for r in &raw.relevant_notes {
        check(&r.note, "relevant_notes")?;
        if !(1..=3).contains(&r.gain) {
            return Err(format!(
                "relevant note '{}' has gain {} (expected 1..=3)",
                r.note, r.gain
            ));
        }
        if !seen.insert(r.note.clone()) {
            return Err(format!("relevant note '{}' listed twice", r.note));
        }
        relevant.push(Relevant {
            note: r.note.clone(),
            gain: r.gain,
        });
    }
    if raw.negative {
        if !relevant.is_empty() {
            return Err("negative question must have no relevant_notes".into());
        }
    } else if relevant.is_empty() {
        return Err(
            "relevant_notes is empty (set \"negative\": true for a no-answer question)".into(),
        );
    }

    let mut evidence = Vec::new();
    for e in &raw.evidence {
        check(&e.note, "evidence")?;
        if e.key_fact.trim().is_empty() {
            return Err(format!(
                "evidence for note '{}' has an empty key_fact",
                e.note
            ));
        }
        if !seen.contains(&e.note) {
            return Err(format!(
                "evidence note '{}' is not listed in relevant_notes",
                e.note
            ));
        }
        evidence.push(EvidenceSpec {
            note: e.note.clone(),
            key_fact: e.key_fact.clone(),
        });
    }
    for n in raw.current.iter().chain(raw.stale.iter()) {
        check(n, "current/stale")?;
    }

    match category {
        Category::Answer if evidence.is_empty() => {
            return Err("answer question needs a non-empty \"evidence\" list".into());
        }
        Category::Temporal => {
            if raw.current.is_empty() {
                return Err("temporal question needs a non-empty \"current\" list".into());
            }
            if raw.stale.is_empty() {
                return Err("temporal question needs a non-empty \"stale\" list".into());
            }
            for c in &raw.current {
                if !seen.contains(c) {
                    return Err(format!(
                        "current note '{c}' is not listed in relevant_notes"
                    ));
                }
            }
            for s in &raw.stale {
                if seen.contains(s) {
                    return Err(format!("stale note '{s}' must not be in relevant_notes"));
                }
                if raw.current.contains(s) {
                    return Err(format!("note '{s}' is both current and stale"));
                }
            }
        }
        _ => {}
    }
    if category != Category::Temporal && (!raw.current.is_empty() || !raw.stale.is_empty()) {
        return Err("current/stale are only valid on temporal questions".into());
    }
    if category != Category::Answer && !evidence.is_empty() {
        return Err("evidence is only valid on answer questions".into());
    }

    Ok(Question {
        id: id.to_string(),
        category,
        text,
        relevant,
        evidence,
        current: raw.current,
        stale: raw.stale,
        negative: raw.negative,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> HashSet<&'static str> {
        ["a", "b", "c"].into_iter().collect()
    }

    fn err_of(line: &str) -> String {
        parse_questions(line, &known()).unwrap_err()
    }

    #[test]
    fn valid_questions_of_each_category_load() {
        let t = r#"{"id":"q1","category":"direct","text":"t","relevant_notes":[{"note":"a"}]}
{"id":"q2","category":"paraphrase","text":"t","relevant_notes":[{"note":"a","gain":2},{"note":"b"}]}
{"id":"q3","category":"temporal","text":"t","relevant_notes":[{"note":"b"}],"current":["b"],"stale":["a"]}
{"id":"q4","category":"answer","text":"t","relevant_notes":[{"note":"c"}],"evidence":[{"note":"c","key_fact":"f"}]}
{"id":"q5","category":"direct","text":"t","negative":true}"#;
        let qs = parse_questions(t, &known()).unwrap();
        assert_eq!(qs.len(), 5);
        assert!(qs[4].negative);
        assert_eq!(qs[1].relevant[0].gain, 2);
    }

    #[test]
    fn unknown_category_names_the_question() {
        let e =
            err_of(r#"{"id":"q9","category":"fuzzy","text":"t","relevant_notes":[{"note":"a"}]}"#);
        assert!(e.contains("q9") && e.contains("fuzzy"), "{e}");
    }

    #[test]
    fn missing_note_reference_names_question_and_note() {
        let e = err_of(
            r#"{"id":"q7","category":"direct","text":"t","relevant_notes":[{"note":"zzz"}]}"#,
        );
        assert!(e.contains("q7") && e.contains("zzz"), "{e}");
    }

    #[test]
    fn duplicate_ids_rejected() {
        let l = r#"{"id":"q1","category":"direct","text":"t","relevant_notes":[{"note":"a"}]}"#;
        let e = err_of(&format!("{l}\n{l}"));
        assert!(e.contains("q1") && e.contains("duplicate"), "{e}");
    }

    #[test]
    fn temporal_requires_current_and_stale() {
        let e = err_of(
            r#"{"id":"t1","category":"temporal","text":"t","relevant_notes":[{"note":"a"}],"current":["a"]}"#,
        );
        assert!(e.contains("t1") && e.contains("stale"), "{e}");
        let e = err_of(
            r#"{"id":"t2","category":"temporal","text":"t","relevant_notes":[{"note":"a"}],"stale":["b"]}"#,
        );
        assert!(e.contains("t2") && e.contains("current"), "{e}");
    }

    #[test]
    fn stale_must_not_be_relevant() {
        let e = err_of(
            r#"{"id":"t3","category":"temporal","text":"t","relevant_notes":[{"note":"a"},{"note":"b"}],"current":["a"],"stale":["b"]}"#,
        );
        assert!(e.contains("t3") && e.contains("stale"), "{e}");
    }

    #[test]
    fn answer_requires_evidence() {
        let e =
            err_of(r#"{"id":"a1","category":"answer","text":"t","relevant_notes":[{"note":"a"}]}"#);
        assert!(e.contains("a1") && e.contains("evidence"), "{e}");
    }

    #[test]
    fn empty_relevant_requires_negative_flag() {
        let e = err_of(r#"{"id":"n1","category":"direct","text":"t","relevant_notes":[]}"#);
        assert!(e.contains("n1") && e.contains("negative"), "{e}");
    }

    #[test]
    fn malformed_json_names_the_line() {
        let e = err_of("{not json");
        assert!(e.contains("line 1"), "{e}");
    }

    #[test]
    fn bad_gain_rejected() {
        let e = err_of(
            r#"{"id":"g1","category":"direct","text":"t","relevant_notes":[{"note":"a","gain":9}]}"#,
        );
        assert!(e.contains("g1") && e.contains("gain"), "{e}");
    }

    #[test]
    fn manifest_validation() {
        let ok = br#"{"corpus_version":"v1","notes":[{"id":"a"},{"id":"b"}],"supersedes":[{"old_note":"a","new_note":"b","effective_at":"2026-01-01T00:00:00Z"}]}"#;
        let m = parse_manifest(ok).unwrap();
        assert_eq!(m.max_chunk_tokens, DEFAULT_MAX_CHUNK_TOKENS);
        assert_eq!(m.supersedes.len(), 1);
        let bad = br#"{"corpus_version":"v1","notes":[{"id":"Bad_ID"}]}"#;
        assert!(parse_manifest(bad).unwrap_err().contains("Bad_ID"));
        let bad = br#"{"corpus_version":"v1","notes":[{"id":"a"}],"supersedes":[{"old_note":"a","new_note":"x","effective_at":"2026-01-01T00:00:00Z"}]}"#;
        assert!(parse_manifest(bad).unwrap_err().contains("'x'"));
    }

    #[test]
    fn load_reads_a_directory_and_hashes_are_stable() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("notes")).unwrap();
        std::fs::write(
            d.path().join("manifest.json"),
            r#"{"corpus_version":"v1","notes":[{"id":"a"}]}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("notes/a.md"), "# A\nhello").unwrap();
        std::fs::write(
            d.path().join("questions.jsonl"),
            r#"{"id":"q1","category":"direct","text":"hi","relevant_notes":[{"note":"a"}]}"#,
        )
        .unwrap();
        let f1 = load(d.path()).unwrap();
        let f2 = load(d.path()).unwrap();
        assert_eq!(f1.corpus_hash, f2.corpus_hash);
        assert_eq!(f1.questions_version, "unversioned");
        std::fs::write(d.path().join("notes/a.md"), "# A\nchanged").unwrap();
        assert_ne!(load(d.path()).unwrap().corpus_hash, f1.corpus_hash);
    }

    #[test]
    fn load_fails_on_missing_note_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("manifest.json"),
            r#"{"corpus_version":"v1","notes":[{"id":"a"}]}"#,
        )
        .unwrap();
        assert!(load(d.path()).unwrap_err().contains("'a'"));
    }
}
