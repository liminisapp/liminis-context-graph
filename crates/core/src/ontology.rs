use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::Error;

// ── Serde deserialization types (YAML schema) ─────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
struct OntologyFile {
    #[serde(default)]
    mode: Option<OntologyModeRaw>,
    #[serde(default)]
    entity_types: Vec<EntityTypeRaw>,
    #[serde(default)]
    relation_types: Vec<RelationTypeRaw>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OntologyModeRaw {
    Open,
    Strict,
}

#[derive(Debug, Clone, Deserialize)]
struct EntityTypeRaw {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    /// Identity-bearing flag (#616): extracted entities of this type get it as their `kind`.
    #[serde(default)]
    identity: bool,
    /// `extract: false` (#637): assert-only type — never offered to or accepted from the LLM
    /// extractor. Defaults to `true`.
    #[serde(default = "default_true")]
    extract: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RelationTypeRaw {
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// Source entity type(s): a single name or a list (#816). Unset accepts any type.
    #[serde(default)]
    source_type: Option<StringOrList>,
    /// Target entity type(s): a single name or a list (#816). Unset accepts any type.
    #[serde(default)]
    target_type: Option<StringOrList>,
    /// Exact SCREAMING_SNAKE_CASE name variants that map to this canonical type.
    #[serde(default)]
    aliases: Option<Vec<String>>,
    /// Names that read the relation backwards (#816): they resolve to this canonical type with
    /// source and target swapped.
    #[serde(default)]
    inverse_aliases: Option<Vec<String>>,
    /// Endpoint order carries no meaning (#816): never checked or swapped.
    #[serde(default)]
    symmetric: bool,
    /// Lowercase substring keywords — if any keyword appears in the normalized name, maps here.
    #[serde(default)]
    keywords: Option<Vec<String>>,
    /// `extract: false` (#637): assert-only relation type. Defaults to `true`.
    #[serde(default = "default_true")]
    extract: bool,
}

fn default_true() -> bool {
    true
}

/// A YAML value that is either one string or a list of strings (#816).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn into_vec(self) -> Vec<String> {
        match self {
            StringOrList::One(s) => vec![s],
            StringOrList::Many(v) => v,
        }
    }
}

// ── Runtime types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OntologyMode {
    Open,
    Strict,
}

impl std::fmt::Display for OntologyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OntologyMode::Open => write!(f, "open"),
            OntologyMode::Strict => write!(f, "strict"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EntityTypeDef {
    pub name: String,
    pub description: Option<String>,
    pub parent: Option<String>,
    /// Identity-bearing (#616): an entity extracted with this as its primary type is created
    /// with `kind` = this type's name, so it stays distinct from same-named entities of other
    /// kinds. Never inherited through `parent`. Defaults to `false`.
    pub identity: bool,
    /// Extractable (#637): when `false` the type is assert-only — omitted from the extraction
    /// prompts and never accepted from the extractor, but otherwise fully declared (assert,
    /// identity, ancestry, drift). Not inherited through `parent`. Defaults to `true`.
    pub extract: bool,
}

#[derive(Debug, Clone)]
pub struct RelationTypeDef {
    pub name: String,
    pub description: Option<String>,
    /// Declared source entity types, normalized, as declared (#816). Empty = unset (any type).
    /// May name types the ontology does not declare — use
    /// [`Ontology::effective_source_types`] for the usable set.
    pub source_types: Vec<String>,
    /// Declared target entity types (#816); see `source_types`.
    pub target_types: Vec<String>,
    /// Exact SCREAMING_SNAKE_CASE name variants that map to this canonical type.
    pub aliases: Vec<String>,
    /// Names that read this relation backwards (#816): resolve to this canonical type with
    /// source and target swapped.
    pub inverse_aliases: Vec<String>,
    /// Endpoint order carries no meaning (#816): never checked or swapped.
    pub symmetric: bool,
    /// Lowercase substring keywords for fuzzy name matching.
    pub keywords: Vec<String>,
    /// Extractable (#637): `false` makes the relation type assert-only. Defaults to `true`.
    pub extract: bool,
}

#[derive(Debug, Clone)]
pub struct Ontology {
    pub mode: OntologyMode,
    pub entity_types: Vec<EntityTypeDef>,
    pub relation_types: Vec<RelationTypeDef>,
    /// Maps each entity type name to its ordered ancestor list (supertype-first, excluding
    /// the implicit `Entity` root). Types with no declared parent map to an empty vec.
    /// Precomputed at load time for O(1) lookup during bulk label stamping.
    pub ancestor_map: HashMap<String, Vec<String>>,
}

impl Ontology {
    /// Declared source types of `rt` that name a declared entity type (#816). Undeclared names
    /// are ignored; when none remain the side behaves as unset (empty = accepts any type).
    pub fn effective_source_types<'a>(&self, rt: &'a RelationTypeDef) -> Vec<&'a str> {
        self.effective_sides(&rt.source_types)
    }

    /// Declared target types of `rt` that name a declared entity type (#816).
    pub fn effective_target_types<'a>(&self, rt: &'a RelationTypeDef) -> Vec<&'a str> {
        self.effective_sides(&rt.target_types)
    }

    fn effective_sides<'a>(&self, declared: &'a [String]) -> Vec<&'a str> {
        declared
            .iter()
            .filter(|t| self.ancestor_map.contains_key(t.as_str()))
            .map(|t| t.as_str())
            .collect()
    }

    /// True when any relation declares a usable signature (#816) or an inverse alias, i.e. when
    /// the direction check has anything to do.
    pub fn has_direction_info(&self) -> bool {
        self.relation_types.iter().any(|r| {
            !r.inverse_aliases.is_empty()
                || (!r.symmetric
                    && (!self.effective_source_types(r).is_empty()
                        || !self.effective_target_types(r).is_empty()))
        })
    }

    pub fn has_entity_types(&self) -> bool {
        !self.entity_types.is_empty()
    }

    pub fn has_relation_types(&self) -> bool {
        !self.relation_types.is_empty()
    }

    pub fn entity_type_names(&self) -> HashSet<String> {
        self.entity_types.iter().map(|e| e.name.clone()).collect()
    }

    pub fn relation_type_names(&self) -> HashSet<String> {
        self.relation_types.iter().map(|r| r.name.clone()).collect()
    }

    /// Entity types offered to the LLM extractor (#637): every declared type with
    /// `extract != false`. `entity_type_names()` remains the full declared set.
    pub fn extractable_entity_types(&self) -> impl Iterator<Item = &EntityTypeDef> {
        self.entity_types.iter().filter(|e| e.extract)
    }

    /// Relation types offered to the LLM extractor (#637).
    pub fn extractable_relation_types(&self) -> impl Iterator<Item = &RelationTypeDef> {
        self.relation_types.iter().filter(|r| r.extract)
    }

    pub fn extractable_entity_type_names(&self) -> HashSet<String> {
        self.extractable_entity_types()
            .map(|e| e.name.clone())
            .collect()
    }

    pub fn extractable_relation_type_names(&self) -> HashSet<String> {
        self.extractable_relation_types()
            .map(|r| r.name.clone())
            .collect()
    }

    /// True when `label` (normalized first, since open-mode labels are raw) names a declared
    /// entity type flagged `extract: false` (#637).
    pub fn is_extract_false_entity(&self, label: &str) -> bool {
        let normalized = normalize_entity_type(label);
        self.entity_types
            .iter()
            .any(|e| !e.extract && e.name == normalized)
    }

    /// True when `label` (normalized first) names a declared relation type flagged
    /// `extract: false` (#637).
    pub fn is_extract_false_relation(&self, label: &str) -> bool {
        let normalized = normalize_relation_type(label);
        self.relation_types
            .iter()
            .any(|r| !r.extract && r.name == normalized)
    }

    /// The set of normalized identity-bearing entity type names (#616). Empty for any
    /// ontology that carries no `identity: true` flag.
    pub fn identity_set(&self) -> std::collections::BTreeSet<String> {
        self.entity_types
            .iter()
            .filter(|e| e.identity)
            .map(|e| e.name.clone())
            .collect()
    }

    /// Every declared type whose ancestor list contains `label` (#776): its strict descendants
    /// in the type hierarchy, sorted. Empty for a leaf or an unknown label.
    pub fn descendants_of(&self, label: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .ancestor_map
            .iter()
            .filter(|(name, ancestors)| {
                name.as_str() != label && ancestors.iter().any(|a| a == label)
            })
            .map(|(name, _)| name.clone())
            .collect();
        out.sort();
        out
    }

    /// Returns the kind an extracted entity of primary type `entity_type` receives, if that
    /// type is identity-bearing (#616, FR-002/FR-003). The lookup is by the normalized
    /// (PascalCase) name of the primary type only: parents never confer identity and no label
    /// or declaration order is consulted. `None` means the default kind applies.
    pub fn identity_kind(&self, entity_type: &str) -> Option<String> {
        let normalized = normalize_entity_type(entity_type);
        self.entity_types
            .iter()
            .find(|e| e.identity && e.extract && e.name == normalized)
            .map(|e| e.name.clone())
    }
}

