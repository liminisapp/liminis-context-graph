//! `knowledge_lint` result (lint/mod.rs `response` and `hydrate`).
//!
//! Findings are discriminated by `category`; each category carries its own `details` shape. The
//! shapes a consumer once guessed wrong (#852) are pinned here: stale pages carry
//! `details.derived_document` and a `causes` map; weak-support findings carry the kind in
//! `finding.reason` and a `support` tally in `details`.

use super::Nullable;
use schemars::JsonSchema;
use std::collections::BTreeMap;

/// An entity cited by a finding.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct EntityRef {
    pub uuid: String,
    pub name: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct FactCitation {
    pub uuid: String,
    pub source: EntityRef,
    pub target: EntityRef,
    pub relation: String,
}

/// Evidence for a finding; lists are capped, `*_total` carries the uncapped count.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Citations {
    pub episodes: Vec<String>,
    pub episodes_total: u64,
    pub documents: Vec<String>,
    pub documents_total: u64,
    pub entities: Vec<EntityRef>,
    pub entities_total: u64,
    pub facts: Vec<FactCitation>,
    pub facts_total: u64,
}

/// Distinct supporting documents per origin.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Support {
    pub primary: u64,
    pub external: u64,
    pub derived: u64,
    pub answer: u64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct FormerlySupporting {
    pub episode: String,
    pub document: Nullable<String>,
    pub origin: String,
    pub cause: String,
}

/// `stale_derived_page` details.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct StaleDetails {
    /// The derived page's document id; `null` when it has none.
    pub derived_document: Nullable<String>,
    pub affected_facts: u64,
    pub total_facts: u64,
    /// Count of formerly-supporting episodes per cause, keyed by cause.
    pub causes: BTreeMap<String, u64>,
    pub formerly_supporting: Vec<FormerlySupporting>,
}

/// `weak_support` details; the kind (`derived_only` / `single_source`) is `finding.reason`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct WeakDetails {
    pub support: Support,
    pub corroboration: u64,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ContradictionSide {
    pub target: String,
    pub facts: Vec<String>,
    pub support: Support,
    pub corroboration: u64,
}

/// `contradiction` details.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct ContradictionDetails {
    pub relation: String,
    pub source: String,
    pub sides: Vec<ContradictionSide>,
}

/// `orphan_entity` details.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct OrphanDetails {
    pub superseded_mentions: u64,
}

/// `near_duplicate_entity` details.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct NearDuplicateDetails {
    pub score: f64,
    pub signal: String,
    pub kind: String,
    pub names: Vec<String>,
    /// Cosine of the stored name embeddings; `null` when unavailable.
    pub embedding_cosine: Nullable<f64>,
}

/// `unlinked_mention` details when `reason` is `unlinked_mention`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct UnlinkedMentionDetails {
    pub document: String,
    pub page: String,
}

/// `unlinked_mention` details when `reason` is `missing_page`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct MissingPageDetails {
    pub mention_documents: u64,
    pub min_mentions: u64,
}

#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
pub enum UnlinkedDetails {
    UnlinkedMention(UnlinkedMentionDetails),
    MissingPage(MissingPageDetails),
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum StaleReason {
    LostCorroboration,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum WeakReason {
    DerivedOnly,
    SingleSource,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum ContradictionReason {
    SingleValuedConflict,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum OrphanReason {
    SupportLost,
    NeverSupported,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum NearDuplicateReason {
    SimilarName,
}

#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum UnlinkedReason {
    UnlinkedMention,
    MissingPage,
}

/// A finding, discriminated by `category`.
#[derive(JsonSchema)]
#[serde(tag = "category", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum Finding {
    StaleDerivedPage {
        id: String,
        group_id: String,
        reason: StaleReason,
        details: StaleDetails,
        citations: Citations,
    },
    WeakSupport {
        id: String,
        group_id: String,
        reason: WeakReason,
        details: WeakDetails,
        citations: Citations,
    },
    Contradiction {
        id: String,
        group_id: String,
        reason: ContradictionReason,
        details: ContradictionDetails,
        citations: Citations,
    },
    OrphanEntity {
        id: String,
        group_id: String,
        reason: OrphanReason,
        details: OrphanDetails,
        citations: Citations,
    },
    NearDuplicateEntity {
        id: String,
        group_id: String,
        reason: NearDuplicateReason,
        details: NearDuplicateDetails,
        citations: Citations,
    },
    UnlinkedMention {
        id: String,
        group_id: String,
        reason: UnlinkedReason,
        details: UnlinkedDetails,
        citations: Citations,
    },
}

/// Per-category outcome in `summary.categories`.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct CategorySummary {
    /// `ok` or `unavailable`.
    pub status: String,
    pub count: u64,
    /// Why the category is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `contradiction` only, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_subjects: Option<u64>,
    /// `near_duplicate_entity` only, when it ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_blocks: Option<u64>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Summary {
    /// Keyed by category name.
    pub categories: BTreeMap<String, CategorySummary>,
    pub total: u64,
}

/// The parameters the report was computed with.
#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct Effective {
    pub categories: Vec<String>,
    pub limit: u64,
    pub near_duplicate_threshold: f64,
    pub min_mentions: u64,
    pub single_valued_relations: Vec<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
pub struct LintResult {
    pub findings: Vec<Finding>,
    pub count: u64,
    /// Opaque cursor for the next page; `null` on the last page.
    pub next_cursor: Nullable<String>,
    pub summary: Summary,
    pub effective: Effective,
}
