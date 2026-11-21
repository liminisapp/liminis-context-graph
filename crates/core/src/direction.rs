//! Edge direction check (#816, ADR-0816).
//!
//! Pure orientation logic shared by ingest (`episode.rs`) and the offline repair pass
//! (`canonicalize.rs`) so the two cannot drift apart. It compares an edge's endpoint entity
//! types against the relation's declared signature and says whether to keep, swap or flag it.
//! It never drops anything.

use crate::ontology::{Ontology, RelationTypeDef};

/// What to do with an edge after comparing its endpoints with the relation signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Nothing to evaluate: symmetric / no signature / an untyped endpoint.
    Skip,
    /// Endpoints fit the signature as extracted (including when both orientations fit).
    Keep,
    /// Endpoints fit only when reversed.
    Swap,
    /// Endpoints fit in neither direction — store unchanged, mark `signature_mismatch`.
    Mismatch,
}

/// Entity types of an endpoint, from its stored `labels` (`["Entity", ancestors…, type]`).
///
/// Drops the implicit `Entity` root and the strict-mode `Unclassified` catch-all. Returns
/// `None` when nothing remains, i.e. the endpoint is untyped and cannot be evaluated. Labels
/// already carry ancestors, so a signature type matches by label-set intersection.
pub fn endpoint_types(labels: &[String]) -> Option<Vec<String>> {
    let types: Vec<String> = labels
        .iter()
        .filter(|l| !matches!(l.as_str(), "Entity" | "Unclassified" | ""))
        .cloned()
        .collect();
    if types.is_empty() {
        None
    } else {
        Some(types)
    }
}

/// True when a side declared as `declared` accepts an endpoint of `types`. An empty
/// declaration (unset side) accepts anything.
fn fits(declared: &[&str], types: &[String]) -> bool {
    declared.is_empty() || declared.iter().any(|d| types.iter().any(|t| t == d))
}