// ── Name normalization ────────────────────────────────────────────────────────

/// Normalizes an entity type name to PascalCase.
/// e.g. "peer_reviewed_paper" → "PeerReviewedPaper", "PERSON" → "Person"
pub fn normalize_entity_type(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    // Split on word boundaries: underscores, hyphens, spaces, and case transitions
    let words = split_words(s);
    words
        .into_iter()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
            }
        })
        .collect()
}

/// Normalizes a relation type name to SCREAMING_SNAKE_CASE.
/// e.g. "authored by" → "AUTHORED_BY", "AffiliatedWith" → "AFFILIATED_WITH"
pub fn normalize_relation_type(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let words = split_words(s);
    words
        .into_iter()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Splits a string into words on underscores, hyphens, spaces, and CamelCase boundaries.
fn split_words(s: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();

    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    let mut i = 0;
    while i < len {
        let c = chars[i];
        if c == '_' || c == '-' || c == ' ' {
            if !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
            i += 1;
            continue;
        }
        // CamelCase boundary: uppercase after lowercase, or uppercase before lowercase with preceding uppercase
        if c.is_uppercase() && !current.is_empty() {
            let prev_is_lower = current
                .chars()
                .last()
                .map(|ch| ch.is_lowercase())
                .unwrap_or(false);
            let next_is_lower = i + 1 < len && chars[i + 1].is_lowercase();
            if prev_is_lower
                || (next_is_lower
                    && current
                        .chars()
                        .last()
                        .map(|ch| ch.is_uppercase())
                        .unwrap_or(false))
            {
                words.push(current.clone());
                current.clear();
            }
        }
        current.push(c);
        i += 1;
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

// ── File loading ──────────────────────────────────────────────────────────────

fn ontology_file_path(workspace_root: &Path) -> Option<PathBuf> {
    let primary = workspace_root.join(".lcg").join("ontology.yaml");
    if primary.exists() {
        return Some(primary);
    }
    let fallback = workspace_root.join(".graphiti").join("ontology.yaml");
    if fallback.exists() {
        return Some(fallback);
    }
    None
}

/// Loads and parses the workspace ontology file.
///
/// Returns `None` if:
/// - `workspace_root` is `None`
/// - no ontology file exists at the expected paths
/// - the file is empty or declares no entity types and no relation types
/// - the file is malformed (logged as a warning; does not panic)
pub fn load_ontology(workspace_root: Option<&Path>) -> Option<Ontology> {
    let root = workspace_root?;
    let path = ontology_file_path(root)?;

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "liminis-context-graph: ontology: failed to read {:?}: {} — falling back to free-form extraction",
                path, e
            );
            return None;
        }
    };

    // An empty or whitespace-only file is intentionally "no ontology" — don't log a parse error.
    if text.trim().is_empty() {
        return None;
    }

    let file: OntologyFile = match serde_yaml::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "liminis-context-graph: ontology: YAML parse error in {:?}: {} — falling back to free-form extraction",
                path, e
            );
            return None;
        }
    };

    let ontology = build_ontology(file, None)?;
    eprintln!(
        "liminis-context-graph: ontology: loaded {} entity type(s), {} relation type(s), mode={} from {:?}",
        ontology.entity_types.len(),
        ontology.relation_types.len(),
        ontology.mode,
        path
    );
    Some(ontology)
}

/// Resolves the path a per-group ontology file would live at for `group_id`, per FR-003/FR-004:
/// `{workspace_root}/.lcg/ontology/<encoded_group_id>.yaml`. Does not check whether the file
/// exists — callers use this both to probe for the file and to know where to look.
///
/// `group_id` is encoded with [`crate::wal_group::encode_group_dir_name`], the same bijective
/// percent-encoding already used for per-group WAL directory names (FR-004), so a `group_id`
/// unsafe as a filesystem path component gets identical treatment here as it does for WAL
/// storage. Returns `Err` only when `group_id` itself is invalid (empty, or too long once
/// encoded) — the same cases `encode_group_dir_name` rejects.
pub fn group_ontology_path(workspace_root: &Path, group_id: &str) -> Result<PathBuf, Error> {
    let encoded = crate::wal_group::encode_group_dir_name(group_id)?;
    Ok(workspace_root
        .join(".lcg")
        .join("ontology")
        .join(format!("{encoded}.yaml")))
}

/// Loads and parses a per-group ontology file for `group_id`, if one exists (FR-001).
///
/// Mirrors [`load_ontology`]'s silent-fallback behavior: returns `None` if the file doesn't
/// exist, is empty, is malformed (logged as a warning; does not panic), or declares no entity
/// types and no relation types. Callers are responsible for falling back to the workspace-wide
/// ontology when this returns `None` (FR-002) — this function only answers "does group_id have
/// its own ontology," not "what ontology should govern group_id."
pub fn load_group_ontology(workspace_root: &Path, group_id: &str) -> Option<Ontology> {
    let path = group_ontology_path(workspace_root, group_id).ok()?;
    if !path.exists() {
        return None;
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "liminis-context-graph: ontology: failed to read per-group ontology {:?} for group {:?}: {} — falling back to workspace ontology",
                path, group_id, e
            );
            return None;
        }
    };

    // An empty or whitespace-only file is intentionally "no per-group ontology" — don't log a
    // parse error, just fall back like a missing file would.
    if text.trim().is_empty() {
        return None;
    }

    let file: OntologyFile = match serde_yaml::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "liminis-context-graph: ontology: YAML parse error in per-group ontology {:?} for group {:?}: {} — falling back to workspace ontology",
                path, group_id, e
            );
            return None;
        }
    };

    let ontology = build_ontology(file, None)?;
    eprintln!(
        "liminis-context-graph: ontology: loaded {} entity type(s), {} relation type(s), mode={} for group {:?} from {:?}",
        ontology.entity_types.len(),
        ontology.relation_types.len(),
        ontology.mode,
        group_id,
        path
    );
    Some(ontology)
}

/// Loads and parses an ontology from a bare file path, independent of any workspace root.
///
/// Unlike [`load_ontology`], this is intended for tooling (e.g. `lcg-eval`) that needs to load
/// a standalone ontology fixture and must be told loudly, via `Err`, when the file is missing,
/// unreadable, malformed, or declares no entity types and no relation types — it never falls
/// back to "no ontology" silently.
///
/// `mode` is authoritative and always overrides any `mode:` key declared inside the file, so a
/// single fixture file can drive `Open` and `Strict` runs without being rewritten between them.
pub fn load_ontology_from_path(path: &Path, mode: OntologyMode) -> Result<Ontology, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read ontology file {:?}: {}", path, e))?;

    if text.trim().is_empty() {
        return Err(format!("ontology file {:?} is empty", path));
    }

    let file: OntologyFile = serde_yaml::from_str(&text)
        .map_err(|e| format!("YAML parse error in ontology file {:?}: {}", path, e))?;

    build_ontology(file, Some(mode)).ok_or_else(|| {
        format!(
            "ontology file {:?} declares no entity types and no relation types",
            path
        )
    })
}

