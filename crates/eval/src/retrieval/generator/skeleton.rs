//! The deterministic skeleton of the retrieval-v2 fixture (#788 scope change, ADR-0788).
//!
//! The skeleton is everything about the corpus *except its prose*: entities and facts, which note
//! carries which facts, each note's origin, reference time, target length and citations, the
//! explicit supersessions, and the question set's ground truth (relevant notes, gains, evidence key
//! facts, current/stale). The same scenario and seed give a byte-identical `skeleton.json`.
//!
//! Prose is rendered once by an LLM in a separate, explicitly invoked step ([`super::render`]) and
//! the rendered notes and question wordings are checked in. [`super::check`] verifies the rendered
//! text against this skeleton. CI and ordinary eval runs never call a model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A named thing a note may mention (and the words that describe it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkEntity {
    pub name: String,
    /// `project`, `vendor`, `person` or `policy`.
    pub kind: String,
    /// How notes call this kind of thing ("supplier").
    pub kind_note: String,
    /// How a paraphrase question calls it ("outside provider").
    pub kind_question: String,
    /// What it is about, as notes say it ("billing").
    pub domain_note: String,
    /// What it is about, as a paraphrase question says it ("invoicing and payment collection").
    pub domain_question: String,
}

/// One fact a note must state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkFact {
    pub entity: String,
    pub attribute: String,
    /// Must appear verbatim (case-insensitive) in the rendered note.
    pub value: String,
    /// Canonical statement of the fact; the renderer may reword it but must keep the entity name
    /// and the value. It is also the evidence key fact of questions that rely on this fact.
    pub statement: String,
    /// For long notes: the 0-based section that carries the fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<usize>,
}

/// One note, without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkNote {
    pub id: String,
    /// `fact`, `decision`, `recap`, `digest`, `journal`, `clip`, `wiki` or `answer`.
    pub kind: String,
    pub origin: String,
    pub reference_time: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cites: Vec<String>,
    pub target_words: usize,
    /// Sections of a long note (each is rendered, and chunked, separately); 1 otherwise.
    pub sections: usize,
    pub title_hint: String,
    /// Entities this note is about (it may name these and no other entity).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<SkEntity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<SkFact>,
    /// Decision notes: the decision topic ("hosting provider").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Decision notes: recorded as an explicit supersession of the previous step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit: Option<bool>,
    /// Decision notes (explicit steps): when the change takes effect, as prose ("26 September 2025").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_text: Option<String>,
    /// Decision notes: earlier values the note must not mention (implicit steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkSup {
    pub old_note: String,
    pub new_note: String,
    pub effective_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkRel {
    pub note: String,
    pub gain: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkEv {
    pub note: String,
    pub key_fact: String,
}

/// A question's ground truth plus what the wording must respect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkQuestion {
    pub id: String,
    pub category: String,
    /// What the question asks, in a sentence (for the renderer and the human reviewer).
    pub intent: String,
    /// A plain wording of the question. The LLM renderer rewrites it; it is never the checked-in
    /// text of the real fixture.
    pub text_hint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevant_notes: Vec<SkRel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<SkEv>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub negative: bool,
    /// Names that must appear verbatim in the wording (empty for paraphrase questions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// Paraphrase: how to refer to the subject instead of naming it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<String>,
    /// Canonical statements the answer rests on (informs the renderer and the checks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub basis: Vec<String>,
    /// Negatives about a real entity: attribute marker words that must never share a note with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skeleton {
    pub skeleton_version: u32,
    pub corpus_version: String,
    pub questions_version: String,
    pub description: String,
    pub seed: u64,
    pub chunk_max_tokens: usize,
    pub entities: Vec<SkEntity>,
    /// Notes in manifest (ingest) order.
    pub notes: Vec<SkNote>,
    pub supersedes: Vec<SkSup>,
    pub questions: Vec<SkQuestion>,
}

pub const SKELETON_VERSION: u32 = 1;

impl Skeleton {
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("serializable");
        s.push('\n');
        s
    }

    pub fn from_json(text: &str) -> Result<Skeleton, String> {
        serde_json::from_str(text).map_err(|e| format!("skeleton.json: {e}"))
    }

    pub fn note(&self, id: &str) -> Option<&SkNote> {
        self.notes.iter().find(|n| n.id == id)
    }

    /// Questions per category label.
    pub fn category_counts(&self) -> BTreeMap<&str, usize> {
        let mut m = BTreeMap::new();
        for q in &self.questions {
            *m.entry(q.category.as_str()).or_default() += 1;
        }
        m
    }
}

/// Hash of `skeleton.json` text (first 16 hex of sha256), recorded in `render.json` so a rendered
/// fixture can be recognised as stale when the skeleton changes.
pub fn skeleton_hash(text: &str) -> String {
    lcg_core::sha256_hex::sha256_hex(text.as_bytes())[..16].to_string()
}
