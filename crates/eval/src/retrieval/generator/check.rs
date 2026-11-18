//! Checks of rendered prose against the skeleton (#788 scope change, ADR-0788).
//!
//! The checks do not regenerate anything. They verify that the checked-in notes and questions
//! still carry what the checked-in skeleton says they must:
//!
//! * every skeleton fact appears in its note (entity name and value, verbatim, any case);
//! * every evidence key fact is findable in its note, and a passage fact sits in a later chunk of a
//!   multi-chunk note;
//! * a note names no entity other than its own (so negatives stay unanswerable and near-misses stay
//!   near-misses), and an implicit-contradiction note does not mention the earlier values;
//! * derived and answer pages carry their `[[citations]]`;
//! * question wording keeps the required names (or, for paraphrase, none), shares at most
//!   [`PARAPHRASE_MAX_FACT_OVERLAP`] of its content words with the sentence carrying the fact and
//!   at most [`PARAPHRASE_MAX_NOTE_OVERLAP`] with the whole note (FR-012, on the rendered text);
//! * a negative's fake entity appears in no note, and a real entity never shares a note with the
//!   asked attribute's marker words.
//!
//! Errors fail the check; warnings (size outside the target band) are reported but do not.

use std::collections::BTreeMap;
use std::path::Path;

use super::questions::{
    content_words, overlap, PARAPHRASE_MAX_FACT_OVERLAP, PARAPHRASE_MAX_NOTE_OVERLAP,
};
use super::skeleton::*;
use crate::retrieval::mapping::chunk_markdown;

/// Rendered prose, keyed by id.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub notes: BTreeMap<String, String>,
    pub questions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct Findings {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn has(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(&needle.to_lowercase())
}

fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Sentences of `text`: split after `.`, `!` or `?` only when followed by whitespace or the end of
/// the text (so a decimal such as `7.4` stays whole), and at line breaks.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\n' {
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            continue;
        }
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?') && chars.peek().is_none_or(|n| n.is_whitespace()) {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Reads `notes/*.md` and `questions.jsonl` from a rendered fixture directory.
/// Like [`load_rendered`] but for resuming a crashed render: whatever notes and question wording
/// exist are returned, and a missing `notes/` or `questions.jsonl` (written only at the very end of
/// a run) is simply empty rather than an error.
pub fn load_rendered_partial(dir: &Path) -> Rendered {
    let mut r = Rendered::default();
    if let Ok(rd) = std::fs::read_dir(dir.join("notes")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                if let (Some(stem), Ok(body)) = (p.file_stem(), std::fs::read_to_string(&p)) {
                    r.notes.insert(stem.to_string_lossy().to_string(), body);
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("questions.jsonl")) {
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let (Some(id), Some(t)) = (v["id"].as_str(), v["text"].as_str()) {
                    r.questions.insert(id.to_string(), t.to_string());
                }
            }
        }
    }
    r
}

pub fn load_rendered(dir: &Path) -> Result<Rendered, String> {
    let mut r = Rendered::default();
    let notes = dir.join("notes");
    for e in std::fs::read_dir(&notes)
        .map_err(|e| format!("{}: {e}", notes.display()))?
        .flatten()
    {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "md") {
            let id = p
                .file_stem()
                .expect("file stem")
                .to_string_lossy()
                .to_string();
            r.notes.insert(
                id,
                std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
            );
        }
    }
    let q = dir.join("questions.jsonl");
    for line in std::fs::read_to_string(&q)
        .map_err(|e| format!("{}: {e}", q.display()))?
        .lines()
    {
        let v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("questions.jsonl: {e}"))?;
        if let (Some(id), Some(t)) = (v["id"].as_str(), v["text"].as_str()) {
            r.questions.insert(id.to_string(), t.to_string());
        }
    }
    Ok(r)
}

/// Checks that depend only on the skeleton itself.
pub fn check_skeleton(sk: &Skeleton) -> Findings {
    let mut f = Findings::default();
    let ids: std::collections::BTreeSet<&str> = sk.notes.iter().map(|n| n.id.as_str()).collect();
    for q in &sk.questions {
        for r in &q.relevant_notes {
            if !ids.contains(r.note.as_str()) {
                f.errors.push(format!(
                    "question '{}': relevant note '{}' is not a skeleton note",
                    q.id, r.note
                ));
            }
        }
        for e in &q.evidence {
            match sk.note(&e.note) {
                Some(n) if n.facts.iter().any(|x| x.statement == e.key_fact) => {}
                _ => f.errors.push(format!(
                    "question '{}': evidence key fact is not a fact of note '{}'",
                    q.id, e.note
                )),
            }
        }
        if q.category == "passage" {
            let sec = q.evidence.first().and_then(|e| {
                sk.note(&e.note)
                    .and_then(|n| n.facts.iter().find(|x| x.statement == e.key_fact))
                    .and_then(|x| x.section)
            });
            if sec.unwrap_or(0) < 1 {
                f.errors.push(format!(
                    "question '{}': passage fact is not beyond the first section",
                    q.id
                ));
            }
        }
    }
    for n in &sk.notes {
        for c in &n.cites {
            if !ids.contains(c.as_str()) {
                f.errors
                    .push(format!("note '{}': cites unknown note '{c}'", n.id));
            }
        }
    }
    f
}