/// Builds an [`Ontology`] from a parsed [`OntologyFile`]: resolves mode, normalizes and
/// validates entity/relation type names, cleans up parent references, and computes the
/// ancestor map. Returns `None` if the resulting ontology has no entity types and no relation
/// types (an empty ontology is treated as "no ontology" by both loading entry points).
///
/// `mode_override`, when given, always wins over the file's own declared `mode:` key.
fn build_ontology(file: OntologyFile, mode_override: Option<OntologyMode>) -> Option<Ontology> {
    let mode = match mode_override {
        Some(m) => m,
        None => match file.mode {
            Some(OntologyModeRaw::Strict) => OntologyMode::Strict,
            _ => OntologyMode::Open,
        },
    };

    let mut entity_types: Vec<EntityTypeDef> = file
        .entity_types
        .into_iter()
        .filter_map(|raw| {
            let normalized = normalize_entity_type(&raw.name);
            if normalized.is_empty() {
                eprintln!("liminis-context-graph: ontology: skipping entity type with blank name");
                return None;
            }
            if normalized == "Entity" {
                eprintln!(
                    "liminis-context-graph: ontology: 'Entity' is the implicit root type and should not be declared in entity_types — ignoring"
                );
                return None;
            }
            if normalized != raw.name {
                eprintln!(
                    "liminis-context-graph: ontology: entity type '{}' normalized to '{}'",
                    raw.name, normalized
                );
            }
            let parent = raw.parent.map(|p| normalize_entity_type(&p));
            // `Merged` is a reserved kind (it would masquerade as a merge tombstone, #615) and
            // `Unclassified` is the strict-mode catch-all: neither may carry identity.
            let identity = raw.identity && !matches!(normalized.as_str(), "Merged" | "Unclassified");
            if raw.identity && !identity {
                eprintln!(
                    "liminis-context-graph: ontology: entity type '{normalized}' is reserved and cannot be identity-bearing — ignoring `identity: true`"
                );
            }
            Some(EntityTypeDef {
                name: normalized,
                description: raw.description,
                parent,
                identity,
                extract: raw.extract,
            })
        })
        .collect();

    validate_and_clean_parents(&mut entity_types);
    let ancestor_map = compute_ancestor_map(&entity_types);

    let relation_types: Vec<RelationTypeDef> = file
        .relation_types
        .into_iter()
        .filter_map(|raw| {
            let normalized = normalize_relation_type(&raw.name);
            if normalized.is_empty() {
                eprintln!(
                    "liminis-context-graph: ontology: skipping relation type with blank name"
                );
                return None;
            }
            if normalized != raw.name {
                eprintln!(
                    "liminis-context-graph: ontology: relation type '{}' normalized to '{}'",
                    raw.name, normalized
                );
            }
            let normalize_side = |side: Option<StringOrList>| -> Vec<String> {
                let mut out: Vec<String> = Vec::new();
                for t in side.map(StringOrList::into_vec).unwrap_or_default() {
                    let n = normalize_entity_type(&t);
                    if !n.is_empty() && !out.contains(&n) {
                        out.push(n);
                    }
                }
                out
            };
            let source_types = normalize_side(raw.source_type);
            let target_types = normalize_side(raw.target_type);
            let inverse_aliases: Vec<String> = raw
                .inverse_aliases
                .unwrap_or_default()
                .into_iter()
                .map(|a| normalize_relation_type(&a))
                .filter(|a| !a.is_empty())
                .collect();
            let aliases: Vec<String> = raw
                .aliases
                .unwrap_or_default()
                .into_iter()
                .map(|a| normalize_relation_type(&a))
                .filter(|a| !a.is_empty())
                .collect();
            let keywords: Vec<String> = raw
                .keywords
                .unwrap_or_default()
                .into_iter()
                .map(|k| k.to_lowercase())
                .filter(|k| !k.is_empty())
                .collect();
            Some(RelationTypeDef {
                name: normalized,
                description: raw.description,
                source_types,
                target_types,
                aliases,
                inverse_aliases,
                symmetric: raw.symmetric,
                keywords,
                extract: raw.extract,
            })
        })
        .collect();

    // Coerce to None if both axes are empty — prevents injecting empty prompt sections
    if entity_types.is_empty() && relation_types.is_empty() {
        return None;
    }

    warn_direction_declarations(&entity_types, &relation_types);

    Some(Ontology {
        mode,
        entity_types,
        relation_types,
        ancestor_map,
    })
}

/// Load-time warnings for the direction fields (#816): signature types the ontology does not
/// declare (that side is then ignored by the endpoint check), and inverse aliases that collide
/// with another relation's name, alias or inverse alias. Never an error.
fn warn_direction_declarations(entity_types: &[EntityTypeDef], relation_types: &[RelationTypeDef]) {
    let declared: HashSet<&str> = entity_types.iter().map(|e| e.name.as_str()).collect();
    for r in relation_types {
        for (side, types) in [
            ("source_type", &r.source_types),
            ("target_type", &r.target_types),
        ] {
            for t in types {
                if !declared.contains(t.as_str()) {
                    eprintln!(
                        "liminis-context-graph: ontology: relation type '{}' {} names '{}' \
                         which is not a declared entity type — ignoring it in the direction check",
                        r.name, side, t
                    );
                }
            }
        }
    }
    // name / alias owners (first declaration wins for the message)
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for r in relation_types {
        owner.entry(r.name.as_str()).or_insert(r.name.as_str());
        for a in &r.aliases {
            owner.entry(a.as_str()).or_insert(r.name.as_str());
        }
    }
    let mut inverse_owner: HashMap<&str, &str> = HashMap::new();
    for r in relation_types {
        for ia in &r.inverse_aliases {
            if let Some(o) = owner.get(ia.as_str()) {
                eprintln!(
                    "liminis-context-graph: ontology: inverse alias '{}' of relation type '{}' \
                     collides with a name or alias of '{}' — the forward reading wins",
                    ia, r.name, o
                );
            } else if let Some(o) = inverse_owner.get(ia.as_str()) {
                if *o != r.name.as_str() {
                    eprintln!(
                        "liminis-context-graph: ontology: inverse alias '{}' is declared by both \
                         '{}' and '{}' — the first wins",
                        ia, o, r.name
                    );
                }
            } else {
                inverse_owner.insert(ia.as_str(), r.name.as_str());
            }
        }
    }
}

// ── Parent validation and ancestor map ───────────────────────────────────────

/// Validates and cleans parent relationships in the entity type list in place.
///
/// Phase 1: any `parent` that names a type not in the declared set is cleared with a warning.
/// Phase 2: cycles (A → B → A, A → B → C → A, etc.) are detected by path-following; all
/// members of a cycle have their `parent` cleared and a single warning is logged.
pub fn validate_and_clean_parents(entity_types: &mut [EntityTypeDef]) {
    let declared: HashSet<String> = entity_types.iter().map(|e| e.name.clone()).collect();

    // Phase 1: clear undeclared parents
    for et in entity_types.iter_mut() {
        if let Some(ref p) = et.parent {
            if !declared.contains(p.as_str()) {
                eprintln!(
                    "liminis-context-graph: ontology: entity type '{}' declares parent '{}' \
                     which is not in the declared type set — treating as no parent",
                    et.name, p
                );
                et.parent = None;
            }
        }
    }

    // Phase 2: detect cycles by following each node's parent chain.
    // Use String keys so the map does not borrow from entity_types, allowing mutation below.
    let index_of: HashMap<String, usize> = entity_types
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.clone(), i))
        .collect();

    // Track which nodes we've already processed (no cycle reachable from them)
    let mut safe: HashSet<usize> = HashSet::new();

    for start in 0..entity_types.len() {
        if safe.contains(&start) {
            continue;
        }

        // Walk the parent chain from `start`, collecting the path
        let mut path: Vec<usize> = Vec::new();
        let mut visited_in_path: HashMap<usize, usize> = HashMap::new(); // index → position in path

        let mut cur = start;
        loop {
            if safe.contains(&cur) {
                // Entire path is cycle-free
                safe.extend(path.iter().copied());
                break;
            }
            if let Some(&cycle_start_pos) = visited_in_path.get(&cur) {
                // Found a cycle: path[cycle_start_pos..] forms the cycle
                let cycle: Vec<usize> = path[cycle_start_pos..].to_vec();
                // Collect names as owned Strings before mutating the vec
                let names: Vec<String> = cycle
                    .iter()
                    .map(|&i| entity_types[i].name.clone())
                    .collect();
                eprintln!(
                    "liminis-context-graph: ontology: cycle detected in entity type parent graph: {} — \
                     clearing parents for all cycle members",
                    names.join(" → ")
                );
                for &i in &cycle {
                    entity_types[i].parent = None;
                }
                // Nodes before the cycle entry are safe (they lead into the cycle, which is now broken)
                for &i in &path[..cycle_start_pos] {
                    safe.insert(i);
                }
                break;
            }

            visited_in_path.insert(cur, path.len());
            path.push(cur);

            // Advance to parent; read parent as an owned String to avoid holding a borrow across mutation
            let next_idx = entity_types[cur]
                .parent
                .as_deref()
                .and_then(|p| index_of.get(p))
                .copied();
            match next_idx {
                Some(next) => cur = next,
                None => {
                    // Reached a root (no parent or parent already cleared)
                    safe.extend(path.iter().copied());
                    break;
                }
            }
        }
    }
}

