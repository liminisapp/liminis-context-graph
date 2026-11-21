use crate::ontology::{Ontology, OntologyMode};
use crate::types::SourceType;

static EXTRACT_TEXT: &str = include_str!("extract_text.txt");
static EXTRACT_MESSAGE: &str = include_str!("extract_message.txt");
static EXTRACT_JSON: &str = include_str!("extract_json.txt");
static EXTRACT_EDGES: &str = include_str!("extract_edges.txt");
static CLASSIFY_NODES: &str = include_str!("classify_nodes.txt");

/// The original closed entity-type list used when no workspace ontology is configured.
const DEFAULT_ENTITY_TYPES_SECTION: &str = "\
For each entity extracted, assign a single entity_type label from this closed ontology:
- Person: An individual human being
- Organization: A company, institution, group, or governing body
- Software: A software application, library, or framework
- Service: A deployed service, platform, or SaaS product
- System: An integrated technical system or infrastructure component
- Technology: A protocol, standard, methodology, or technical approach
- Concept: An abstract idea, principle, or theoretical framework
- Location: A physical or virtual place, region, or address
- Event: A dated or scheduled occurrence or incident
- Process: A workflow, procedure, or repeatable operation
- Requirement: A constraint, specification, or policy requirement
- Document: A document, report, specification, or publication
- Product: A physical or digital product or deliverable
- Project: A project, initiative, or program
- Award: A prize, honor, or recognition
- Book: A book, novel, or long-form publication
Choose the single most appropriate type. If none fit well, use the closest match.";

fn build_entity_types_section(ontology: Option<&Ontology>) -> String {
    let onto = match ontology {
        // `extract: false` types (#637) are never offered; none left ⇒ same as no ontology.
        Some(o) if o.extractable_entity_types().next().is_some() => o,
        _ => return DEFAULT_ENTITY_TYPES_SECTION.to_string(),
    };

    let mut section = String::from(
        "<ENTITY_TYPES>\nThe following entity types are defined for this workspace:\n",
    );
    for et in onto.extractable_entity_types() {
        if let Some(desc) = &et.description {
            section.push_str(&format!("- {}: {}\n", et.name, desc));
        } else {
            section.push_str(&format!("- {}\n", et.name));
        }
    }
    match onto.mode {
        OntologyMode::Strict => section.push_str(
            "Only extract entities whose type is exactly one of the listed types; \
             do not invent or use types not in this list.\n",
        ),
        OntologyMode::Open => section.push_str(
            "Prefer the listed entity types when they apply; \
             you may use other types for entities that clearly don't fit any listed type.\n",
        ),
    }
    section.push_str("</ENTITY_TYPES>");
    section
}

// FR-002/FR-003 (issue #310): under `strict` mode, this section also lists each relation
// type's declared aliases/keywords and appends an instruction restricting the model to the
// declared vocabulary — mirroring `build_entity_types_section`'s mode-aware pattern. Under
// `open` mode, output is deliberately unchanged from before this issue: Acceptance Scenario 2
// requires the open-mode rendering stay byte-identical, so the `Open` arm below is a no-op
// rather than adding both-modes text like the entity-type section does (see ADR-0310).
// ADR-0816 relaxes that constraint for direction: signatures (including half signatures) and
// symmetric markers render in both modes, and the `# DIRECTION` block in `extract_edges.txt`
// applies to every mode. Inverse aliases render in strict mode only (open mode never resolves
// aliases).
fn build_fact_types_section(ontology: Option<&Ontology>) -> String {
    let onto = match ontology {
        Some(o) if o.extractable_relation_types().next().is_some() => o,
        _ => return String::new(),
    };

    let mut section = String::from(
        "<FACT_TYPES>\nThe following relation types are defined for this workspace:\n",
    );
    for rt in onto.extractable_relation_types() {
        // #816: a half signature renders too (`(Person → any)`), lists join with `|`, and only
        // declared entity types are shown. Symmetric relations carry no direction to show.
        let src = onto.effective_source_types(rt);
        let tgt = onto.effective_target_types(rt);
        let sig = if rt.symmetric || (src.is_empty() && tgt.is_empty()) {
            String::new()
        } else {
            let side = |v: &[&str]| {
                if v.is_empty() {
                    "any".to_string()
                } else {
                    v.join(" | ")
                }
            };
            format!(" ({} → {})", side(&src), side(&tgt))
        };
        let sym = if rt.symmetric { " [symmetric]" } else { "" };
        if let Some(desc) = &rt.description {
            section.push_str(&format!("- {}{}{}: {}\n", rt.name, sig, sym, desc));
        } else {
            section.push_str(&format!("- {}{}{}\n", rt.name, sig, sym));
        }
        if rt.symmetric {
            section.push_str(
                "  Symmetric: the order of source_name and target_name does not matter.\n",
            );
        }
        if onto.mode == OntologyMode::Strict {
            for ia in &rt.inverse_aliases {
                section.push_str(&format!(
                    "  Inverse alias: {} = {} with source and target swapped (emit {} as it reads, \
                     so source_name is the subject of {}; the system swaps it into {} order)\n",
                    ia, rt.name, ia, ia, rt.name
                ));
            }
            if !rt.aliases.is_empty() {
                section.push_str(&format!("  Aliases: {}\n", rt.aliases.join(", ")));
            }
            if !rt.keywords.is_empty() {
                section.push_str(&format!("  Keywords: {}\n", rt.keywords.join(", ")));
            }
        }
    }
    section.push_str("</FACT_TYPES>\n");
    match onto.mode {
        OntologyMode::Strict => section.push_str(
            "Only use relation types from the list above, identified by their canonical name. \
             Aliases and keywords are listed to help you recognize a matching fact — always \
             output the canonical name, never the alias or keyword itself. Do not invent \
             relation types outside this list.\n",
        ),
        OntologyMode::Open => {}
    }
    section
}