/// Checks `rendered` against `sk`.
pub fn check_rendered(sk: &Skeleton, r: &Rendered) -> Findings {
    let mut f = check_skeleton(sk);
    let all_names: Vec<&str> = sk.entities.iter().map(|e| e.name.as_str()).collect();

    // Note set.
    for n in &sk.notes {
        if !r.notes.contains_key(&n.id) {
            f.errors.push(format!("note '{}': not rendered", n.id));
        }
    }
    for id in r.notes.keys() {
        if sk.note(id).is_none() {
            f.errors
                .push(format!("note '{id}': rendered but not in the skeleton"));
        }
    }

    for n in &sk.notes {
        let Some(body) = r.notes.get(&n.id) else {
            continue;
        };
        for fact in &n.facts {
            if !has(body, &fact.entity) {
                f.errors.push(format!(
                    "note '{}': entity '{}' is not named",
                    n.id, fact.entity
                ));
            }
            if !has(body, &fact.value) {
                f.errors.push(format!(
                    "note '{}': the value '{}' ({} of {}) does not appear",
                    n.id, fact.value, fact.attribute, fact.entity
                ));
            }
        }

        // No other entity may be named.
        let own: Vec<&str> = n.entities.iter().map(|e| e.name.as_str()).collect();
        for name in &all_names {
            if has(body, name)
                && !own.contains(name)
                && !n
                    .facts
                    .iter()
                    .any(|x| has(&x.value, name) || x.entity == *name)
            {
                f.errors.push(format!(
                    "note '{}': names '{name}', which it is not about",
                    n.id
                ));
            }
        }

        // Decision notes.
        if n.kind == "decision" {
            if n.explicit == Some(true) {
                if let Some(eff) = &n.effective_text {
                    if !has(body, eff) {
                        f.errors.push(format!(
                            "note '{}': an explicit revision must state its effective date '{eff}'",
                            n.id
                        ));
                    }
                }
            }
            for v in &n.earlier_values {
                // Generic one-word values ("weekly") appear innocently; check named things only.
                let named = v.chars().any(|c| c.is_uppercase() || c.is_ascii_digit());
                if named && has(body, v) {
                    f.errors.push(format!(
                        "note '{}': an implicit revision must not mention the earlier value '{v}'",
                        n.id
                    ));
                }
            }
        }

        // Citations.
        if n.origin == "derived" || n.origin == "answer" {
            for c in &n.cites {
                if !body.contains(&format!("[[{c}]]")) {
                    f.errors
                        .push(format!("note '{}': does not cite [[{c}]]", n.id));
                }
            }
        }

        // Size and structure.
        let wc = words(body);
        if n.kind == "journal" && wc <= 5000 {
            f.errors.push(format!(
                "note '{}': a journal must exceed 5,000 words, has {wc}",
                n.id
            ));
        }
        if n.sections > 1 {
            let headings = body.matches("\n## ").count();
            if headings + 1 < n.sections {
                f.errors.push(format!(
                    "note '{}': {} sections need {} '## ' headings, found {headings}",
                    n.id,
                    n.sections,
                    n.sections - 1
                ));
            }
        }
        let (lo, hi) = if n.sections > 1 {
            (0.5, 1.6)
        } else {
            (0.35, 3.0)
        };
        let ratio = wc as f64 / n.target_words.max(1) as f64;
        if ratio < lo || ratio > hi {
            f.warnings.push(format!(
                "note '{}': {wc} words against a target of {} ({ratio:.2}×)",
                n.id, n.target_words
            ));
        }
    }

    // Questions.
    for q in &sk.questions {
        let Some(text) = r.questions.get(&q.id) else {
            f.errors.push(format!("question '{}': not rendered", q.id));
            continue;
        };
        if text.trim().is_empty() {
            f.errors.push(format!("question '{}': empty wording", q.id));
            continue;
        }
        if q.category == "paraphrase" {
            for name in &all_names {
                if has(text, name) {
                    f.errors.push(format!(
                        "question '{}': a paraphrase must not name '{name}'",
                        q.id
                    ));
                }
            }
            check_paraphrase(sk, r, q, text, &mut f);
        } else {
            for name in &q.names {
                if !has(text, name) {
                    f.errors.push(format!(
                        "question '{}': the wording must name '{name}'",
                        q.id
                    ));
                }
            }
        }
        for e in &q.evidence {
            if let (Some(n), Some(body)) = (sk.note(&e.note), r.notes.get(&e.note)) {
                let fact = n.facts.iter().find(|x| x.statement == e.key_fact);
                let findable = fact.is_some_and(|x| has(body, &x.entity) && has(body, &x.value))
                    && {
                        let kw = content_words(&e.key_fact);
                        kw.is_empty() || overlap(&e.key_fact, body) >= 0.6
                    };
                if !findable {
                    f.errors.push(format!(
                        "question '{}': key fact is not findable in note '{}'",
                        q.id, e.note
                    ));
                }
                if q.category == "passage" {
                    if let Some(x) = fact {
                        let chunks = chunk_markdown(body, sk.chunk_max_tokens);
                        match chunks.iter().position(|c| has(c, &x.value) && has(c, &x.entity)) {
                            Some(i) if i >= 1 && chunks.len() >= 2 => {}
                            _ => f.errors.push(format!(
                                "question '{}': the passage fact is not in a later chunk of a multi-chunk note '{}'",
                                q.id, e.note
                            )),
                        }
                    }
                }
            }
        }
        if q.negative {
            check_negative(sk, r, q, text, &mut f);
        }
    }
    f
}

