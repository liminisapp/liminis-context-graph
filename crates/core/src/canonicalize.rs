//! Relation canonicalization pass — maps free-text edge `name` to a controlled `relation_type`.
//!
//! Two-layer architecture (per spec #163):
//!   1. Lexical pass: stem/keyword rules driven from the workspace ontology.
//!   2. Embedding fallback (P2): cosine similarity against canonical type description glosses.
//!
//! Co-occurrence noise edges (`ALICE → BOB` pattern) are reclassified to UNCLASSIFIED and
//! WAL-recorded. They are NEVER deleted — 94% of arrow-named edges carry rich `relation_type`
//! values that would be irreversibly lost. See ADR-0033.
//! Unmatched residual edges are also marked `UNCLASSIFIED`.
//!
//! Crash mid-pass: only committed batches appear in the WAL. The pass is idempotent, so
//! re-running after recovery is safe — a second run adds zero new WAL mutations.
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use regex::Regex;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    app_state::AppState,
    db::{cosine_similarity, value_as_string},
    direction::{self, Orientation},
    embedder::EmbedRole,
    error::Error,
    ontology::{normalize_relation_type, Ontology},
    wal_exec,
};

const DEFAULT_EMBEDDING_THRESHOLD: f32 = 0.7;
const PAGE_SIZE: usize = 500;
const WRITE_BATCH_SIZE: usize = 250;
const PROGRESS_EVERY: usize = 5000;

// ── Public types ──────────────────────────────────────────────────────────────

pub struct CanonicalizeParams {
    pub group_id: String,
    pub dry_run: bool,
    pub embedding_threshold: Option<f32>,
}

pub struct CanonicalizeReport {
    pub group_id: String,
    pub total_edges: usize,
    pub mapped_count: usize,
    pub noise_count: usize,
    pub residual_count: usize,
    pub embedding_fallback_promoted: usize,
    /// Edges whose endpoints were (or, in a dry run, would be) swapped (#816).
    pub swapped_count: usize,
    /// Edges that are (or would be) marked `signature_mismatch` (#816).
    pub signature_mismatch_count: usize,
    /// Per relation type direction counts (#816). Empty when the ontology declares no
    /// signature and no inverse alias.
    pub by_type: BTreeMap<String, TypeDirectionCounts>,
    pub dry_run: bool,
}

/// Direction-check counts for one relation type (#816): `checked` = edges that could be
/// evaluated (typed endpoints, signature declared), `swapped` and `mismatched` as in the ingest
/// rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeDirectionCounts {
    pub checked: usize,
    pub swapped: usize,
    pub mismatched: usize,
}

impl CanonicalizeReport {
    fn to_json(&self) -> Value {
        let total = self.total_edges as f64;
        json!({
            "group_id": self.group_id,
            "total_edges": self.total_edges,
            "mapped_count": self.mapped_count,
            "noise_count": self.noise_count,
            "residual_count": self.residual_count,
            "embedding_fallback_promoted": self.embedding_fallback_promoted,
            "swapped_count": self.swapped_count,
            "signature_mismatch_count": self.signature_mismatch_count,
            "by_type": self.by_type.iter().map(|(t, c)| (t.clone(), json!({
                "checked": c.checked,
                "swapped": c.swapped,
                "mismatched": c.mismatched,
            }))).collect::<serde_json::Map<String, Value>>(),
            "mapped_pct": if total > 0.0 { (self.mapped_count as f64 / total * 100.0).round() } else { 0.0 },
            "noise_pct": if total > 0.0 { (self.noise_count as f64 / total * 100.0).round() } else { 0.0 },
            "residual_pct": if total > 0.0 { (self.residual_count as f64 / total * 100.0).round() } else { 0.0 },
            "dry_run": self.dry_run,
        })
    }
}

#[derive(Debug, Clone)]
pub enum EdgeClass {
    /// Edge maps to this canonical relation type.
    Mapped(String),
    /// Edge maps to this canonical relation type via an *inverse* alias (or an inverse keyword
    /// hit, #816): the name reads the relation backwards, so source and target are swapped
    /// when the retype is applied.
    MappedInverse(String),
    /// Co-occurrence noise edge — reclassified to UNCLASSIFIED (never deleted; see ADR-0033).
    Noise,
    /// No lexical or embedding rule matched — mark UNCLASSIFIED.
    Residual,
}

#[derive(Debug, Clone)]
pub struct EdgeRecord {
    pub uuid: String,
    pub name: String,
    pub relation_type: Option<String>,
    pub fact: String,
}

// ── Lexical index ─────────────────────────────────────────────────────────────

pub struct LexicalIndex {
    /// SCREAMING_SNAKE_CASE token → (canonical type name, inverted) (exact match, #816).
    exact: HashMap<String, (String, bool)>,
    /// (lowercase keyword, canonical type name) for substring matching.
    keywords: Vec<(String, String)>,
    /// canonical type name → lowercase inverse aliases. A keyword hit counts as an inverse
    /// reading only when the text also contains one of the matched type's inverse aliases.
    inverse_texts: HashMap<String, Vec<String>>,
    /// All canonical type names (for checking if current_rt is already canonical).
    canonical_names: std::collections::HashSet<String>,
    /// Declared `extract: false` relation names (#637). An edge already typed as one of these
    /// was asserted by the host and is never remapped by name or keyword evidence.
    assert_only_names: std::collections::HashSet<String>,
}

/// Builds the alias→canonical relation-type map: each canonical name maps to itself, and each
/// declared alias maps to its canonical name. Shared by `build_lexical_index` (the offline
/// `knowledge_canonicalize_relations` pass) and the ingest-time strict-mode filter in
/// `episode.rs` (FR-001) — one map, two consumers, per ADR-0310. Only extractable relation
/// types (#637) are mapped, so neither consumer can retype an edge *into* an `extract: false`
/// type from extracted evidence.
pub fn build_alias_map(ontology: &Ontology) -> HashMap<String, String> {
    build_alias_map_directed(ontology)
        .into_iter()
        .map(|(alias, (canonical, _))| (alias, canonical))
        .collect()
}