/// Returns the entity extraction system prompt for the given source type, with optional ontology injection.
pub fn entity_system_prompt(source_type: SourceType, ontology: Option<&Ontology>) -> String {
    let template = match source_type {
        SourceType::Text => EXTRACT_TEXT,
        SourceType::Message => EXTRACT_MESSAGE,
        SourceType::Json => EXTRACT_JSON,
    };
    let section = build_entity_types_section(ontology);
    template.replace("{{ENTITY_TYPES_SECTION}}", &section)
}

/// Issue #823 (ADR-0823): whether literal-object edges are enabled — true iff the ontology
/// offers at least one extractable relation type (the same predicate `build_fact_types_section`
/// uses, so an ontology whose relation types are all `extract: false` counts as no ontology).
///
/// This single predicate gates both the edge prompt's endpoint rules and the ingest-side
/// admission of name-level self-loops (`episode.rs`), so the two can never disagree. Under it a
/// fact stating a literal value (a budget, date, price, role) about one entity is emitted with
/// `source_name == target_name` and stored as a self-loop on that entity, instead of being pinned
/// to an unrelated nearby entity. Applies to strict and open modes alike.
pub fn literal_object_edges_enabled(ontology: Option<&Ontology>) -> bool {
    ontology.is_some_and(|o| o.extractable_relation_types().next().is_some())
}

/// The TASK bullet rendered when literal-object edges are off (no ontology) — today's text,
/// byte-for-byte, so the free-form prompt (and its cassette keys) is unchanged (FR-006).
const TASK_ENDPOINT_CLAUSE_DEFAULT: &str =
    "- involve two DISTINCT ENTITIES from the ENTITIES list,";

/// Extraction rules 2–3 rendered when literal-object edges are off (no ontology) — today's text.
const SINGLE_ENTITY_RULES_DEFAULT: &str = "\
2. Each fact must involve two distinct entities — source_name and target_name NEVER refer to the same entity.
3. NEVER extract facts that describe only a single entity's state, feeling, or attribute. Instead, identify the second entity that the state or feeling relates to and form a proper triple.
   - BAD: \"Alice feels happy\" (single-entity state — what is Alice happy about?)
   - GOOD: \"Alice feels happy about Bob's promotion\" → Alice FEELS_HAPPY_ABOUT Bob's promotion
   - GOOD: \"Alice congratulated Bob\" (relationship between two entities), \"Alice lives in Paris\" (relationship between entity and place)";

/// The TASK bullet rendered under an ontology (#823).
const TASK_ENDPOINT_CLAUSE_LITERAL: &str = "\
- involve two DISTINCT ENTITIES from the ENTITIES list, or state a literal value about a single entity from the list (see rule 3),";