fn check_paraphrase(sk: &Skeleton, r: &Rendered, q: &SkQuestion, text: &str, f: &mut Findings) {
    let Some(rel) = q.relevant_notes.first() else {
        return;
    };
    let (Some(n), Some(body)) = (sk.note(&rel.note), r.notes.get(&rel.note)) else {
        return;
    };
    let fact = q
        .basis
        .first()
        .and_then(|b| n.facts.iter().find(|x| &x.statement == b));
    if content_words(text).len() < PARAPHRASE_MIN_CONTENT_WORDS {
        f.errors.push(format!(
            "question '{}': a paraphrase needs at least {PARAPHRASE_MIN_CONTENT_WORDS} content words",
            q.id
        ));
    }
    let note_overlap = overlap(text, body);
    if note_overlap > PARAPHRASE_MAX_NOTE_OVERLAP {
        f.errors.push(format!(
            "question '{}': {:.0}% of its content words appear in note '{}' (limit {:.0}%)",
            q.id,
            note_overlap * 100.0,
            rel.note,
            PARAPHRASE_MAX_NOTE_OVERLAP * 100.0
        ));
    }
    if let Some(x) = fact {
        let carrying: Vec<String> = sentences(body)
            .into_iter()
            .filter(|s| has(s, &x.value))
            .collect();
        if carrying.is_empty() {
            f.errors.push(format!(
                "question '{}': no sentence of note '{}' carries the fact value '{}', so the paraphrase overlap cannot be checked",
                q.id, rel.note, x.value
            ));
        }
        let worst = carrying
            .iter()
            .map(|s| overlap(text, s))
            .fold(0.0_f64, f64::max);
        if worst > PARAPHRASE_MAX_FACT_OVERLAP {
            f.errors.push(format!(
                "question '{}': {:.0}% of its content words appear in the sentence carrying the fact in '{}' (limit {:.0}%)",
                q.id,
                worst * 100.0,
                rel.note,
                PARAPHRASE_MAX_FACT_OVERLAP * 100.0
            ));
        }
    }
}

/// A negative must still be a near miss: its wording shares at least this share of its content
/// words with some note, or it tests nothing. The v2 integration tests assert the same floor.
pub const NEGATIVE_MIN_NEAR_MISS_OVERLAP: f64 = 0.33;

/// Fewest content words a paraphrase question may have; fewer is not a searchable question.
pub const PARAPHRASE_MIN_CONTENT_WORDS: usize = 3;

fn check_negative(sk: &Skeleton, r: &Rendered, q: &SkQuestion, text: &str, f: &mut Findings) {
    let best = r
        .notes
        .values()
        .map(|b| overlap(text, b))
        .fold(0.0_f64, f64::max);
    if best < NEGATIVE_MIN_NEAR_MISS_OVERLAP {
        f.errors.push(format!(
            "question '{}': no note shares {:.0}% of its content words (best {:.0}%), so it is not a near miss",
            q.id,
            NEGATIVE_MIN_NEAR_MISS_OVERLAP * 100.0,
            best * 100.0
        ));
    }
    let Some(name) = q.names.first() else { return };
    if q.markers.is_empty() {
        // A fake entity must appear nowhere.
        if let Some((id, _)) = r.notes.iter().find(|(_, b)| has(b, name)) {
            f.errors.push(format!(
                "question '{}': the fake entity '{name}' appears in note '{id}', so the question is answerable",
                q.id
            ));
        }
    } else if let Some((id, _)) = r
        .notes
        .iter()
        .find(|(_, b)| has(b, name) && q.markers.iter().any(|m| has(b, m)))
    {
        f.errors.push(format!(
            "question '{}': note '{id}' mentions '{name}' together with the asked attribute, so the question is answerable",
            q.id
        ));
    }
    let _ = sk;
}

/// Renders `findings` for a terminal.
pub fn describe(f: &Findings) -> String {
    let mut s = String::new();
    for e in f.errors.iter().take(40) {
        s.push_str(&format!("  error: {e}\n"));
    }
    if f.errors.len() > 40 {
        s.push_str(&format!("  … and {} more errors\n", f.errors.len() - 40));
    }
    for w in f.warnings.iter().take(15) {
        s.push_str(&format!("  warning: {w}\n"));
    }
    if f.warnings.len() > 15 {
        s.push_str(&format!(
            "  … and {} more warnings\n",
            f.warnings.len() - 15
        ));
    }
    s
}