/// Like [`build_alias_map`] but carries direction (#816): each entry is
/// `(canonical, inverted)`, where `inverted` is true for an `inverse_aliases` entry — a name
/// that reads the relation backwards and so needs source and target swapped.
///
/// Canonical names and forward aliases are inserted first (forward aliases keep their existing
/// last-wins behaviour); inverse aliases are added only where the name is still free, so a
/// collision always resolves toward the forward reading, deterministically.
pub fn build_alias_map_directed(ontology: &Ontology) -> HashMap<String, (String, bool)> {
    let mut exact: HashMap<String, (String, bool)> = HashMap::new();
    for rt in ontology.extractable_relation_types() {
        // Canonical name maps to itself
        exact.insert(rt.name.clone(), (rt.name.clone(), false));
        // Aliases map to the canonical name
        for alias in &rt.aliases {
            exact.insert(alias.clone(), (rt.name.clone(), false));
        }
    }
    for rt in ontology.extractable_relation_types() {
        for ia in &rt.inverse_aliases {
            exact
                .entry(ia.clone())
                .or_insert_with(|| (rt.name.clone(), true));
        }
    }
    exact
}

pub fn build_lexical_index(ontology: &Ontology) -> LexicalIndex {
    let exact = build_alias_map_directed(ontology);
    let mut inverse_texts: HashMap<String, Vec<String>> = HashMap::new();
    for rt in ontology.extractable_relation_types() {
        if !rt.inverse_aliases.is_empty() {
            inverse_texts.insert(
                rt.name.clone(),
                rt.inverse_aliases
                    .iter()
                    .map(|a| a.to_lowercase())
                    .collect(),
            );
        }
    }
    let mut keywords: Vec<(String, String)> = Vec::new();
    let mut canonical_names = std::collections::HashSet::new();
    let mut assert_only_names = std::collections::HashSet::new();

    for rt in &ontology.relation_types {
        // Every declared name stays "already canonical" (idempotent for asserted edges),
        // including `extract: false` ones (#637).
        canonical_names.insert(rt.name.clone());
        if !rt.extract {
            assert_only_names.insert(rt.name.clone());
            continue;
        }
        // Keywords for substring matching
        for kw in &rt.keywords {
            keywords.push((kw.clone(), rt.name.clone()));
        }
    }

    LexicalIndex {
        exact,
        keywords,
        inverse_texts,
        canonical_names,
        assert_only_names,
    }
}

// ── Noise detection ───────────────────────────────────────────────────────────

/// Returns true if `s` matches the co-occurrence noise pattern: `X → Y` or `X -> Y`
/// where both sides start with an uppercase letter (optionally followed by uppercase letters,
/// digits, and spaces). Bare lowercase words cannot match.
pub fn is_noise_edge(s: &str) -> bool {
    static NOISE_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = NOISE_RE
        .get_or_init(|| Regex::new(r"^[A-Z][A-Z0-9 ]*\s*(→|->)\s*[A-Z][A-Z0-9 ]*$").unwrap());
    re.is_match(s.trim())
}

// ── Lexical classification ────────────────────────────────────────────────────

fn mapped(canonical: &str, inverted: bool) -> EdgeClass {
    if inverted {
        EdgeClass::MappedInverse(canonical.to_string())
    } else {
        EdgeClass::Mapped(canonical.to_string())
    }
}

impl LexicalIndex {
    /// True when `lower_text` (lowercased, normalized) contains one of `canonical`'s inverse
    /// aliases — the only evidence a keyword hit reads the relation backwards (#816).
    fn reads_inverse(&self, canonical: &str, lower_text: &str) -> bool {
        self.inverse_texts
            .get(canonical)
            .is_some_and(|v| v.iter().any(|ia| lower_text.contains(ia.as_str())))
    }
}

/// Classifies a single edge lexically using the ontology index.
///
/// Order of checks (FR-002: match against both `name` and `relation_type`):
/// 1. Noise pattern on `name` field → Noise
/// 2. Noise pattern on `relation_type` field → Noise
/// 3. Normalized `name` in exact map (canonical name or alias) → Mapped
/// 4. Any keyword matches a substring of the lowercase normalized name → Mapped
/// 5. `relation_type` is already a canonical name → Mapped (idempotent)
/// 6. Normalized `relation_type` in exact map (alias) or keyword match → Mapped
/// 7. → Residual
///
/// Before step 3, an edge whose `relation_type` is a declared `extract: false` name is returned
/// as `Mapped` to itself, so asserted assert-only edges are never retyped (#637).
pub fn classify_edge_lexically(
    name: &str,
    current_rt: Option<&str>,
    idx: &LexicalIndex,
) -> EdgeClass {
    // Noise checks
    if is_noise_edge(name) {
        return EdgeClass::Noise;
    }
    if let Some(rt) = current_rt {
        if is_noise_edge(rt) {
            return EdgeClass::Noise;
        }
    }

    // An edge already typed as an `extract: false` relation was asserted by the host (#637):
    // keep it as is rather than letting name/keyword evidence from extractable types retype it.
    if let Some(rt) = current_rt {
        if idx.assert_only_names.contains(rt) {
            return EdgeClass::Mapped(rt.to_string());
        }
    }

    // Exact match on normalized name
    let normalized = normalize_relation_type(name);
    if let Some((canonical, inverted)) = idx.exact.get(&normalized) {
        return mapped(canonical, *inverted);
    }

    // Keyword substring match on lowercase normalized name
    let lower = normalized.to_lowercase();
    for (kw, canonical) in &idx.keywords {
        if lower.contains(kw.as_str()) {
            return mapped(canonical, idx.reads_inverse(canonical, &lower));
        }
    }

    // Secondary signal: check relation_type field (FR-002 — match on name *or* relation_type).
    // Covers edges where name is uninformative but relation_type was set to an alias by an
    // earlier pass, or the extractor used a variant that normalized to an alias form.
    if let Some(rt) = current_rt {
        if idx.canonical_names.contains(rt) {
            // Already canonical — idempotent (post-#94 ingestion path)
            return EdgeClass::Mapped(rt.to_string());
        }
        let rt_normalized = normalize_relation_type(rt);
        if let Some((canonical, inverted)) = idx.exact.get(&rt_normalized) {
            return mapped(canonical, *inverted);
        }
        let rt_lower = rt_normalized.to_lowercase();
        for (kw, canonical) in &idx.keywords {
            if rt_lower.contains(kw.as_str()) {
                return mapped(canonical, idx.reads_inverse(canonical, &rt_lower));
            }
        }
    }

    EdgeClass::Residual
}