/// Precomputes the ancestor map for the given (validated, cycle-free) entity type list.
///
/// Returns a `HashMap` from type name to ordered ancestor list (supertype-first, excluding
/// the implicit `Entity` root). Types with no parent map to an empty vec.
pub fn compute_ancestor_map(entity_types: &[EntityTypeDef]) -> HashMap<String, Vec<String>> {
    let parent_of: HashMap<&str, &str> = entity_types
        .iter()
        .filter_map(|e| e.parent.as_deref().map(|p| (e.name.as_str(), p)))
        .collect();

    let mut map = HashMap::new();
    for et in entity_types {
        let mut ancestors = Vec::new();
        let mut cur = et.name.as_str();
        while let Some(&p) = parent_of.get(cur) {
            ancestors.push(p.to_string());
            cur = p;
        }
        ancestors.reverse(); // supertype-first (root at index 0)
        map.insert(et.name.clone(), ancestors);
    }
    map
}

// ── Content hash ─────────────────────────────────────────────────────────────

/// Returns a stable semantic content hash of the given ontology.
///
/// - `None` returns the sentinel string `"none"`, representing "ingested with no ontology".
/// - Hash is based on the parsed struct, not raw YAML bytes — cosmetic edits (whitespace,
///   comments) produce the same hash.
/// - Canonical form: `"mode:{mode}\nentity_types:{entries}\nrelation_types:{entries}"` where
///   entries are sorted by name and formatted as `NAME\0DESCRIPTION` (entity) or
///   `NAME\0SOURCE\0TARGET\0DESCRIPTION` (relation), joined with `\0\0`.
pub fn content_hash(ontology: Option<&Ontology>) -> String {
    let Some(o) = ontology else {
        return "none".to_string();
    };

    let mut entity_entries: Vec<String> = o
        .entity_types
        .iter()
        .map(|e| format!("{}\0{}", e.name, e.description.as_deref().unwrap_or("")))
        .collect();
    entity_entries.sort_unstable();

    let mut relation_entries: Vec<String> = o
        .relation_types
        .iter()
        .map(|r| {
            let mut aliases = r.aliases.clone();
            aliases.sort_unstable();
            let mut keywords = r.keywords.clone();
            keywords.sort_unstable();
            format!(
                "{}\0{}\0{}\0{}\0{}\0{}",
                r.name,
                // A one-element list serializes as the legacy single string, so existing
                // ontologies keep their hash (#816).
                r.source_types.join("|"),
                r.target_types.join("|"),
                r.description.as_deref().unwrap_or(""),
                aliases.join("\0"),
                keywords.join("\0"),
            )
        })
        .collect();
    relation_entries.sort_unstable();

    // Collect parent edges and append them only when at least one parent is declared.
    // This preserves hash backward compatibility for flat ontologies (no spurious drift
    // on upgrade from pre-#173 versions). See ADR-0032.
    let mut parent_entries: Vec<String> = o
        .entity_types
        .iter()
        .filter_map(|e| e.parent.as_deref().map(|p| format!("{}\0{}", e.name, p)))
        .collect();
    parent_entries.sort_unstable();

    let mut canonical = format!(
        "mode:{}\nentity_types:{}\nrelation_types:{}",
        o.mode,
        entity_entries.join("\0\0"),
        relation_entries.join("\0\0"),
    );
    if !parent_entries.is_empty() {
        canonical.push_str(&format!("\nparent_edges:{}", parent_entries.join("\0\0")));
    }
    // Identity flags (#616) are appended only when at least one is set, so an ontology that
    // declares none hashes byte-identically to before the flag existed.
    let identity_entries: Vec<String> = o.identity_set().into_iter().collect();
    if !identity_entries.is_empty() {
        canonical.push_str(&format!("\nidentity:{}", identity_entries.join("\0\0")));
    }

    // `extract: false` flags (#637) follow the same append-only-when-present rule.
    let mut extract_false_entities: Vec<&str> = o
        .entity_types
        .iter()
        .filter(|e| !e.extract)
        .map(|e| e.name.as_str())
        .collect();
    extract_false_entities.sort_unstable();
    let mut extract_false_relations: Vec<&str> = o
        .relation_types
        .iter()
        .filter(|r| !r.extract)
        .map(|r| r.name.as_str())
        .collect();
    extract_false_relations.sort_unstable();
    if !extract_false_entities.is_empty() || !extract_false_relations.is_empty() {
        canonical.push_str(&format!(
            "\nextract_false:{}\nextract_false_relations:{}",
            extract_false_entities.join("\0\0"),
            extract_false_relations.join("\0\0"),
        ));
    }

    // Direction fields (#816) follow the same append-only-when-present rule (ADR-0032).
    let mut inverse_entries: Vec<String> = o
        .relation_types
        .iter()
        .filter(|r| !r.inverse_aliases.is_empty())
        .map(|r| {
            let mut ia = r.inverse_aliases.clone();
            ia.sort_unstable();
            format!("{}\0{}", r.name, ia.join("\0"))
        })
        .collect();
    inverse_entries.sort_unstable();
    let mut symmetric_entries: Vec<&str> = o
        .relation_types
        .iter()
        .filter(|r| r.symmetric)
        .map(|r| r.name.as_str())
        .collect();
    symmetric_entries.sort_unstable();
    if !inverse_entries.is_empty() {
        canonical.push_str(&format!(
            "\ninverse_aliases:{}",
            inverse_entries.join("\0\0")
        ));
    }
    if !symmetric_entries.is_empty() {
        canonical.push_str(&format!("\nsymmetric:{}", symmetric_entries.join("\0\0")));
    }

    crate::sha256_hex::sha256_hex(canonical.as_bytes())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_ontology(dir: &TempDir, content: &str) -> PathBuf {
        let lcg_dir = dir.path().join(".lcg");
        std::fs::create_dir_all(&lcg_dir).unwrap();
        let path = lcg_dir.join("ontology.yaml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    // ── normalization ─────────────────────────────────────────────────────────

    #[test]
    fn normalize_entity_type_pascal_case() {
        assert_eq!(normalize_entity_type("person"), "Person");
        assert_eq!(normalize_entity_type("ORGANIZATION"), "Organization");
        assert_eq!(
            normalize_entity_type("peer_reviewed_paper"),
            "PeerReviewedPaper"
        );
        assert_eq!(normalize_entity_type("MyEntity"), "MyEntity");
        assert_eq!(normalize_entity_type(""), "");
    }

    #[test]
    fn normalize_relation_type_screaming_snake() {
        assert_eq!(normalize_relation_type("AUTHORED"), "AUTHORED");
        assert_eq!(
            normalize_relation_type("affiliated_with"),
            "AFFILIATED_WITH"
        );
        assert_eq!(normalize_relation_type("AffiliatedWith"), "AFFILIATED_WITH");
        assert_eq!(normalize_relation_type("cites"), "CITES");
        assert_eq!(normalize_relation_type(""), "");
    }

    // ── load_ontology ─────────────────────────────────────────────────────────

    #[test]
    fn load_ontology_missing_file_returns_none() {
        let dir = TempDir::new().unwrap();
        let result = load_ontology(Some(dir.path()));
        assert!(result.is_none());
    }

    #[test]
    fn load_ontology_none_workspace_root_returns_none() {
        let result = load_ontology(None);
        assert!(result.is_none());
    }

    #[test]
    fn load_ontology_empty_types_returns_none() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "mode: open\nentity_types: []\nrelation_types: []\n");
        let result = load_ontology(Some(dir.path()));
        assert!(result.is_none(), "empty ontology should return None");
    }

    #[test]
    fn load_ontology_malformed_yaml_returns_none_no_panic() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "not: valid: yaml: [{{\n");
        let result = load_ontology(Some(dir.path()));
        assert!(result.is_none(), "malformed YAML should return None");
    }

    #[test]
    fn load_ontology_valid_file_returns_some() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            r#"