/// Extraction rules 2–3 rendered under an ontology (#823): a literal-valued fact about one entity
/// is a self-referencing edge on that entity, never pinned to a nearby entity it does not involve.
const SINGLE_ENTITY_RULES_LITERAL: &str = "\
2. When a fact relates two entities, source_name and target_name are two distinct entities from the ENTITIES list.
3. **Literal values**: when a fact states a literal value — an amount, price, budget, cost, date, deadline, duration, percentage, count, score, version, role or title — about ONE entity, and no other listed entity takes part in that fact, set BOTH source_name and target_name to that entity. The value stays in the fact text.
   - \"Marlin Core has a monthly budget of $4,200\" → source_name: Marlin Core, target_name: Marlin Core
   - \"Orbit Supply charges $43 per unit\" → source_name: Orbit Supply, target_name: Orbit Supply
   - NEVER attach such a fact to another listed entity just because that entity appears nearby in the message. If the fact does not name the other entity, the other entity is not part of it.
   - If another listed entity genuinely takes part in the fact, extract the edge between the two as usual: \"Tomas Kowalski set Marlin Core's monthly budget at $4,200\" → Tomas Kowalski and Marlin Core.
   - Do not extract a single entity's mood or feeling as a literal value; for a state or feeling, identify the second entity it relates to (\"Alice feels happy about Bob's promotion\") or skip it.";

/// Returns the edge extraction system prompt with optional ontology injection.
///
/// With no ontology (or one offering no extractable relation type) the rendering is
/// byte-identical to the pre-#823 prompt — pinned by `testdata/extract_edges_no_ontology.golden.txt`.
pub fn edge_system_prompt(ontology: Option<&Ontology>) -> String {
    let section = build_fact_types_section(ontology);
    let (task, rules) = if literal_object_edges_enabled(ontology) {
        (TASK_ENDPOINT_CLAUSE_LITERAL, SINGLE_ENTITY_RULES_LITERAL)
    } else {
        (TASK_ENDPOINT_CLAUSE_DEFAULT, SINGLE_ENTITY_RULES_DEFAULT)
    };
    EXTRACT_EDGES
        .replace("{{TASK_ENDPOINT_CLAUSE}}", task)
        .replace("{{SINGLE_ENTITY_RULES}}", rules)
        .replace("{{FACT_TYPES_SECTION}}", &section)
}

/// Returns the entity classification system prompt.
pub fn classify_system_prompt() -> &'static str {
    CLASSIFY_NODES
}

/// Builds the entity extraction user message for a single episode.
pub fn entity_user_prompt(body: &str, custom_instructions: Option<&str>) -> String {
    let custom = custom_instructions.unwrap_or("").trim();
    if custom.is_empty() {
        format!("<TEXT>\n{body}\n</TEXT>\n")
    } else {
        format!("<TEXT>\n{body}\n</TEXT>\n\n{custom}\n")
    }
}

/// Builds the entity extraction user message for a message-type episode.
pub fn message_user_prompt(body: &str, custom_instructions: Option<&str>) -> String {
    let custom = custom_instructions.unwrap_or("").trim();
    if custom.is_empty() {
        format!("<CURRENT MESSAGE>\n{body}\n</CURRENT MESSAGE>\n")
    } else {
        format!("<CURRENT MESSAGE>\n{body}\n</CURRENT MESSAGE>\n\n{custom}\n")
    }
}

/// Builds the entity extraction user message for a JSON-type episode.
pub fn json_user_prompt(body: &str, custom_instructions: Option<&str>) -> String {
    let custom = custom_instructions.unwrap_or("").trim();
    if custom.is_empty() {
        format!("<JSON>\n{body}\n</JSON>\n\nExtract relevant entities from the provided JSON.\n")
    } else {
        format!(
            "<JSON>\n{body}\n</JSON>\n\nExtract relevant entities from the provided JSON.\n{custom}\n"
        )
    }
}

/// Builds the entity user prompt dispatch — selects the appropriate format by source type.
pub fn entity_user_prompt_for(
    source_type: SourceType,
    body: &str,
    custom_instructions: Option<&str>,
) -> String {
    match source_type {
        SourceType::Text => entity_user_prompt(body, custom_instructions),
        SourceType::Message => message_user_prompt(body, custom_instructions),
        SourceType::Json => json_user_prompt(body, custom_instructions),
    }
}