// ── Direction plan (#816) ─────────────────────────────────────────────────────

/// Both endpoints of a stored edge plus its attributes, read for the direction check.
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    pub src_uuid: String,
    pub src_name: String,
    pub src_labels: Vec<String>,
    pub dst_uuid: String,
    pub dst_name: String,
    pub dst_labels: Vec<String>,
    pub name: String,
    pub attributes: String,
}

/// The direction write for one edge: swap its endpoints and/or change its attributes.
#[derive(Debug, Clone)]
pub struct DirectionWrite {
    pub rn_uuid: String,
    /// `Some((new_src, new_dst))` when the edge's endpoints are swapped.
    pub swap_to: Option<(String, String)>,
    /// `Some` only when the swapped edge's ingest-form name (`"{src} → {dst}"`) is rewritten.
    pub new_name: Option<String>,
    /// `Some` when the `signature_mismatch` mark changes (full read-merged attributes JSON).
    pub new_attributes: Option<String>,
}

const SIGNATURE_MISMATCH_KEY: &str = "signature_mismatch";

/// Per-type counts delta from [`plan_direction`]: `(relation type, checked, swapped, mismatched)`.
pub type DirectionCounts = (String, usize, usize, usize);

/// Decides the direction write for one edge, or `None` when nothing changes. Shared rule set with
/// ingest ([`direction::check_orientation`]), plus the repair-only parts:
/// - an edge retyped through an inverse alias/keyword (`inverse_forced`) is swapped first;
/// - `signature_mismatch` is set on a mismatch and cleared once the edge fits (or is swapped to
///   fit), so the mark always reflects the current ontology; a skipped edge keeps its mark.
///
/// Returns the write (if any) and the per-type counts delta.
pub fn plan_direction(
    onto: &Ontology,
    final_rt: &str,
    inverse_forced: bool,
    rn_uuid: &str,
    info: &EndpointInfo,
) -> (Option<DirectionWrite>, Option<DirectionCounts>) {
    let Some(rt_def) = onto.relation_types.iter().find(|r| r.name == final_rt) else {
        return (None, None);
    };
    let src_types = direction::endpoint_types(onto, &info.src_labels);
    let dst_types = direction::endpoint_types(onto, &info.dst_labels);
    // An inverse reading is the direction information itself, so it is applied even when the
    // canonical type declares no signature. Symmetric types ignore endpoint order entirely.
    let forced = inverse_forced && !rt_def.symmetric;
    let (eff_src, eff_dst) = if forced {
        (dst_types.as_deref(), src_types.as_deref())
    } else {
        (src_types.as_deref(), dst_types.as_deref())
    };
    let orientation = direction::check_orientation(onto, rt_def, eff_src, eff_dst);
    let sig_swap = orientation == Orientation::Swap;
    let swap = forced ^ sig_swap;
    let mismatch = orientation == Orientation::Mismatch;
    let evaluated = orientation != Orientation::Skip;

    // Mark: set on mismatch, cleared on keep/swap, untouched on skip.
    let mut attrs: serde_json::Map<String, Value> = serde_json::from_str::<Value>(&info.attributes)
        .ok()
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();
    let has_mark = attrs.get(SIGNATURE_MISMATCH_KEY) == Some(&Value::Bool(true));
    let mut new_attributes = None;
    match orientation {
        Orientation::Mismatch if !has_mark => {
            attrs.insert(SIGNATURE_MISMATCH_KEY.to_string(), Value::Bool(true));
            new_attributes = Some(Value::Object(attrs).to_string());
        }
        Orientation::Keep | Orientation::Swap if has_mark => {
            attrs.remove(SIGNATURE_MISMATCH_KEY);
            new_attributes = Some(Value::Object(attrs).to_string());
        }
        _ => {}
    }

    let counts = if evaluated || forced {
        Some((
            rt_def.name.clone(),
            usize::from(evaluated),
            usize::from(swap),
            usize::from(mismatch),
        ))
    } else {
        None
    };
    if !swap && new_attributes.is_none() {
        return (None, counts);
    }
    let (swap_to, new_name) = if swap {
        let ingest_form = format!("{} → {}", info.src_name, info.dst_name);
        let new_name =
            (info.name == ingest_form).then(|| format!("{} → {}", info.dst_name, info.src_name));
        (
            Some((info.dst_uuid.clone(), info.src_uuid.clone())),
            new_name,
        )
    } else {
        (None, None)
    };
    (
        Some(DirectionWrite {
            rn_uuid: rn_uuid.to_string(),
            swap_to,
            new_name,
            new_attributes,
        }),
        counts,
    )
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Runs the canonicalization pass as a four-phase async operation.
///
/// Callers must add `knowledge_canonicalize_relations` to `service_protocol.py`
/// in the liminis-app repo.
pub async fn canonicalize_relations(
    state: Arc<AppState>,
    params: CanonicalizeParams,
    progress_tx: Option<UnboundedSender<Value>>,
    ontology: Arc<Ontology>,
) -> Result<Value, Error> {
    let threshold = params
        .embedding_threshold
        .unwrap_or(DEFAULT_EMBEDDING_THRESHOLD);

    // ── Phase A: paginated read of all RelatesToNode_ edges (read lock) ───────
    let db = state
        .db
        .load_full()
        .ok_or_else(|| Error::DbUnavailable("DB unavailable".to_string()))?;
    let _read_guard = state.write_lock.read().await;
    let db_a = Arc::clone(&db);
    let group_id_a = params.group_id.clone();
    let edges: Vec<EdgeRecord> = tokio::task::spawn_blocking(move || {
        let conn = db_a.connect().map_err(|e| Error::Ipc(format!("db: {e}")))?;
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let rows = conn
                .dump_relatos_page(Some(&group_id_a), offset, PAGE_SIZE)
                .map_err(|e| Error::Ipc(format!("read edges page: {e}")))?;
            let count = rows.len();
            for row in &rows {
                let uuid = value_as_string(&row[0]);
                let name = value_as_string(&row[1]);
                let fact = value_as_string(&row[4]);
                let rt_str = value_as_string(&row[11]);
                let relation_type = if rt_str.is_empty() {
                    None
                } else {
                    Some(rt_str)
                };
                all.push(EdgeRecord {
                    uuid,
                    name,
                    fact,
                    relation_type,
                });
            }
            if count < PAGE_SIZE {
                break;
            }
            offset += count;
        }
        Ok::<_, Error>(all)
    })
    .await??;
    // ── Phase A2: endpoints + labels for the direction check (#816) ───────────
    // Skipped entirely when the ontology declares no signature and no inverse alias, so an
    // ontology without direction info pays nothing and behaves exactly as before.
    let endpoints: HashMap<String, EndpointInfo> = if ontology.has_direction_info() {
        let db_e = Arc::clone(&db);
        let group_id_e = params.group_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db_e.connect().map_err(|e| Error::Ipc(format!("db: {e}")))?;
            let mut all: HashMap<String, EndpointInfo> = HashMap::new();
            let mut offset = 0;
            loop {
                let rows = conn
                    .dump_relatos_endpoints_page(&group_id_e, offset, PAGE_SIZE)
                    .map_err(|e| Error::Ipc(format!("read edge endpoints page: {e}")))?;
                let count = rows.len();
                for row in &rows {
                    all.insert(
                        value_as_string(&row[0]),
                        EndpointInfo {
                            name: value_as_string(&row[1]),
                            attributes: value_as_string(&row[2]),
                            src_uuid: value_as_string(&row[3]),
                            src_name: value_as_string(&row[4]),
                            src_labels: crate::db::value_as_str_list(&row[5]),
                            dst_uuid: value_as_string(&row[6]),
                            dst_name: value_as_string(&row[7]),
                            dst_labels: crate::db::value_as_str_list(&row[8]),
                        },
                    );
                }
                if count < PAGE_SIZE {
                    break;
                }
                offset += count;
            }
            Ok::<_, Error>(all)
        })
        .await??
    } else {
        HashMap::new()
    };
    drop(_read_guard);

    let total_edges = edges.len();

    // ── Phase B: lexical classification ───────────────────────────────────────
    let idx = build_lexical_index(&ontology);
    let mut classifications: Vec<(EdgeRecord, EdgeClass)> = Vec::with_capacity(edges.len());
    for (i, edge) in edges.into_iter().enumerate() {
        if i % PROGRESS_EVERY == 0 && i > 0 {
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(json!({
                    "type": "progress",
                    "processed": i,
                    "total": total_edges,
                    "phase": "lexical",
                }));
            }
        }
        let class = classify_edge_lexically(&edge.name, edge.relation_type.as_deref(), &idx);
        classifications.push((edge, class));
    }

    // Separate residuals for embedding fallback
    let mut residuals: Vec<usize> = Vec::new();
    for (i, (_, class)) in classifications.iter().enumerate() {
        if matches!(class, EdgeClass::Residual) {
            residuals.push(i);
        }
    }

    // ── Phase C: embedding fallback for residual edges (async, no lock) ───────
    let mut embedding_fallback_promoted = 0usize;
    let mut embedding_skip_warning: Option<String> = None;

    // Pre-embed canonical type descriptions (one per type, cached for this call)
    let gloss_embeddings: Vec<(String, Vec<f32>)> = {
        let mut result = Vec::new();
        let types_with_desc: Vec<(String, String)> = ontology
            .extractable_relation_types()
            .filter_map(|rt| {
                rt.description
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .map(|d| (rt.name.clone(), d.to_string()))
            })
            .collect();

        if types_with_desc.is_empty() {
            embedding_skip_warning = Some(
                "embedding fallback skipped: no canonical types have description glosses"
                    .to_string(),
            );
        } else {
            for (name, desc) in &types_with_desc {
                match state.embedder.embed(desc, EmbedRole::Document).await {
                    Ok(emb) => result.push((name.clone(), emb)),
                    Err(e) => {
                        embedding_skip_warning =
                            Some(format!("embedding fallback skipped: embedder error: {e}"));
                        result.clear();
                        break;
                    }
                }
            }
        }
        result
    };

    if !gloss_embeddings.is_empty() && !residuals.is_empty() {
        let total_residuals = residuals.len();
        for (ri, &idx_in_classifications) in residuals.iter().enumerate() {
            if ri % PROGRESS_EVERY == 0 && ri > 0 {
                if let Some(ref tx) = progress_tx {
                    let _ = tx.send(json!({
                        "type": "progress",
                        "processed": ri,
                        "total": total_residuals,
                        "phase": "embedding_fallback",
                    }));
                }
            }
            let fact = classifications[idx_in_classifications].0.fact.clone();
            if fact.is_empty() {
                continue;
            }
            match state.embedder.embed(&fact, EmbedRole::Document).await {
                Ok(fact_emb) => {
                    let best = gloss_embeddings
                        .iter()
                        .map(|(name, emb)| (name, cosine_similarity(&fact_emb, emb)))
                        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                    if let Some((canonical, sim)) = best {
                        if sim >= threshold {
                            classifications[idx_in_classifications].1 =
                                EdgeClass::Mapped(canonical.clone());
                            embedding_fallback_promoted += 1;
                        }
                    }
                }
                Err(_) => {
                    // Non-fatal: leave as Residual if embed fails for this edge
                }
            }
        }
    }

    // ── Count classes ─────────────────────────────────────────────────────────
    let mut mapped_count = 0usize;
    let mut noise_count = 0usize;
    let mut residual_count = 0usize;
    for (_, class) in &classifications {
        match class {
            EdgeClass::Mapped(_) | EdgeClass::MappedInverse(_) => mapped_count += 1,
            EdgeClass::Noise => noise_count += 1,
            EdgeClass::Residual => residual_count += 1,
        }
    }

    // ── Phase B2: direction plan (#816) ───────────────────────────────────────
    // One write per edge at most, parallel to `classifications`. An edge's final relation type is
    // its mapped canonical type, or (noise/residual) the type it already has.
    let mut direction_writes: Vec<Option<DirectionWrite>> =
        Vec::with_capacity(classifications.len());
    let mut by_type: BTreeMap<String, TypeDirectionCounts> = BTreeMap::new();
    let mut swapped_count = 0usize;
    let mut signature_mismatch_count = 0usize;
    for (edge, class) in &classifications {
        let planned = endpoints.get(&edge.uuid).and_then(|info| {
            let (final_rt, inverse) = match class {
                EdgeClass::Mapped(c) => (c.as_str(), false),
                // The inverse reading is consumed by the retype: once the edge carries the
                // canonical type, a name still spelling the inverse alias must not swap it
                // again (idempotency).
                EdgeClass::MappedInverse(c) => (
                    c.as_str(),
                    edge.relation_type.as_deref() != Some(c.as_str()),
                ),
                EdgeClass::Noise | EdgeClass::Residual => {
                    (edge.relation_type.as_deref().unwrap_or(""), false)
                }
            };
            if final_rt.is_empty() {
                return None;
            }
            Some(plan_direction(
                &ontology, final_rt, inverse, &edge.uuid, info,
            ))
        });
        match planned {
            Some((write, counts)) => {
                if let Some((rt, checked, swapped, mismatched)) = counts {
                    let c = by_type.entry(rt).or_default();
                    c.checked += checked;
                    c.swapped += swapped;
                    c.mismatched += mismatched;
                    swapped_count += swapped;
                    signature_mismatch_count += mismatched;
                }
                direction_writes.push(write);
            }
            None => direction_writes.push(None),
        }
    }

    let report = CanonicalizeReport {
        group_id: params.group_id.clone(),
        total_edges,
        mapped_count,
        noise_count,
        residual_count,
        embedding_fallback_promoted,
        swapped_count,
        signature_mismatch_count,
        by_type,
        dry_run: params.dry_run,
    };

    // Dry-run: return coverage report without mutations
    if params.dry_run {
        return Ok(report.to_json());
    }

    // ── Phase D: batched write lock — apply mutations in chunks of WRITE_BATCH_SIZE ──
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(json!({
            "type": "progress",
            "phase": "writing",
            "total_mutations": mapped_count + noise_count + residual_count,
        }));
    }

    for (batch, batch_dir) in classifications
        .chunks(WRITE_BATCH_SIZE)
        .zip(direction_writes.chunks(WRITE_BATCH_SIZE))
    {
        let batch_data: Vec<(String, Option<String>, EdgeClass)> = batch
            .iter()
            .map(|(edge, class)| (edge.uuid.clone(), edge.relation_type.clone(), class.clone()))
            .collect();
        let batch_dir: Vec<Option<DirectionWrite>> = batch_dir.to_vec();

        let db_d = Arc::clone(&db);
        let state_c = Arc::clone(&state);
        let gid_c = params.group_id.clone();
        let _write_guard = state.write_lock.write().await;
        tokio::task::spawn_blocking(move || -> Result<(), Error> {
            let conn = db_d.connect().map_err(|e| Error::Ipc(format!("db: {e}")))?;
            let mut exec_result: Result<(), Error> = Ok(());
            for ((uuid, current_rt, class), dir_write) in batch_data.iter().zip(batch_dir.iter()) {
                let res = match class {
                    EdgeClass::Mapped(canonical) | EdgeClass::MappedInverse(canonical) => {
                        // Idempotency: skip if already set to the target value
                        if current_rt.as_deref() == Some(canonical.as_str()) {
                            Ok(())
                        } else {
                            conn.exec_params(
                                "MATCH (n:RelatesToNode_ {uuid: $uuid}) SET n.relation_type = $rt",
                                json!({ "uuid": uuid, "rt": canonical }),
                            )
                        }
                    }
                    EdgeClass::Residual => {
                        // Idempotency: skip if already UNCLASSIFIED
                        if current_rt.as_deref() == Some("UNCLASSIFIED") {
                            Ok(())
                        } else {
                            conn.exec_params(
                                "MATCH (n:RelatesToNode_ {uuid: $uuid}) SET n.relation_type = $rt",
                                json!({ "uuid": uuid, "rt": "UNCLASSIFIED" }),
                            )
                        }
                    }
                    EdgeClass::Noise => {
                        // ADR-0033: never delete noise edges. Only set UNCLASSIFIED when the
                        // edge has no relation_type. If a non-empty value already exists, preserve
                        // it — name-based noise classification bypasses relation_type mapping, so
                        // rich existing predicates on arrow-named edges must not be overwritten.
                        let rt_val = current_rt.as_deref().unwrap_or("").trim();
                        if !rt_val.is_empty() {
                            Ok(())
                        } else {
                            conn.exec_params(
                                "MATCH (n:RelatesToNode_ {uuid: $uuid}) SET n.relation_type = $rt",
                                json!({ "uuid": uuid, "rt": "UNCLASSIFIED" }),
                            )
                        }
                    }
                };
                if res.is_err() {
                    exec_result = res;
                    break;
                }
                // Direction write (#816): after the retype, so a retyped-and-reversed edge
                // converges in one pass. Idempotent on re-run: a swapped edge now fits.
                if let Some(w) = dir_write {
                    let res = match &w.swap_to {
                        Some((new_src, new_dst)) => conn.swap_relates_to_endpoints(
                            &w.rn_uuid,
                            new_src,
                            new_dst,
                            w.new_name.as_deref(),
                            w.new_attributes.as_deref(),
                        ),
                        None => match &w.new_attributes {
                            Some(attrs) => conn.set_relates_to_signature_mark(&w.rn_uuid, attrs),
                            None => Ok(()),
                        },
                    };
                    if res.is_err() {
                        exec_result = res;
                        break;
                    }
                }
            }
            // Always flush whatever succeeded before any failure — the DB already committed
            // those mutations; skipping the WAL flush would leave DB/WAL out of sync.
            // Candidate selection and mutation are both scoped to group_id (#447): a single call
            // touches exactly one group's data and routes to that group's own WAL stream.
            let seq = wal_exec::wal_flush_ungrouped(&state_c, &gid_c, conn.drain_mutations());
            wal_exec::advance_wal_position(&conn, &gid_c, seq, &state_c);
            exec_result
        })
        .await??;
        drop(_write_guard);
    }

    let mut resp = report.to_json();
    if let Some(warning) = embedding_skip_warning {
        resp["embedding_warning"] = json!(warning);
    }
    Ok(resp)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ontology::{EntityTypeDef, Ontology, OntologyMode, RelationTypeDef};

    fn make_ontology_with_rules() -> Ontology {
        Ontology {
            mode: OntologyMode::Open,
            entity_types: vec![EntityTypeDef {
                name: "Person".to_string(),
                description: None,
                parent: None,
                identity: false,
                extract: true,
            }],
            ancestor_map: std::collections::HashMap::new(),
            relation_types: vec![
                RelationTypeDef {
                    name: "AUTHORED".to_string(),
                    description: Some("a person authored something".to_string()),
                    source_types: vec![],
                    target_types: vec![],
                    inverse_aliases: vec![],
                    symmetric: false,
                    aliases: vec!["WROTE".to_string(), "AUTHORED_BY".to_string()],
                    keywords: vec!["author".to_string(), "writ".to_string()],
                    extract: true,
                },
                RelationTypeDef {
                    name: "AFFILIATED_WITH".to_string(),
                    description: None,
                    source_types: vec![],
                    target_types: vec![],
                    inverse_aliases: vec![],
                    symmetric: false,
                    aliases: vec!["WORKS_FOR".to_string()],
                    keywords: vec!["affiliat".to_string(), "employ".to_string()],
                    extract: true,
                },
            ],
        }
    }

    // ── Noise regex tests ─────────────────────────────────────────────────────

    #[test]
    fn test_noise_regex_precision() {
        // Valid noise patterns
        assert!(is_noise_edge("BRETT → RAJI"));
        assert!(is_noise_edge("BRETT -> RAJI"));
        assert!(is_noise_edge("ALICE SMITH → BOB JONES"));
        assert!(is_noise_edge("A → B"));
        assert!(is_noise_edge("A1 → B2"));
        assert!(is_noise_edge("FOO → BAR"));

        // Must NOT match
        assert!(!is_noise_edge("wrote"));
        assert!(!is_noise_edge("authored by"));
        assert!(!is_noise_edge("brett → raji")); // lowercase
        assert!(!is_noise_edge("Brett → Raji")); // mixed case (lowercase second char)
        assert!(!is_noise_edge("AFFILIATED_WITH"));
        assert!(!is_noise_edge("AUTHORED"));
        assert!(!is_noise_edge(""));
        assert!(!is_noise_edge("A → b")); // right side lowercase start
        assert!(!is_noise_edge("a → B")); // left side lowercase start
    }

    // ── LexicalIndex alias match ──────────────────────────────────────────────

    #[test]
    fn test_lexical_index_alias_match() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        // Alias "WROTE" maps to AUTHORED
        let class = classify_edge_lexically("WROTE", None, &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AUTHORED"),
            _ => panic!("expected Mapped(AUTHORED), got {class:?}"),
        }

        // Alias "AUTHORED_BY" maps to AUTHORED
        let class = classify_edge_lexically("AUTHORED_BY", None, &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AUTHORED"),
            _ => panic!("expected Mapped(AUTHORED)"),
        }

        // Alias "WORKS_FOR" maps to AFFILIATED_WITH
        let class = classify_edge_lexically("WORKS_FOR", None, &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AFFILIATED_WITH"),
            _ => panic!("expected Mapped(AFFILIATED_WITH)"),
        }
    }

    // ── LexicalIndex keyword match ────────────────────────────────────────────

    #[test]
    fn test_lexical_index_keyword_match() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        // "authoring" normalizes to "AUTHORING", lowercase "authoring" contains "author"
        let class = classify_edge_lexically("authoring", None, &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AUTHORED"),
            _ => panic!("expected Mapped(AUTHORED), got {class:?}"),
        }

        // "was_employed_by" contains "employ" keyword
        let class = classify_edge_lexically("was_employed_by", None, &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AFFILIATED_WITH"),
            _ => panic!("expected Mapped(AFFILIATED_WITH)"),
        }
    }

    #[test]
    fn test_classify_asserted_extract_false_edge_not_remapped() {
        let mut ontology = make_ontology_with_rules();
        ontology.relation_types.push(RelationTypeDef {
            name: "DERIVED_FROM".to_string(),
            description: None,
            source_types: vec![],
            target_types: vec![],
            inverse_aliases: vec![],
            symmetric: false,
            aliases: vec![],
            keywords: vec![],
            extract: false,
        });
        let idx = build_lexical_index(&ontology);

        // The name carries an extractable keyword ("author"), but the edge was asserted as
        // DERIVED_FROM and must stay so.
        let class = classify_edge_lexically("authored from", Some("DERIVED_FROM"), &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "DERIVED_FROM"),
            _ => panic!("expected Mapped(DERIVED_FROM), got {class:?}"),
        }
    }

    // ── Already canonical edge — idempotent ───────────────────────────────────

    #[test]
    fn test_classify_already_canonical_idempotent() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        // Edge has name="unknown_thing" but relation_type="AUTHORED" (set by post-#94 ingestion)
        let class = classify_edge_lexically("unknown_thing", Some("AUTHORED"), &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AUTHORED"),
            _ => panic!("expected Mapped(AUTHORED) for already-canonical relation_type"),
        }
    }

    // ── Residual edge ─────────────────────────────────────────────────────────

    #[test]
    fn test_classify_residual() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        let class = classify_edge_lexically("IS_THE_THIRD_COUSIN_TWICE_REMOVED_OF", None, &idx);
        assert!(matches!(class, EdgeClass::Residual));
    }

    // ── Noise class ───────────────────────────────────────────────────────────

    #[test]
    fn test_classify_noise() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        let class = classify_edge_lexically("BRETT → RAJI", None, &idx);
        assert!(matches!(class, EdgeClass::Noise));

        let class = classify_edge_lexically("FOO -> BAR", None, &idx);
        assert!(matches!(class, EdgeClass::Noise));
    }

    // ── Canonical name maps to itself ─────────────────────────────────────────

    #[test]
    fn test_canonical_name_maps_to_itself() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        let class = classify_edge_lexically("AUTHORED", None, &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AUTHORED"),
            _ => panic!("expected Mapped(AUTHORED)"),
        }
    }

    // ── relation_type alias/keyword as secondary signal (FR-002) ─────────────

    #[test]
    fn test_classify_via_relation_type_alias() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        // name has no signal; relation_type is an alias → should be Mapped via secondary path
        let class = classify_edge_lexically("SOME_UNRECOGNIZED_PREDICATE", Some("WROTE"), &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AUTHORED"),
            _ => panic!("expected Mapped(AUTHORED) via relation_type alias, got {class:?}"),
        }
    }

    #[test]
    fn test_classify_via_relation_type_keyword() {
        let ontology = make_ontology_with_rules();
        let idx = build_lexical_index(&ontology);

        // name has no signal; relation_type contains a keyword when normalized/lowercased
        let class =
            classify_edge_lexically("SOME_UNRECOGNIZED_PREDICATE", Some("WAS_EMPLOYED_BY"), &idx);
        match class {
            EdgeClass::Mapped(t) => assert_eq!(t, "AFFILIATED_WITH"),
            _ => {
                panic!("expected Mapped(AFFILIATED_WITH) via relation_type keyword, got {class:?}")
            }
        }
    }
    // ── #816: direction ──────────────────────────────────────────────────────

    fn direction_ontology() -> Ontology {
        let mut o = make_ontology_with_rules();
        o.mode = OntologyMode::Strict;
        o.relation_types[0].inverse_aliases = vec!["HAS_AUTHOR".to_string()];
        o
    }

    #[test]
    fn directed_alias_map_marks_inverse_and_forward_wins_collisions() {
        let mut o = direction_ontology();
        // Collides with the forward alias WROTE of the same type: forward reading wins.
        o.relation_types[0]
            .inverse_aliases
            .push("WROTE".to_string());
        let m = build_alias_map_directed(&o);
        assert_eq!(m["HAS_AUTHOR"], ("AUTHORED".to_string(), true));
        assert_eq!(m["WROTE"], ("AUTHORED".to_string(), false));
        assert_eq!(m["AUTHORED"], ("AUTHORED".to_string(), false));
        // The undirected projection is unchanged shape.
        assert_eq!(build_alias_map(&o)["HAS_AUTHOR"], "AUTHORED");
    }

    #[test]
    fn classify_inverse_alias_and_inverse_keyword() {
        let idx = build_lexical_index(&direction_ontology());
        match classify_edge_lexically("HAS_AUTHOR", None, &idx) {
            EdgeClass::MappedInverse(t) => assert_eq!(t, "AUTHORED"),
            other => panic!("expected MappedInverse, got {other:?}"),
        }
        // Forward alias unchanged.
        assert!(matches!(
            classify_edge_lexically("WROTE", None, &idx),
            EdgeClass::Mapped(_)
        ));
        // Keyword hit ("author") on text that also spells an inverse alias reads inversely...
        assert!(matches!(
            classify_edge_lexically("x", Some("HAS_AUTHOR_OF_RECORD_HAS_AUTHOR"), &idx),
            EdgeClass::MappedInverse(_)
        ));
        // ...a plain keyword hit does not.
        assert!(matches!(
            classify_edge_lexically("co-authored", None, &idx),
            EdgeClass::Mapped(_)
        ));
    }

    fn info(src: &[&str], dst: &[&str], name: &str, attrs: &str) -> EndpointInfo {
        EndpointInfo {
            src_uuid: "S".into(),
            src_name: "Sable".into(),
            src_labels: src.iter().map(|s| s.to_string()).collect(),
            dst_uuid: "D".into(),
            dst_name: "Lena".into(),
            dst_labels: dst.iter().map(|s| s.to_string()).collect(),
            name: name.into(),
            attributes: attrs.into(),
        }
    }

    fn signature_ontology() -> Ontology {
        let mut o = make_ontology_with_rules();
        o.entity_types.push(EntityTypeDef {
            name: "Project".to_string(),
            description: None,
            parent: None,
            identity: false,
            extract: true,
        });
        o.ancestor_map = crate::ontology::compute_ancestor_map(&o.entity_types);
        o.relation_types[0].source_types = vec!["Person".to_string()];
        o
    }

    #[test]
    fn plan_direction_swap_rewrites_ingest_form_name_only() {
        let o = signature_ontology();
        let reversed = info(
            &["Entity", "Project"],
            &["Entity", "Person"],
            "Sable → Lena",
            "{}",
        );
        let (w, counts) = plan_direction(&o, "AUTHORED", false, "rn", &reversed);
        let w = w.unwrap();
        assert_eq!(w.swap_to, Some(("D".to_string(), "S".to_string())));
        assert_eq!(w.new_name.as_deref(), Some("Lena → Sable"));
        assert_eq!(counts, Some(("AUTHORED".to_string(), 1, 1, 0)));

        let custom = info(
            &["Entity", "Project"],
            &["Entity", "Person"],
            "owned by",
            "{}",
        );
        let (w, _) = plan_direction(&o, "AUTHORED", false, "rn", &custom);
        assert_eq!(
            w.unwrap().new_name,
            None,
            "non-ingest-form names are left alone"
        );
    }

    #[test]
    fn plan_direction_mark_set_cleared_and_idempotent() {
        let o = signature_ontology();
        // Neither way: mark.
        let bad = info(
            &["Entity", "Project"],
            &["Entity", "Project"],
            "x",
            "{\"k\":1}",
        );
        let (w, counts) = plan_direction(&o, "AUTHORED", false, "rn", &bad);
        let attrs: Value = serde_json::from_str(&w.unwrap().new_attributes.unwrap()).unwrap();
        assert_eq!(attrs["signature_mismatch"], true);
        assert_eq!(attrs["k"], 1);
        assert_eq!(counts, Some(("AUTHORED".to_string(), 1, 0, 1)));
        // Already marked and still mismatched: no write.
        let marked = info(
            &["Entity", "Project"],
            &["Entity", "Project"],
            "x",
            "{\"signature_mismatch\":true}",
        );
        assert!(plan_direction(&o, "AUTHORED", false, "rn", &marked)
            .0
            .is_none());
        // Now fits: mark cleared.
        let fits = info(
            &["Entity", "Person"],
            &["Entity", "Project"],
            "x",
            "{\"signature_mismatch\":true,\"k\":1}",
        );
        let attrs: Value = serde_json::from_str(
            &plan_direction(&o, "AUTHORED", false, "rn", &fits)
                .0
                .unwrap()
                .new_attributes
                .unwrap(),
        )
        .unwrap();
        assert!(attrs.get("signature_mismatch").is_none());
        assert_eq!(attrs["k"], 1);
        // Untyped endpoint: skipped, an existing mark is left alone.
        let untyped = info(
            &["Entity"],
            &["Entity", "Project"],
            "x",
            "{\"signature_mismatch\":true}",
        );
        assert!(plan_direction(&o, "AUTHORED", false, "rn", &untyped)
            .0
            .is_none());
    }

    #[test]
    fn plan_direction_inverse_forces_swap_even_without_signature() {
        let o = direction_ontology(); // no signatures at all
        let i = info(&["Entity"], &["Entity"], "HAS_AUTHOR", "{}");
        let (w, counts) = plan_direction(&o, "AUTHORED", true, "rn", &i);
        assert_eq!(w.unwrap().swap_to, Some(("D".to_string(), "S".to_string())));
        assert_eq!(counts, Some(("AUTHORED".to_string(), 0, 1, 0)));
        // Not forced: nothing to do.
        assert!(plan_direction(&o, "AUTHORED", false, "rn", &i).0.is_none());
    }

    #[test]
    fn plan_direction_forced_swap_that_also_fits_is_not_swapped_back() {
        let o = signature_ontology(); // AUTHORED: Person → any
                                      // Inverse reading: extracted Project → Person; forcing the swap gives Person → Project.
        let i = info(
            &["Entity", "Project"],
            &["Entity", "Person"],
            "HAS_AUTHOR",
            "{}",
        );
        let (w, counts) = plan_direction(&o, "AUTHORED", true, "rn", &i);
        assert_eq!(w.unwrap().swap_to, Some(("D".to_string(), "S".to_string())));
        assert_eq!(counts, Some(("AUTHORED".to_string(), 1, 1, 0)));
        // Inverse reading but extracted already Person → Project: forced swap makes it
        // Project → Person, which fails Person → any, so the signature swap undoes it.
        let j = info(
            &["Entity", "Person"],
            &["Entity", "Project"],
            "HAS_AUTHOR",
            "{}",
        );
        assert!(plan_direction(&o, "AUTHORED", true, "rn", &j).0.is_none());
    }

    /// Structural integrity of a swap on a real DB: still exactly one source hop, one target hop
    /// and one direct rel, pointing the new way.
    #[test]
    fn swap_relates_to_endpoints_rewires_all_three_structures() {
        use crate::{EntityRow, RelatesToEdge};
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::db::Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap();
        let conn = db.connect().unwrap();
        conn.init_schema(4).unwrap();
        let mk = |n: &str| EntityRow {
            uuid: format!("u-{n}"),
            name: n.to_string(),
            group_id: "g".to_string(),
            labels: vec!["Entity".to_string()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            name_embedding: vec![1.0, 0.0, 0.0, 0.0],
            summary: n.to_string(),
            attributes: "{}".to_string(),
            ..Default::default()
        };
        conn.insert_entity(&mk("a")).unwrap();
        conn.insert_entity(&mk("b")).unwrap();
        conn.insert_relates_to_edge(&RelatesToEdge {
            uuid: "rn1".to_string(),
            name: "a → b".to_string(),
            source_node_uuid: "u-a".to_string(),
            target_node_uuid: "u-b".to_string(),
            group_id: "g".to_string(),
            fact: "a rel b".to_string(),
            fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            attributes: "{}".to_string(),
            relation_type: Some("REL".to_string()),
            ..Default::default()
        })
        .unwrap();

        conn.swap_relates_to_endpoints(
            "rn1",
            "u-b",
            "u-a",
            Some("b → a"),
            Some("{\"signature_mismatch\":true}"),
        )
        .unwrap();

        let count = |cypher: &str| -> usize {
            conn.query_params(cypher, serde_json::json!({}))
                .unwrap()
                .len()
        };
        assert_eq!(
            count("MATCH (s:Entity {uuid:'u-b'})-[:RELATES_TO]->(rn:RelatesToNode_ {uuid:'rn1'}) RETURN rn.uuid"),
            1
        );
        assert_eq!(
            count("MATCH (rn:RelatesToNode_ {uuid:'rn1'})-[:RELATES_TO]->(d:Entity {uuid:'u-a'}) RETURN rn.uuid"),
            1
        );
        assert_eq!(
            count("MATCH (s:Entity)-[:RELATES_TO]->(rn:RelatesToNode_ {uuid:'rn1'}) RETURN s.uuid"),
            1,
            "exactly one source hop"
        );
        assert_eq!(
            count("MATCH (rn:RelatesToNode_ {uuid:'rn1'})-[:RELATES_TO]->(d:Entity) RETURN d.uuid"),
            1,
            "exactly one target hop"
        );
        assert_eq!(
            count("MATCH (s:Entity {uuid:'u-b'})-[r:RELATES_TO {uuid:'rn1'}]->(d:Entity {uuid:'u-a'}) RETURN r.uuid"),
            1,
            "direct rel points the new way"
        );
        assert_eq!(
            count("MATCH (s:Entity)-[r:RELATES_TO {uuid:'rn1'}]->(d:Entity) RETURN r.uuid"),
            1,
            "exactly one direct rel"
        );
        let rows = conn
            .query_params(
                "MATCH (rn:RelatesToNode_ {uuid:'rn1'}) RETURN rn.name, rn.attributes",
                serde_json::json!({}),
            )
            .unwrap();
        assert_eq!(
            crate::db::value_as_string(&rows[0][1]),
            "{\"signature_mismatch\":true}"
        );
        assert_eq!(crate::db::value_as_string(&rows[0][0]), "b → a");
    }
}