mode: strict
entity_types:
  - name: Person
    description: A human individual
  - name: Organization
    description: A company or institution
  - name: Paper
relation_types:
  - name: AUTHORED
    description: A person authored a paper
    source_type: Person
    target_type: Paper
  - name: AFFILIATED_WITH
    source_type: Person
    target_type: Organization
"#,
        );
        let ontology = load_ontology(Some(dir.path())).expect("should load ontology");
        assert_eq!(ontology.mode, OntologyMode::Strict);
        assert_eq!(ontology.entity_types.len(), 3);
        assert_eq!(ontology.relation_types.len(), 2);
        assert!(ontology.has_entity_types());
        assert!(ontology.has_relation_types());
        let names = ontology.entity_type_names();
        assert!(names.contains("Person"));
        assert!(names.contains("Organization"));
        assert!(names.contains("Paper"));
        let rnames = ontology.relation_type_names();
        assert!(rnames.contains("AUTHORED"));
        assert!(rnames.contains("AFFILIATED_WITH"));
    }

    #[test]
    fn load_ontology_entity_types_only() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "mode: open\nentity_types:\n  - name: Person\n");
        let ontology = load_ontology(Some(dir.path())).expect("should load");
        assert_eq!(ontology.entity_types.len(), 1);
        assert_eq!(ontology.relation_types.len(), 0);
        assert!(!ontology.has_relation_types());
    }

    #[test]
    fn load_ontology_defaults_to_open_mode() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "entity_types:\n  - name: Concept\n");
        let ontology = load_ontology(Some(dir.path())).expect("should load");
        assert_eq!(ontology.mode, OntologyMode::Open);
    }

    #[test]
    fn load_ontology_normalizes_names() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: peer_reviewed_paper\nrelation_types:\n  - name: affiliated_with\n",
        );
        let ontology = load_ontology(Some(dir.path())).expect("should load");
        assert_eq!(ontology.entity_types[0].name, "PeerReviewedPaper");
        assert_eq!(ontology.relation_types[0].name, "AFFILIATED_WITH");
    }

    #[test]
    fn load_ontology_fallback_graphiti_path() {
        let dir = TempDir::new().unwrap();
        let graphiti_dir = dir.path().join(".graphiti");
        std::fs::create_dir_all(&graphiti_dir).unwrap();
        std::fs::write(
            graphiti_dir.join("ontology.yaml"),
            "entity_types:\n  - name: Person\n",
        )
        .unwrap();
        let ontology = load_ontology(Some(dir.path())).expect("should load from .graphiti");
        assert_eq!(ontology.entity_types.len(), 1);
    }

    // ── group_ontology_path / load_group_ontology ────────────────────────────

    fn write_group_ontology(dir: &TempDir, group_id: &str, content: &str) -> PathBuf {
        let path = group_ontology_path(dir.path(), group_id).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn group_ontology_path_uses_encoded_group_id() {
        let dir = TempDir::new().unwrap();
        let path = group_ontology_path(dir.path(), "safe-group_1").unwrap();
        assert_eq!(
            path,
            dir.path()
                .join(".lcg")
                .join("ontology")
                .join("safe-group_1.yaml")
        );
    }

    #[test]
    fn group_ontology_path_percent_encodes_unsafe_group_id() {
        let dir = TempDir::new().unwrap();
        let path = group_ontology_path(dir.path(), "acme/prod").unwrap();
        assert_eq!(
            path,
            dir.path()
                .join(".lcg")
                .join("ontology")
                .join("acme%2Fprod.yaml")
        );
    }

    #[test]
    fn group_ontology_path_rejects_empty_group_id() {
        let dir = TempDir::new().unwrap();
        assert!(group_ontology_path(dir.path(), "").is_err());
    }

    #[test]
    fn load_group_ontology_missing_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(load_group_ontology(dir.path(), "group-a").is_none());
    }

    #[test]
    fn load_group_ontology_empty_file_returns_none() {
        let dir = TempDir::new().unwrap();
        write_group_ontology(&dir, "group-a", "");
        assert!(load_group_ontology(dir.path(), "group-a").is_none());
    }

    #[test]
    fn load_group_ontology_malformed_yaml_returns_none_no_panic() {
        let dir = TempDir::new().unwrap();
        write_group_ontology(&dir, "group-a", "not: valid: yaml: [{{\n");
        assert!(load_group_ontology(dir.path(), "group-a").is_none());
    }

    #[test]
    fn load_group_ontology_valid_file_returns_some() {
        let dir = TempDir::new().unwrap();
        write_group_ontology(
            &dir,
            "group-a",
            "mode: strict\nentity_types:\n  - name: KnowledgeChannel\n",
        );
        let ontology = load_group_ontology(dir.path(), "group-a").expect("should load");
        assert_eq!(ontology.mode, OntologyMode::Strict);
        assert_eq!(ontology.entity_types[0].name, "KnowledgeChannel");
    }

    #[test]
    fn load_group_ontology_does_not_read_another_groups_file() {
        let dir = TempDir::new().unwrap();
        write_group_ontology(&dir, "group-a", "entity_types:\n  - name: Person\n");
        assert!(load_group_ontology(dir.path(), "group-b").is_none());
    }

    #[test]
    fn load_group_ontology_percent_encoded_group_id_round_trips() {
        let dir = TempDir::new().unwrap();
        write_group_ontology(&dir, "acme/prod", "entity_types:\n  - name: Person\n");
        let ontology =
            load_group_ontology(dir.path(), "acme/prod").expect("should load encoded file");
        assert_eq!(ontology.entity_types[0].name, "Person");
        // A different, unsafe group_id that happens to share the same encoded prefix must not collide.
        assert!(load_group_ontology(dir.path(), "acme").is_none());
    }

    // ── load_ontology_from_path ───────────────────────────────────────────────

    #[test]
    fn load_ontology_from_path_happy_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ontology.yaml");
        std::fs::write(
            &path,
            "entity_types:\n  - name: Person\nrelation_types:\n  - name: authored\n",
        )
        .unwrap();
        let ontology =
            load_ontology_from_path(&path, OntologyMode::Open).expect("should load from bare path");
        assert_eq!(ontology.mode, OntologyMode::Open);
        assert_eq!(ontology.entity_types.len(), 1);
        assert_eq!(ontology.relation_types.len(), 1);
    }

    #[test]
    fn load_ontology_from_path_missing_file_is_err() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does-not-exist.yaml");
        let result = load_ontology_from_path(&path, OntologyMode::Strict);
        assert!(result.is_err(), "missing file must be a loud error");
    }

    #[test]
    fn load_ontology_from_path_malformed_yaml_is_err() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ontology.yaml");
        std::fs::write(&path, "not: valid: yaml: [{{\n").unwrap();
        let result = load_ontology_from_path(&path, OntologyMode::Strict);
        assert!(result.is_err(), "malformed YAML must be a loud error");
    }

    #[test]
    fn load_ontology_from_path_empty_types_is_err() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ontology.yaml");
        std::fs::write(&path, "mode: open\nentity_types: []\nrelation_types: []\n").unwrap();
        let result = load_ontology_from_path(&path, OntologyMode::Strict);
        assert!(
            result.is_err(),
            "an ontology with no entity or relation types must be a loud error"
        );
    }

    #[test]
    fn load_ontology_from_path_mode_overrides_file_declared_open() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ontology.yaml");
        std::fs::write(&path, "mode: open\nentity_types:\n  - name: Person\n").unwrap();
        let ontology = load_ontology_from_path(&path, OntologyMode::Strict).expect("should load");
        assert_eq!(
            ontology.mode,
            OntologyMode::Strict,
            "CLI-supplied mode must override the file's declared mode"
        );
    }

    #[test]
    fn load_ontology_from_path_mode_overrides_file_declared_strict() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ontology.yaml");
        std::fs::write(&path, "mode: strict\nentity_types:\n  - name: Person\n").unwrap();
        let ontology = load_ontology_from_path(&path, OntologyMode::Open).expect("should load");
        assert_eq!(
            ontology.mode,
            OntologyMode::Open,
            "CLI-supplied mode must override the file's declared mode"
        );
    }

    #[test]
    fn load_ontology_from_path_mode_applies_even_without_file_declared_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ontology.yaml");
        std::fs::write(&path, "entity_types:\n  - name: Person\n").unwrap();
        let ontology = load_ontology_from_path(&path, OntologyMode::Strict).expect("should load");
        assert_eq!(ontology.mode, OntologyMode::Strict);
    }

    // ── content_hash ─────────────────────────────────────────────────────────

    #[allow(clippy::type_complexity)]
    fn make_ontology(
        mode: OntologyMode,
        entities: &[(&str, Option<&str>)],
        relations: &[(&str, Option<&str>, Option<&str>, Option<&str>)],
    ) -> Ontology {
        let entity_types: Vec<EntityTypeDef> = entities
            .iter()
            .map(|(name, desc)| EntityTypeDef {
                name: name.to_string(),
                description: desc.map(|s| s.to_string()),
                parent: None,
                identity: false,
                extract: true,
            })
            .collect();
        let ancestor_map = compute_ancestor_map(&entity_types);
        Ontology {
            mode,
            entity_types,
            ancestor_map,
            relation_types: relations
                .iter()
                .map(|(name, src, tgt, desc)| crate::ontology::RelationTypeDef {
                    name: name.to_string(),
                    source_types: src.map(|s| vec![s.to_string()]).unwrap_or_default(),
                    target_types: tgt.map(|s| vec![s.to_string()]).unwrap_or_default(),
                    inverse_aliases: vec![],
                    symmetric: false,
                    description: desc.map(|s| s.to_string()),
                    aliases: vec![],
                    keywords: vec![],
                    extract: true,
                })
                .collect(),
        }
    }

    #[test]
    fn content_hash_none_returns_sentinel() {
        assert_eq!(content_hash(None), "none");
    }

    #[test]
    fn content_hash_same_ontology_is_stable() {
        let o = make_ontology(OntologyMode::Open, &[("Person", None)], &[]);
        let h1 = content_hash(Some(&o));
        let h2 = content_hash(Some(&o));
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_entity_addition_changes_hash() {
        let o1 = make_ontology(OntologyMode::Open, &[("Person", None)], &[]);
        let o2 = make_ontology(
            OntologyMode::Open,
            &[("Person", None), ("Equipment", None)],
            &[],
        );
        assert_ne!(content_hash(Some(&o1)), content_hash(Some(&o2)));
    }

    #[test]
    fn content_hash_relation_rename_changes_hash() {
        let o1 = make_ontology(
            OntologyMode::Open,
            &[("Person", None)],
            &[("AUTHORED", None, None, None)],
        );
        let o2 = make_ontology(
            OntologyMode::Open,
            &[("Person", None)],
            &[("WROTE", None, None, None)],
        );
        assert_ne!(content_hash(Some(&o1)), content_hash(Some(&o2)));
    }

    #[test]
    fn content_hash_mode_flip_changes_hash() {
        let o1 = make_ontology(OntologyMode::Open, &[("Person", None)], &[]);
        let o2 = make_ontology(OntologyMode::Strict, &[("Person", None)], &[]);
        assert_ne!(content_hash(Some(&o1)), content_hash(Some(&o2)));
    }

    #[test]
    fn content_hash_description_update_changes_hash() {
        let o1 = make_ontology(OntologyMode::Open, &[("Person", None)], &[]);
        let o2 = make_ontology(
            OntologyMode::Open,
            &[("Person", Some("A human individual"))],
            &[],
        );
        assert_ne!(content_hash(Some(&o1)), content_hash(Some(&o2)));
    }

    #[test]
    fn content_hash_order_independent() {
        let o1 = make_ontology(
            OntologyMode::Open,
            &[("Person", None), ("Organization", None)],
            &[],
        );
        let o2 = make_ontology(
            OntologyMode::Open,
            &[("Organization", None), ("Person", None)],
            &[],
        );
        assert_eq!(content_hash(Some(&o1)), content_hash(Some(&o2)));
    }

    #[test]
    fn content_hash_none_differs_from_empty_would_be_same_sentinel() {
        // None always returns "none" — distinct from any real ontology hash
        let h = content_hash(None);
        assert_eq!(h, "none");
        assert_ne!(h.len(), 64); // real SHA-256 is 64 hex chars; "none" is 4
    }

    // ── parent / ancestor tests ───────────────────────────────────────────────

    #[test]
    fn load_ontology_2level_parent_builds_ancestor_map() {
        let dir = TempDir::new().unwrap();
        // Use pre-normalized names (PascalCase) to avoid surprises with name normalization
        write_ontology(
            &dir,
            "entity_types:\n  - name: Document\n  - name: Rfc\n    parent: Document\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load");
        assert_eq!(
            o.ancestor_map.get("Rfc"),
            Some(&vec!["Document".to_string()])
        );
        assert_eq!(o.ancestor_map.get("Document"), Some(&vec![]));
    }

    #[test]
    fn load_ontology_3level_chain_builds_full_ancestor_list() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Document\n  - name: Rfc\n    parent: Document\n  - name: SubDoc\n    parent: Rfc\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load");
        // SubDoc → Rfc → Document: ancestors are [Document, Rfc] (supertype-first)
        assert_eq!(
            o.ancestor_map.get("SubDoc"),
            Some(&vec!["Document".to_string(), "Rfc".to_string()])
        );
        assert_eq!(
            o.ancestor_map.get("Rfc"),
            Some(&vec!["Document".to_string()])
        );
        assert_eq!(o.ancestor_map.get("Document"), Some(&vec![]));
    }

    #[test]
    fn descendants_of_covers_multi_level_and_unknown_labels() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Document\n  - name: Rfc\n    parent: Document\n  - name: SubDoc\n    parent: Rfc\n  - name: Person\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load");
        assert_eq!(o.descendants_of("Document"), vec!["Rfc", "SubDoc"]);
        assert_eq!(o.descendants_of("Rfc"), vec!["SubDoc"]);
        assert!(o.descendants_of("SubDoc").is_empty());
        assert!(o.descendants_of("Person").is_empty());
        assert!(o.descendants_of("Nope").is_empty());
    }

    #[test]
    fn load_ontology_flat_produces_empty_ancestor_vecs() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n  - name: Organization\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load");
        assert_eq!(o.ancestor_map.get("Person"), Some(&vec![]));
        assert_eq!(o.ancestor_map.get("Organization"), Some(&vec![]));
    }

    #[test]
    fn load_ontology_undeclared_parent_cleared_with_no_panic() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Rfc\n    parent: NonExistent\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load without panicking");
        // Rfc should load with no parent (name stays as-is since it's already normalized)
        let rfc = o.entity_types.iter().find(|e| e.name == "Rfc");
        assert!(rfc.is_some(), "Rfc must still be present");
        assert!(
            rfc.unwrap().parent.is_none(),
            "undeclared parent must be cleared"
        );
        assert_eq!(o.ancestor_map.get("Rfc"), Some(&vec![]));
    }

    #[test]
    fn load_ontology_2node_cycle_cleared_no_panic() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: A\n    parent: B\n  - name: B\n    parent: A\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load without panicking");
        let a = o.entity_types.iter().find(|e| e.name == "A").unwrap();
        let b = o.entity_types.iter().find(|e| e.name == "B").unwrap();
        assert!(
            a.parent.is_none(),
            "cycle member A must have parent cleared"
        );
        assert!(
            b.parent.is_none(),
            "cycle member B must have parent cleared"
        );
        assert_eq!(o.ancestor_map.get("A"), Some(&vec![]));
        assert_eq!(o.ancestor_map.get("B"), Some(&vec![]));
    }

    #[test]
    fn load_ontology_3node_cycle_all_cleared_no_panic() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: A\n    parent: C\n  - name: B\n    parent: A\n  - name: C\n    parent: B\n",
        );
        let o = load_ontology(Some(dir.path())).expect("should load without panicking");
        for name in ["A", "B", "C"] {
            let et = o.entity_types.iter().find(|e| e.name == name).unwrap();
            assert!(
                et.parent.is_none(),
                "cycle member {name} must have parent cleared"
            );
            assert_eq!(o.ancestor_map.get(name), Some(&vec![]));
        }
    }

    #[test]
    fn load_ontology_entity_declared_explicitly_is_ignored() {
        // `Entity` is the implicit root — declaring it explicitly is silently dropped with a warning
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "entity_types:\n  - name: Entity\n  - name: Person\n");
        let o = load_ontology(Some(dir.path())).expect("should load without panicking");
        let names: Vec<_> = o.entity_types.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"Entity"),
            "explicit 'Entity' type must be filtered"
        );
        assert!(names.contains(&"Person"), "other types must remain");
    }

    #[test]
    fn content_hash_flat_ontology_unchanged_vs_pre_173_format() {
        // Regression guard: flat ontologies (no parent fields) must produce the same hash
        // as pre-#173 code, because the `parent_edges:` segment is only appended when
        // at least one parent is declared (ADR-0032).
        // The canonical form for a flat ontology is identical to the pre-#173 form:
        //   "mode:{mode}\nentity_types:{entries}\nrelation_types:{entries}"
        let o = make_ontology(OntologyMode::Open, &[("Person", None)], &[]);
        // Manually build the expected canonical string using the pre-#173 format
        let expected_canonical = "mode:open\nentity_types:Person\0\nrelation_types:";
        let expected_hash = { crate::sha256_hex::sha256_hex(expected_canonical.as_bytes()) };
        assert_eq!(
            content_hash(Some(&o)),
            expected_hash,
            "flat-ontology hash must match pre-#173 canonical form"
        );
    }

    #[test]
    fn identity_flag_parses_and_defaults_false() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: person\n    identity: true\n  - name: Technology\n",
        );
        let o = load_ontology(Some(dir.path())).unwrap();
        let person = o.entity_types.iter().find(|e| e.name == "Person").unwrap();
        let tech = o
            .entity_types
            .iter()
            .find(|e| e.name == "Technology")
            .unwrap();
        assert!(person.identity, "identity: true must parse");
        assert!(!tech.identity, "absent identity must default to false");
        assert_eq!(
            o.identity_set().into_iter().collect::<Vec<_>>(),
            vec!["Person".to_string()]
        );
    }

    #[test]
    fn identity_kind_matches_normalized_primary_type_only() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Document\n    identity: true\n  - name: RFC\n    parent: Document\n  - name: Person\n    identity: true\n",
        );
        let o = load_ontology(Some(dir.path())).unwrap();
        assert_eq!(o.identity_kind("person").as_deref(), Some("Person"));
        assert_eq!(o.identity_kind("PERSON").as_deref(), Some("Person"));
        // FR-003: a non-identity child of an identity-bearing parent stays default.
        assert_eq!(o.identity_kind("Rfc"), None);
        assert_eq!(o.identity_kind("Unclassified"), None);
        assert_eq!(o.identity_kind("Undeclared"), None);
    }

    #[test]
    fn content_hash_flagless_unchanged_and_identity_changes_hash() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "entity_types:\n  - name: Person\n");
        let flat = load_ontology(Some(dir.path())).unwrap();
        let expected = {
            crate::sha256_hex::sha256_hex(
                "mode:open\nentity_types:Person\0\nrelation_types:".as_bytes(),
            )
        };
        assert_eq!(content_hash(Some(&flat)), expected);

        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n    identity: true\n",
        );
        let flagged = load_ontology(Some(dir.path())).unwrap();
        assert_ne!(content_hash(Some(&flagged)), expected);

        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n    identity: false\n",
        );
        let explicit_false = load_ontology(Some(dir.path())).unwrap();
        assert_eq!(content_hash(Some(&explicit_false)), expected);
    }

    const ASSERT_ONLY_YAML: &str = "entity_types:\n  - name: Person\n  - name: Source\n    identity: true\n    extract: false\nrelation_types:\n  - name: KNOWS\n  - name: DERIVED_FROM\n    extract: false\n";

    #[test]
    fn extract_defaults_true_and_parses_false() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, ASSERT_ONLY_YAML);
        let o = load_ontology(Some(dir.path())).unwrap();
        let person = o.entity_types.iter().find(|e| e.name == "Person").unwrap();
        let source = o.entity_types.iter().find(|e| e.name == "Source").unwrap();
        assert!(person.extract);
        assert!(!source.extract);
        assert!(source.identity);
        let knows = o.relation_types.iter().find(|r| r.name == "KNOWS").unwrap();
        let derived = o
            .relation_types
            .iter()
            .find(|r| r.name == "DERIVED_FROM")
            .unwrap();
        assert!(knows.extract);
        assert!(!derived.extract);
    }

    #[test]
    fn extractable_accessors_exclude_extract_false_but_declared_set_keeps_it() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, ASSERT_ONLY_YAML);
        let o = load_ontology(Some(dir.path())).unwrap();
        assert!(o.entity_type_names().contains("Source"));
        assert!(o.relation_type_names().contains("DERIVED_FROM"));
        assert_eq!(
            o.extractable_entity_type_names(),
            HashSet::from(["Person".to_string()])
        );
        assert_eq!(
            o.extractable_relation_type_names(),
            HashSet::from(["KNOWS".to_string()])
        );
        // Raw open-mode labels are normalized before comparing.
        assert!(o.is_extract_false_entity("Source"));
        assert!(o.is_extract_false_entity("source"));
        assert!(o.is_extract_false_entity("SOURCE"));
        assert!(!o.is_extract_false_entity("Person"));
        assert!(!o.is_extract_false_entity("Unknown"));
        assert!(o.is_extract_false_relation("derived_from"));
        assert!(o.is_extract_false_relation("DerivedFrom"));
        assert!(!o.is_extract_false_relation("KNOWS"));
        // The identity set still counts the assert-only type (FR-008) …
        assert!(o.identity_set().contains("Source"));
        // … but the extraction path never derives its kind (FR-004).
        assert_eq!(o.identity_kind("Source"), None);
    }

    #[test]
    fn content_hash_extract_flag_changes_hash_only_when_false() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n    identity: true\n",
        );
        let base = load_ontology(Some(dir.path())).unwrap();
        let base_hash = content_hash(Some(&base));

        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n    identity: true\n    extract: true\n",
        );
        let explicit_true = load_ontology(Some(dir.path())).unwrap();
        assert_eq!(content_hash(Some(&explicit_true)), base_hash);

        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n    identity: true\n    extract: false\n",
        );
        let flipped = load_ontology(Some(dir.path())).unwrap();
        assert_ne!(content_hash(Some(&flipped)), base_hash);
        // Flipping extract never changes the identity set (FR-011).
        assert_eq!(flipped.identity_set(), base.identity_set());

        // A relation flip is also visible.
        write_ontology(&dir, "relation_types:\n  - name: KNOWS\n");
        let r_base = load_ontology(Some(dir.path())).unwrap();
        write_ontology(
            &dir,
            "relation_types:\n  - name: KNOWS\n    extract: false\n",
        );
        let r_flipped = load_ontology(Some(dir.path())).unwrap();
        assert_ne!(content_hash(Some(&r_base)), content_hash(Some(&r_flipped)));
    }

    #[test]
    fn content_hash_adding_parent_changes_hash() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "entity_types:\n  - name: Document\n  - name: RFC\n");
        let o_flat = load_ontology(Some(dir.path())).unwrap();

        write_ontology(
            &dir,
            "entity_types:\n  - name: Document\n  - name: RFC\n    parent: Document\n",
        );
        let o_hierarchy = load_ontology(Some(dir.path())).unwrap();

        let h_flat = content_hash(Some(&o_flat));
        let h_hierarchy = content_hash(Some(&o_hierarchy));
        assert_ne!(
            h_flat, h_hierarchy,
            "adding a parent must change the content hash"
        );
        // Verify the hierarchy hash includes parent_edges in its canonical form
        assert_eq!(h_hierarchy.len(), 64, "hash must be a valid SHA-256 hex");
    }

    #[test]
    fn content_hash_parent_edges_order_independent() {
        // Two ontologies with the same parent edges in different declaration order produce the same hash
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: A\n  - name: B\n    parent: A\n  - name: C\n    parent: A\n",
        );
        let o1 = load_ontology(Some(dir.path())).unwrap();

        write_ontology(
            &dir,
            "entity_types:\n  - name: A\n  - name: C\n    parent: A\n  - name: B\n    parent: A\n",
        );
        let o2 = load_ontology(Some(dir.path())).unwrap();

        assert_eq!(
            content_hash(Some(&o1)),
            content_hash(Some(&o2)),
            "parent_edges must be sorted — declaration order must not affect hash"
        );
    }

    #[test]
    fn content_hash_removing_parent_restores_flat_hash() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, "entity_types:\n  - name: Document\n  - name: RFC\n");
        let o_flat = load_ontology(Some(dir.path())).unwrap();

        write_ontology(
            &dir,
            "entity_types:\n  - name: Document\n  - name: RFC\n    parent: Document\n",
        );
        let o_hierarchy = load_ontology(Some(dir.path())).unwrap();

        write_ontology(&dir, "entity_types:\n  - name: Document\n  - name: RFC\n");
        let o_restored = load_ontology(Some(dir.path())).unwrap();

        assert_ne!(
            content_hash(Some(&o_flat)),
            content_hash(Some(&o_hierarchy))
        );
        assert_eq!(
            content_hash(Some(&o_flat)),
            content_hash(Some(&o_restored)),
            "removing all parents must restore the flat-ontology hash"
        );
    }
    // ── #816: direction fields ────────────────────────────────────────────────

    const DIRECTION_YAML: &str = "entity_types:\n  - name: Person\n  - name: Team\n  - name: Project\n  - name: System\nrelation_types:\n  - name: OWNS\n    source_type: [Person, Team]\n    target_type: [Project, System]\n    inverse_aliases: [OWNED_BY]\n  - name: KNOWS\n    symmetric: true\n  - name: APPROVES\n    source_type: Person\n";

    #[test]
    fn direction_fields_parse_lists_inverse_aliases_symmetric() {
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, DIRECTION_YAML);
        let o = load_ontology(Some(dir.path())).unwrap();
        let owns = o.relation_types.iter().find(|r| r.name == "OWNS").unwrap();
        assert_eq!(owns.source_types, vec!["Person", "Team"]);
        assert_eq!(owns.target_types, vec!["Project", "System"]);
        assert_eq!(owns.inverse_aliases, vec!["OWNED_BY"]);
        assert!(!owns.symmetric);
        let knows = o.relation_types.iter().find(|r| r.name == "KNOWS").unwrap();
        assert!(knows.symmetric);
        let approves = o
            .relation_types
            .iter()
            .find(|r| r.name == "APPROVES")
            .unwrap();
        assert_eq!(approves.source_types, vec!["Person"]);
        assert!(approves.target_types.is_empty());
        assert!(o.has_direction_info());
    }

    #[test]
    fn single_element_list_hashes_as_legacy_string() {
        let legacy = "entity_types:\n  - name: Person\n  - name: Paper\nrelation_types:\n  - name: AUTHORED\n    source_type: Person\n    target_type: Paper\n";
        let listed = "entity_types:\n  - name: Person\n  - name: Paper\nrelation_types:\n  - name: AUTHORED\n    source_type: [Person]\n    target_type: [Paper]\n";
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, legacy);
        let a = load_ontology(Some(dir.path())).unwrap();
        write_ontology(&dir, listed);
        let b = load_ontology(Some(dir.path())).unwrap();
        assert_eq!(content_hash(Some(&a)), content_hash(Some(&b)));
        // Pre-#816 canonical form: signature segments are the plain strings (entity entries
        // sorted: "Paper\0" < "Person\0").
        let expected = crate::sha256_hex::sha256_hex(
            "mode:open\nentity_types:Paper\0\0\0Person\0\nrelation_types:AUTHORED\0Person\0Paper\0\0\0"
                .as_bytes(),
        );
        assert_eq!(content_hash(Some(&a)), expected);
    }

    #[test]
    fn hash_unchanged_without_new_fields_and_changes_with_them() {
        let base = "entity_types:\n  - name: Person\n  - name: Doc\nrelation_types:\n  - name: WROTE\n    source_type: Person\n";
        let dir = TempDir::new().unwrap();
        write_ontology(&dir, base);
        let o = load_ontology(Some(dir.path())).unwrap();
        // The canonical string has no direction segments.
        let canonical =
            "mode:open\nentity_types:Doc\0\0\0Person\0\nrelation_types:WROTE\0Person\0\0\0\0";
        assert_eq!(
            content_hash(Some(&o)),
            crate::sha256_hex::sha256_hex(canonical.as_bytes())
        );
        write_ontology(&dir, &format!("{base}    inverse_aliases: [WRITTEN_BY]\n"));
        let inv = load_ontology(Some(dir.path())).unwrap();
        assert_ne!(content_hash(Some(&inv)), content_hash(Some(&o)));
        write_ontology(&dir, &format!("{base}    symmetric: true\n"));
        let sym = load_ontology(Some(dir.path())).unwrap();
        assert_ne!(content_hash(Some(&sym)), content_hash(Some(&o)));
        assert_ne!(content_hash(Some(&sym)), content_hash(Some(&inv)));
        // Multi-element list differs from a single element.
        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\n  - name: Doc\nrelation_types:\n  - name: WROTE\n    source_type: [Person, Doc]\n",
        );
        let multi = load_ontology(Some(dir.path())).unwrap();
        assert_ne!(content_hash(Some(&multi)), content_hash(Some(&o)));
    }

    #[test]
    fn undeclared_signature_type_is_ignored_not_an_error() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\nrelation_types:\n  - name: LIKES\n    source_type: [Person, Ghost]\n    target_type: Ghost\n",
        );
        let o = load_ontology(Some(dir.path())).unwrap();
        let likes = &o.relation_types[0];
        // Declared as written (so the hash is stable)...
        assert_eq!(likes.source_types, vec!["Person", "Ghost"]);
        // ...but ignored by the check: Ghost is dropped, an all-undeclared side becomes unset.
        assert_eq!(o.effective_source_types(likes), vec!["Person"]);
        assert!(o.effective_target_types(likes).is_empty());
    }

    #[test]
    fn blank_and_duplicate_signature_entries_are_dropped() {
        let dir = TempDir::new().unwrap();
        write_ontology(
            &dir,
            "entity_types:\n  - name: Person\nrelation_types:\n  - name: LIKES\n    source_type: [person, Person, '']\n",
        );
        let o = load_ontology(Some(dir.path())).unwrap();
        assert_eq!(o.relation_types[0].source_types, vec!["Person"]);
    }

    #[test]
    fn no_direction_info_for_flat_relations() {
        let o = make_ontology(
            OntologyMode::Open,
            &[("Person", None)],
            &[("KNOWS", None, None, None)],
        );
        assert!(!o.has_direction_info());
    }
}