pub(crate) fn strip_control_chars(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Sanitizes a list of entity names for use in the edge-extraction prompt and tool schema:
/// strips control characters (including newlines, which would break the bullet-list structure
/// or a JSON schema `enum` entry), trims whitespace, drops entries that become empty, and
/// deduplicates while preserving first-seen order (FR-001, FR-006).
pub fn sanitize_entity_names(entity_names: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    entity_names
        .iter()
        .filter_map(|n| {
            let sanitized = strip_control_chars(n).trim().to_string();
            if sanitized.is_empty() || !seen.insert(sanitized.clone()) {
                None
            } else {
                Some(sanitized)
            }
        })
        .collect()
}

/// Normalizes a name to the key used to match an entity against a model-echoed edge endpoint
/// name: strips control characters (the same stripping `sanitize_entity_names` applies before a
/// name ever reaches the model), trims whitespace, and lowercases. An entity name containing a
/// control character is control-stripped before the model ever sees it (both in the edge
/// prompt's entity list and the tool schema's `enum`), so any endpoint-resolution key derived
/// from the *original* entity name must use the same normalization or a genuine batch-local
/// match will spuriously fail and fall through to salvage or drop.
pub fn normalize_name(name: &str) -> String {
    strip_control_chars(name).trim().to_lowercase()
}

/// Builds the edge extraction user message.
///
/// `entity_names` is the list of entity names extracted in the entity pass.
/// `reference_time` is an ISO 8601 timestamp used for temporal grounding.
/// `body` is the episode text.
pub fn edge_user_prompt(
    entity_names: &[String],
    reference_time: &str,
    body: &str,
    custom_instructions: Option<&str>,
) -> String {
    let entities_section = sanitize_entity_names(entity_names)
        .into_iter()
        .map(|n| format!("- {n}"))
        .collect::<Vec<_>>()
        .join("\n");

    let custom = custom_instructions.unwrap_or("").trim();
    if custom.is_empty() {
        format!(
            "<CURRENT_MESSAGE>\n{body}\n</CURRENT_MESSAGE>\n\n\
             <ENTITIES>\n{entities_section}\n</ENTITIES>\n\n\
             <REFERENCE_TIME>\n{reference_time}\n</REFERENCE_TIME>\n"
        )
    } else {
        format!(
            "<CURRENT_MESSAGE>\n{body}\n</CURRENT_MESSAGE>\n\n\
             <ENTITIES>\n{entities_section}\n</ENTITIES>\n\n\
             <REFERENCE_TIME>\n{reference_time}\n</REFERENCE_TIME>\n\n\
             {custom}\n"
        )
    }
}

/// Prepends a document-mode `<DOCUMENT_CONTEXT>` block (issue #785, FR-011) to a built user
/// prompt, ahead of the source-text tag. `None` (every chunk-mode call) returns `user` unchanged,
/// so the chunk-mode prompt is byte-identical (FR-019).
pub fn with_document_context(document_context: Option<&str>, user: String) -> String {
    match document_context.map(str::trim).filter(|c| !c.is_empty()) {
        Some(ctx) => format!("{ctx}\n\n{user}"),
        None => user,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_context_is_prepended_only_when_present() {
        let user = edge_user_prompt(&["A".to_string()], "2026-01-01T00:00:00Z", "body", None);
        assert_eq!(with_document_context(None, user.clone()), user);
        assert_eq!(with_document_context(Some("  "), user.clone()), user);
        let with = with_document_context(
            Some("<DOCUMENT_CONTEXT>\nx\n</DOCUMENT_CONTEXT>"),
            user.clone(),
        );
        assert!(with.starts_with("<DOCUMENT_CONTEXT>"));
        assert!(with.ends_with(&user));
        assert!(
            with.find("</DOCUMENT_CONTEXT>").unwrap() < with.find("<CURRENT_MESSAGE>").unwrap()
        );
    }

    #[test]
    fn prompt_files_are_non_empty() {
        assert!(
            !EXTRACT_TEXT.is_empty(),
            "extract_text.txt must not be empty"
        );
        assert!(
            !EXTRACT_MESSAGE.is_empty(),
            "extract_message.txt must not be empty"
        );
        assert!(
            !EXTRACT_JSON.is_empty(),
            "extract_json.txt must not be empty"
        );
        assert!(
            !EXTRACT_EDGES.is_empty(),
            "extract_edges.txt must not be empty"
        );
        assert!(
            !CLASSIFY_NODES.is_empty(),
            "classify_nodes.txt must not be empty"
        );
    }

    #[test]
    fn source_type_dispatch_returns_distinct_prompts() {
        let text = entity_system_prompt(SourceType::Text, None);
        let message = entity_system_prompt(SourceType::Message, None);
        let json = entity_system_prompt(SourceType::Json, None);
        assert_ne!(text, message, "text and message prompts must differ");
        assert_ne!(text, json, "text and json prompts must differ");
        assert_ne!(message, json, "message and json prompts must differ");
    }

    #[test]
    fn no_ontology_uses_default_entity_types() {
        let prompt = entity_system_prompt(SourceType::Text, None);
        assert!(
            prompt.contains("Person: An individual human being"),
            "default entity types must appear when no ontology is set"
        );
        assert!(
            !prompt.contains("{{ENTITY_TYPES_SECTION}}"),
            "placeholder must not appear in output"
        );
    }

    // FR-002: the entity prompt must not forbid the entity types its own ontology defines —
    // the old unqualified "NEVER extract...abstract concepts" ban contradicted the Concept
    // entity type and must not reappear in any of the three source-type prompts.
    #[test]
    fn concept_ban_contradiction_is_resolved_by_specificity_test() {
        for (label, prompt) in [
            ("extract_text.txt", EXTRACT_TEXT),
            ("extract_message.txt", EXTRACT_MESSAGE),
            ("extract_json.txt", EXTRACT_JSON),
        ] {
            assert!(
                !prompt.contains("NEVER extract vague or standalone abstract concepts"),
                "{label}: the old unqualified concept-ban phrasing must not reappear"
            );
            assert!(
                prompt.contains("Concept entity"),
                "{label}: must instruct that a specific, named concept is extracted as a Concept entity"
            );
            assert!(
                prompt.contains("Wikipedia-article test") || prompt.contains("Wikipedia article"),
                "{label}: must carry the specificity/Wikipedia-article test for concepts"
            );
        }
    }

    fn ontology_with_relation_types(mode: OntologyMode) -> Ontology {
        Ontology {
            mode,
            entity_types: vec![],
            relation_types: vec![
                crate::ontology::RelationTypeDef {
                    name: "LAUNCHED".to_string(),
                    description: Some("One entity launched another".to_string()),
                    source_types: vec![],
                    target_types: vec![],
                    inverse_aliases: vec![],
                    symmetric: false,
                    aliases: vec!["LAUNCHED_BY".to_string(), "LAUNCHED_FROM".to_string()],
                    keywords: vec!["launch".to_string()],
                    extract: true,
                },
                crate::ontology::RelationTypeDef {
                    name: "USES".to_string(),
                    description: None,
                    source_types: vec![],
                    target_types: vec![],
                    inverse_aliases: vec![],
                    symmetric: false,
                    aliases: vec![],
                    keywords: vec![],
                    extract: true,
                },
            ],
            ancestor_map: std::collections::HashMap::new(),
        }
    }

    // SC-002/Acceptance Scenario 2: open-mode rendering is byte-identical to before this issue
    // — no aliases/keywords, no mode-instruction sentence.
    #[test]
    fn open_mode_fact_types_section_unchanged() {
        let onto = ontology_with_relation_types(OntologyMode::Open);
        let prompt = edge_system_prompt(Some(&onto));
        assert!(
            !prompt.contains("Aliases:"),
            "open mode must not list aliases: {prompt}"
        );
        assert!(
            !prompt.contains("Keywords:"),
            "open mode must not list keywords: {prompt}"
        );
        assert!(
            !prompt.contains("Only use relation types"),
            "open mode must not add a vocabulary-restriction instruction: {prompt}"
        );
    }

    // SC-002/Acceptance Scenario 1: strict-mode rendering differs from open-mode for the same
    // ontology, and contains an explicit vocabulary-restriction instruction.
    #[test]
    fn strict_and_open_fact_types_sections_differ() {
        let open_prompt =
            edge_system_prompt(Some(&ontology_with_relation_types(OntologyMode::Open)));
        let strict_prompt =
            edge_system_prompt(Some(&ontology_with_relation_types(OntologyMode::Strict)));
        assert_ne!(
            open_prompt, strict_prompt,
            "strict and open renderings must differ for the same ontology"
        );
        assert!(
            strict_prompt.contains("Only use relation types"),
            "strict mode must state the vocabulary-restriction instruction: {strict_prompt}"
        );
    }

    // Acceptance Scenario 3: declared aliases/keywords are visible in the strict-mode rendering.
    #[test]
    fn strict_mode_exposes_aliases_and_keywords() {
        let onto = ontology_with_relation_types(OntologyMode::Strict);
        let prompt = edge_system_prompt(Some(&onto));
        assert!(
            prompt.contains("Aliases: LAUNCHED_BY, LAUNCHED_FROM"),
            "strict mode must list LAUNCHED's aliases: {prompt}"
        );
        assert!(
            prompt.contains("Keywords: launch"),
            "strict mode must list LAUNCHED's keywords: {prompt}"
        );
    }

    // Edge Case: a relation type with no declared aliases/keywords must not print empty
    // "Aliases:"/"Keywords:" lines.
    #[test]
    fn strict_mode_omits_alias_keyword_lines_when_absent() {
        let onto = ontology_with_relation_types(OntologyMode::Strict);
        let prompt = edge_system_prompt(Some(&onto));
        let uses_line_idx = prompt.find("- USES").expect("USES type must be listed");
        let after_uses = &prompt[uses_line_idx..];
        let next_line_end = after_uses.find('\n').unwrap_or(after_uses.len());
        let following = &after_uses[next_line_end..];
        assert!(
            !following.trim_start().starts_with("Aliases:")
                && !following.trim_start().starts_with("Keywords:"),
            "USES has no declared aliases/keywords and must not print empty lines: {prompt}"
        );
    }

    #[test]
    fn sanitize_entity_names_strips_control_chars_dedupes_and_drops_empty() {
        let names = vec![
            "Alice".to_string(),
            "Alice".to_string(),               // exact duplicate, dropped
            "Bob\ncontrol\tchars".to_string(), // control chars stripped
            "   ".to_string(),                 // empty after trim, dropped
            "\u{0007}".to_string(),            // empty after control-char strip, dropped
            "  Carol  ".to_string(),           // trimmed
        ];
        let sanitized = sanitize_entity_names(&names);
        assert_eq!(
            sanitized,
            vec![
                "Alice".to_string(),
                "Bobcontrolchars".to_string(),
                "Carol".to_string(),
            ]
        );
    }

    #[test]
    fn sanitize_entity_names_empty_input_yields_empty_output() {
        assert!(sanitize_entity_names(&[]).is_empty());
        assert!(sanitize_entity_names(&["   ".to_string(), "\n".to_string()]).is_empty());
    }

    #[test]
    fn edge_user_prompt_contains_entities() {
        let names = vec!["Alice".to_string(), "Acme Corp".to_string()];
        let prompt = edge_user_prompt(
            &names,
            "2026-01-01T00:00:00Z",
            "Alice works at Acme Corp.",
            None,
        );
        assert!(
            prompt.contains("Alice"),
            "edge prompt must contain entity name"
        );
        assert!(
            prompt.contains("Acme Corp"),
            "edge prompt must contain entity name"
        );
        assert!(
            prompt.contains("REFERENCE_TIME"),
            "edge prompt must contain REFERENCE_TIME"
        );
    }

    // ── extract: false (#637) ─────────────────────────────────────────────────

    fn assert_only_ontology(mode: OntologyMode, extract_false: bool) -> Ontology {
        use crate::ontology::{EntityTypeDef, RelationTypeDef};
        let flag = !extract_false;
        Ontology {
            mode,
            entity_types: vec![
                EntityTypeDef {
                    name: "Person".to_string(),
                    description: Some("A human".to_string()),
                    parent: None,
                    identity: false,
                    extract: true,
                },
                EntityTypeDef {
                    name: "Source".to_string(),
                    description: Some("A wiki page the host asserts".to_string()),
                    parent: None,
                    identity: true,
                    extract: flag,
                },
            ],
            relation_types: vec![
                RelationTypeDef {
                    name: "KNOWS".to_string(),
                    description: Some("acquaintance".to_string()),
                    source_types: vec![],
                    target_types: vec![],
                    inverse_aliases: vec![],
                    symmetric: false,
                    aliases: vec![],
                    keywords: vec![],
                    extract: true,
                },
                RelationTypeDef {
                    name: "DERIVED_FROM".to_string(),
                    description: Some("provenance link".to_string()),
                    source_types: vec![],
                    target_types: vec![],
                    inverse_aliases: vec![],
                    symmetric: false,
                    aliases: vec!["SOURCED_FROM".to_string()],
                    keywords: vec![],
                    extract: flag,
                },
            ],
            ancestor_map: std::collections::HashMap::new(),
        }
    }

    // Issue #824: the text extraction prompt tells the model that wikilink citations name other
    // notes and are not entities (the primary fix for note-ID junk nodes; the post-extraction
    // filter is the backstop). Deliberate snapshot of the instruction.
    #[test]
    fn text_prompt_says_citations_are_not_entities() {
        let p = entity_system_prompt(SourceType::Text, None);
        assert!(p.contains("[[note-id]]"));
        assert!(p.contains("(from [[project-quill-core-1]])"));
        assert!(p.contains("NEVER extract"));
        assert!(p.contains("not an entity"));
        // The Message and Json prompts are unchanged by #824.
        assert!(!entity_system_prompt(SourceType::Message, None).contains("[[note-id]]"));
        assert!(!entity_system_prompt(SourceType::Json, None).contains("[[note-id]]"));
    }

    #[test]
    fn extract_false_types_absent_from_prompts() {
        for mode in [OntologyMode::Open, OntologyMode::Strict] {
            let onto = assert_only_ontology(mode, true);
            let entity = entity_system_prompt(SourceType::Text, Some(&onto));
            assert!(entity.contains("- Person: A human"));
            assert!(!entity.contains("Source"));
            assert!(!entity.contains("wiki page the host asserts"));
            let edge = edge_system_prompt(Some(&onto));
            assert!(edge.contains("- KNOWS: acquaintance"));
            assert!(!edge.contains("DERIVED_FROM"));
            assert!(!edge.contains("SOURCED_FROM"));
            assert!(!edge.contains("provenance link"));
        }
    }

    #[test]
    fn flagless_prompts_list_every_type() {
        // With no `extract: false` type the rendering lists all declared types.
        let onto = assert_only_ontology(OntologyMode::Open, false);
        let entity = entity_system_prompt(SourceType::Text, Some(&onto));
        assert!(entity.contains(
            "- Person: A human\n- Source: A wiki page the host asserts\nPrefer the listed"
        ));
        let section = build_fact_types_section(Some(&onto));
        assert_eq!(
            section,
            "<FACT_TYPES>\nThe following relation types are defined for this workspace:\n\
             - KNOWS: acquaintance\n- DERIVED_FROM: provenance link\n</FACT_TYPES>\n"
        );
    }

    #[test]
    fn all_extract_false_renders_as_no_ontology_types() {
        use crate::ontology::{EntityTypeDef, RelationTypeDef};
        let onto = Ontology {
            mode: OntologyMode::Strict,
            entity_types: vec![EntityTypeDef {
                name: "Source".to_string(),
                description: None,
                parent: None,
                identity: true,
                extract: false,
            }],
            relation_types: vec![RelationTypeDef {
                name: "DERIVED_FROM".to_string(),
                description: None,
                source_types: vec![],
                target_types: vec![],
                inverse_aliases: vec![],
                symmetric: false,
                aliases: vec![],
                keywords: vec![],
                extract: false,
            }],
            ancestor_map: std::collections::HashMap::new(),
        };
        assert_eq!(
            build_entity_types_section(Some(&onto)),
            DEFAULT_ENTITY_TYPES_SECTION
        );
        assert_eq!(build_fact_types_section(Some(&onto)), "");
    }
    // ── #816: direction ──────────────────────────────────────────────────────

    fn rel(
        name: &str,
        src: &[&str],
        tgt: &[&str],
        inverse: &[&str],
        symmetric: bool,
    ) -> crate::ontology::RelationTypeDef {
        crate::ontology::RelationTypeDef {
            name: name.to_string(),
            description: None,
            source_types: src.iter().map(|s| s.to_string()).collect(),
            target_types: tgt.iter().map(|s| s.to_string()).collect(),
            inverse_aliases: inverse.iter().map(|s| s.to_string()).collect(),
            symmetric,
            aliases: vec![],
            keywords: vec![],
            extract: true,
        }
    }

    fn direction_ontology(mode: OntologyMode) -> Ontology {
        let entity_types: Vec<crate::ontology::EntityTypeDef> =
            ["Person", "Team", "Project", "System", "Policy"]
                .iter()
                .map(|n| crate::ontology::EntityTypeDef {
                    name: n.to_string(),
                    description: None,
                    parent: None,
                    identity: false,
                    extract: true,
                })
                .collect();
        let ancestor_map = crate::ontology::compute_ancestor_map(&entity_types);
        Ontology {
            mode,
            entity_types,
            relation_types: vec![
                rel("APPROVES", &["Person"], &[], &["APPROVED_BY"], false),
                rel(
                    "OWNS",
                    &["Person", "Team"],
                    &["Project", "System"],
                    &[],
                    false,
                ),
                rel("GOVERNS", &[], &["Policy"], &[], false),
                rel("KNOWS", &[], &[], &[], true),
                rel("USES", &[], &[], &[], false),
            ],
            ancestor_map,
        }
    }

    #[test]
    fn edge_prompt_defines_direction_in_every_mode() {
        for prompt in [
            edge_system_prompt(None),
            edge_system_prompt(Some(&direction_ontology(OntologyMode::Open))),
            edge_system_prompt(Some(&direction_ontology(OntologyMode::Strict))),
        ] {
            assert!(prompt.contains("*as its type name reads*"), "{prompt}");
            assert!(
                prompt.contains("Sable Core is owned by Lena Moreau"),
                "{prompt}"
            );
            assert!(prompt.contains("Lena Moreau OWNS Sable Core"), "{prompt}");
        }
    }

    #[test]
    fn signatures_render_half_and_list_forms() {
        for mode in [OntologyMode::Open, OntologyMode::Strict] {
            let prompt = edge_system_prompt(Some(&direction_ontology(mode)));
            assert!(prompt.contains("- APPROVES (Person → any)"), "{prompt}");
            assert!(
                prompt.contains("- OWNS (Person | Team → Project | System)"),
                "{prompt}"
            );
            assert!(prompt.contains("- GOVERNS (any → Policy)"), "{prompt}");
            assert!(
                prompt.contains("- USES\n"),
                "no signature, no parens: {prompt}"
            );
            assert!(prompt.contains("- KNOWS [symmetric]"), "{prompt}");
            assert!(prompt.contains("order of source_name and target_name does not matter"));
        }
    }

    #[test]
    fn inverse_aliases_render_in_strict_mode_only() {
        let strict = edge_system_prompt(Some(&direction_ontology(OntologyMode::Strict)));
        assert!(
            strict.contains("APPROVED_BY = APPROVES with source and target swapped"),
            "{strict}"
        );
        let open = edge_system_prompt(Some(&direction_ontology(OntologyMode::Open)));
        assert!(!open.contains("APPROVED_BY"), "{open}");
    }

    // ── #823: literal-object edges ───────────────────────────────────────────

    const NO_ONTOLOGY_GOLDEN: &str = include_str!("testdata/extract_edges_no_ontology.golden.txt");

    // FR-006: the free-form edge prompt is byte-identical to the pre-#823 rendering, and so is
    // the rendering for an ontology that offers no extractable relation type.
    #[test]
    fn no_ontology_edge_prompt_matches_golden() {
        assert_eq!(edge_system_prompt(None), NO_ONTOLOGY_GOLDEN);
        assert!(!literal_object_edges_enabled(None));
        let mut onto = assert_only_ontology(OntologyMode::Strict, true);
        for rt in &mut onto.relation_types {
            rt.extract = false;
        }
        assert!(!literal_object_edges_enabled(Some(&onto)));
        assert_eq!(edge_system_prompt(Some(&onto)), NO_ONTOLOGY_GOLDEN);
    }

    #[test]
    fn ontology_edge_prompt_carries_literal_object_rule_in_both_modes() {
        for mode in [OntologyMode::Open, OntologyMode::Strict] {
            let onto = direction_ontology(mode);
            assert!(literal_object_edges_enabled(Some(&onto)));
            let prompt = edge_system_prompt(Some(&onto));
            assert!(prompt.contains("set BOTH source_name and target_name to that entity"));
            assert!(prompt.contains("source_name: Marlin Core, target_name: Marlin Core"));
            assert!(prompt.contains("NEVER attach such a fact to another listed entity"));
            assert!(prompt.contains("Tomas Kowalski and Marlin Core"));
            assert!(prompt.contains("or state a literal value about a single entity"));
            // The contradictory free-form rules must not survive alongside the new rule.
            assert!(
                !prompt.contains("NEVER refer to the same entity"),
                "{prompt}"
            );
            assert!(!prompt.contains("NEVER extract facts that describe only a single entity"));
            assert!(
                !prompt.contains("{{"),
                "no unrendered placeholder: {prompt}"
            );
        }
    }

    #[test]
    fn undeclared_signature_types_are_not_rendered() {
        let mut onto = direction_ontology(OntologyMode::Open);
        onto.relation_types = vec![rel("LIKES", &["Ghost"], &["Person"], &[], false)];
        let prompt = edge_system_prompt(Some(&onto));
        assert!(prompt.contains("- LIKES (any → Person)"), "{prompt}");
        assert!(!prompt.contains("Ghost"), "{prompt}");
    }
}