/// Checks an edge of relation `rt` from `src` to `dst` against its declared signature.
///
/// `src`/`dst` are the endpoint type sets from [`endpoint_types`] (`None` = untyped).
/// Rules: symmetric relations, relations whose effective signature is empty on both sides, and
/// edges with an untyped endpoint are skipped. If the forward orientation fits (which includes
/// both orientations fitting) the edge is kept; if only the reverse fits it is swapped;
/// otherwise it is a mismatch.
pub fn check_orientation(
    onto: &Ontology,
    rt: &RelationTypeDef,
    src: Option<&[String]>,
    dst: Option<&[String]>,
) -> Orientation {
    if rt.symmetric {
        return Orientation::Skip;
    }
    let src_decl = onto.effective_source_types(rt);
    let tgt_decl = onto.effective_target_types(rt);
    if src_decl.is_empty() && tgt_decl.is_empty() {
        return Orientation::Skip;
    }
    let (Some(src), Some(dst)) = (src, dst) else {
        return Orientation::Skip;
    };
    if fits(&src_decl, src) && fits(&tgt_decl, dst) {
        Orientation::Keep
    } else if fits(&src_decl, dst) && fits(&tgt_decl, src) {
        Orientation::Swap
    } else {
        Orientation::Mismatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ontology::{compute_ancestor_map, EntityTypeDef, OntologyMode};

    fn et(name: &str, parent: Option<&str>) -> EntityTypeDef {
        EntityTypeDef {
            name: name.into(),
            description: None,
            parent: parent.map(String::from),
            identity: false,
            extract: true,
        }
    }

    fn rt(name: &str, src: &[&str], tgt: &[&str], symmetric: bool) -> RelationTypeDef {
        RelationTypeDef {
            name: name.into(),
            description: None,
            source_types: src.iter().map(|s| s.to_string()).collect(),
            target_types: tgt.iter().map(|s| s.to_string()).collect(),
            inverse_aliases: vec![],
            symmetric,
            aliases: vec![],
            keywords: vec![],
            extract: true,
        }
    }

    fn onto(relations: Vec<RelationTypeDef>) -> Ontology {
        let entity_types = vec![
            et("Person", None),
            et("Employee", Some("Person")),
            et("Policy", None),
            et("Project", None),
            et("Topic", None),
        ];
        let ancestor_map = compute_ancestor_map(&entity_types);
        Ontology {
            mode: OntologyMode::Strict,
            entity_types,
            relation_types: relations,
            ancestor_map,
        }
    }

    fn labels(l: &[&str]) -> Option<Vec<String>> {
        endpoint_types(&l.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn check(o: &Ontology, src: &[&str], dst: &[&str]) -> Orientation {
        let s = labels(src);
        let d = labels(dst);
        check_orientation(o, &o.relation_types[0], s.as_deref(), d.as_deref())
    }

    #[test]
    fn kept_when_forward_fits() {
        let o = onto(vec![rt("APPROVES", &["Person"], &[], false)]);
        assert_eq!(
            check(&o, &["Entity", "Person"], &["Entity", "Policy"]),
            Orientation::Keep
        );
    }

    #[test]
    fn swapped_when_only_reverse_fits() {
        let o = onto(vec![rt("APPROVES", &["Person"], &[], false)]);
        assert_eq!(
            check(&o, &["Entity", "Policy"], &["Entity", "Person"]),
            Orientation::Swap
        );
    }

    #[test]
    fn mismatch_when_neither_fits() {
        let o = onto(vec![rt("APPROVES", &["Person"], &["Policy"], false)]);
        assert_eq!(
            check(&o, &["Entity", "Project"], &["Entity", "Topic"]),
            Orientation::Mismatch
        );
    }

    #[test]
    fn symmetric_is_skipped() {
        let o = onto(vec![rt("KNOWS", &["Person"], &["Policy"], true)]);
        assert_eq!(
            check(&o, &["Entity", "Policy"], &["Entity", "Person"]),
            Orientation::Skip
        );
    }

    #[test]
    fn ancestry_matches_through_labels() {
        let o = onto(vec![rt("APPROVES", &["Person"], &["Policy"], false)]);
        // Employee's stored labels include its ancestor Person.
        assert_eq!(
            check(&o, &["Entity", "Person", "Employee"], &["Entity", "Policy"]),
            Orientation::Keep
        );
        assert_eq!(
            check(&o, &["Entity", "Policy"], &["Entity", "Person", "Employee"]),
            Orientation::Swap
        );
    }

    #[test]
    fn list_signature_accepts_any_member() {
        let o = onto(vec![rt("OWNS", &["Person"], &["Project", "Policy"], false)]);
        assert_eq!(check(&o, &["Person"], &["Policy"]), Orientation::Keep);
        assert_eq!(check(&o, &["Project"], &["Person"]), Orientation::Swap);
    }

    #[test]
    fn unset_signature_and_untyped_endpoint_are_skipped() {
        let o = onto(vec![rt("USES", &[], &[], false)]);
        assert_eq!(check(&o, &["Person"], &["Policy"]), Orientation::Skip);
        let o = onto(vec![rt("APPROVES", &["Person"], &[], false)]);
        assert_eq!(check(&o, &["Entity"], &["Person"]), Orientation::Skip);
        assert_eq!(
            check(&o, &["Entity", "Unclassified"], &["Person"]),
            Orientation::Skip
        );
    }

    #[test]
    fn both_orientations_fit_keeps_extracted_order() {
        let o = onto(vec![rt("MENTORS", &["Person"], &["Person"], false)]);
        assert_eq!(check(&o, &["Person"], &["Person"]), Orientation::Keep);
    }

    #[test]
    fn undeclared_signature_side_is_ignored() {
        // Ghost is undeclared: the source side behaves as unset, so only the target is checked.
        let o = onto(vec![rt("LIKES", &["Ghost"], &["Policy"], false)]);
        assert_eq!(check(&o, &["Topic"], &["Policy"]), Orientation::Keep);
        assert_eq!(check(&o, &["Policy"], &["Topic"]), Orientation::Swap);
        // Fully undeclared signature → nothing to check.
        let o = onto(vec![rt("LIKES", &["Ghost"], &["Ghost"], false)]);
        assert_eq!(check(&o, &["Topic"], &["Policy"]), Orientation::Skip);
    }
}
