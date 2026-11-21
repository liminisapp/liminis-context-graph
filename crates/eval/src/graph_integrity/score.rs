//! The pure scoring function of `lcg-eval graph-integrity` (#819): plain structs in, [`Report`] out.
//!
//! No DB, no service, no clock. All maps are ordered, so the same input yields identical JSON.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use lcg_core::types::{EntityRow, RelatesToEdge};

use super::mapping::{Direction, Mapping};
use super::report::*;
use super::rules::{contains_bounded, effective_edge_type, normalize, type_matches, JunkRule};
use super::skeleton::Skeleton;

/// The edge-attributes JSON key under which #816 stores a signature mismatch mark (`true`). Graphs
/// built before #816 carry none; when no edge carries the key the report says marks are absent.
pub const SIGNATURE_MARK_KEY: &str = "signature_mismatch";

/// One ontology relation signature, reduced to what scoring needs. Each side is the set of accepted
/// entity types (#816 lists; ancestry is already in node labels); an empty side accepts anything.
/// A symmetric relation is carried with both sides empty, as the ingest-time check skips it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelSig {
    pub name: String,
    pub source_types: Vec<String>,
    pub target_types: Vec<String>,
}

pub struct ScoreInput<'a> {
    pub skeleton: &'a Skeleton,
    pub mapping: &'a Mapping,
    pub nodes: &'a [EntityRow],
    pub edges: &'a [RelatesToEdge],
    /// `None` = no ontology.
    pub signatures: Option<&'a [RelSig]>,
    pub group: &'a str,
    pub ontology_source: &'a str,
}

fn has_label(labels: &[String], t: &str) -> bool {
    labels.iter().any(|l| l.eq_ignore_ascii_case(t))
}

fn mark_state(attributes: &str) -> (bool, bool) {
    // (key present, mark set)
    let Ok(v) = serde_json::from_str::<serde_json::Value>(attributes) else {
        return (false, false);
    };
    match v.get(SIGNATURE_MARK_KEY) {
        None | Some(serde_json::Value::Null) => (false, false),
        Some(serde_json::Value::Bool(false)) => (true, false),
        Some(_) => (true, true),
    }
}

pub fn score(inp: ScoreInput<'_>) -> Report {
    let sk = inp.skeleton;
    let map = inp.mapping;

    // skeleton entity index by normalised name (first wins)
    let mut ent_idx: HashMap<String, usize> = HashMap::new();
    for (i, e) in sk.entities.iter().enumerate() {
        ent_idx.entry(normalize(&e.name)).or_insert(i);
    }
    // node → skeleton entity; entity → node set
    let mut node_ent: HashMap<&str, usize> = HashMap::new();
    let mut ent_nodes: Vec<Vec<&EntityRow>> = vec![Vec::new(); sk.entities.len()];
    for n in inp.nodes {
        if let Some(&i) = ent_idx.get(&normalize(&n.name)) {
            node_ent.insert(n.uuid.as_str(), i);
            ent_nodes[i].push(n);
        }
    }
    let ent_uuids: Vec<HashSet<&str>> = ent_nodes
        .iter()
        .map(|v| v.iter().map(|n| n.uuid.as_str()).collect())
        .collect();

    // ── typing ──
    let kind_map: HashMap<String, &Vec<String>> = map
        .kinds
        .iter()
        .map(|(k, v)| (normalize(k), &v.node_types))
        .collect();
    let mut found = 0;
    let mut split = 0;
    let mut typed_num = 0;
    let mut typed_den = 0;
    let mut absent = Vec::new();
    let mut unmapped_kinds = BTreeSet::new();
    for (i, e) in sk.entities.iter().enumerate() {
        let nodes = &ent_nodes[i];
        if nodes.is_empty() {
            absent.push(e.name.clone());
        } else {
            found += 1;
        }
        if nodes.len() > 1 {
            split += 1;
        }
        match kind_map.get(&normalize(&e.kind)) {
            None => {
                unmapped_kinds.insert(e.kind.clone());
            }
            Some(expected) => {
                typed_den += 1;
                if nodes.iter().any(|n| type_matches(&n.labels, expected)) {
                    typed_num += 1;
                }
            }
        }
    }
    let typing = Typing {
        entities: sk.entities.len(),
        found: Ratio::new(found, sk.entities.len()),
        split,
        typed: Ratio::new(typed_num, typed_den),
        absent,
        unmapped_kinds: unmapped_kinds.into_iter().collect(),
    };

    // ── edge indexes ──
    let edge_facts: Vec<String> = inp.edges.iter().map(|e| normalize(&e.fact)).collect();
    let edge_types: Vec<String> = inp
        .edges
        .iter()
        .map(|e| normalize(effective_edge_type(e.relation_type.as_deref(), &e.name)))
        .collect();
    let mut incident: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in inp.edges.iter().enumerate() {
        incident
            .entry(e.source_node_uuid.as_str())
            .or_default()
            .push(i);
        if e.target_node_uuid != e.source_node_uuid {
            incident
                .entry(e.target_node_uuid.as_str())
                .or_default()
                .push(i);
        }
    }

    // ── links and value facts ──
    let link_map: HashMap<String, (&String, &super::mapping::LinkMap)> = map
        .links
        .iter()
        .map(|(a, l)| (normalize(a), (a, l)))
        .collect();
    let mut link_seen: HashSet<(usize, String, usize)> = HashSet::new();
    let mut value_seen: HashSet<(usize, String, String)> = HashSet::new();
    let mut by_attr: BTreeMap<String, LinkCounts> = BTreeMap::new();
    let mut total = LinkCounts::default();
    let mut unmapped_attrs: BTreeSet<String> = BTreeSet::new();
    let mut stated = 0;
    let mut misattached = 0;
    let mut value_total = 0;

    for f in &sk.facts {
        let Some(&subj) = ent_idx.get(&normalize(&f.entity)) else {
            continue;
        };
        let nval = normalize(&f.value);
        let nattr = normalize(&f.attribute);
        if let Some(&obj) = ent_idx.get(&nval) {
            // link-shaped fact
            let Some((attr_key, lm)) = link_map.get(&nattr) else {
                unmapped_attrs.insert(f.attribute.clone());
                continue;
            };
            if !link_seen.insert((subj, nattr.clone(), obj)) {
                continue;
            }
            let c = by_attr.entry((*attr_key).clone()).or_default();
            c.present.den += 1;
            c.typed.den += 1;
            total.present.den += 1;
            total.typed.den += 1;
            let (s, o) = (&ent_uuids[subj], &ent_uuids[obj]);
            let mut present = false;
            let mut typed = false;
            let mut dir_ok = false;
            let mut cands: BTreeSet<usize> = BTreeSet::new();
            for u in s {
                if let Some(v) = incident.get(u) {
                    cands.extend(v.iter().copied());
                }
            }
            for &ei in &cands {
                let e = &inp.edges[ei];
                let fwd = s.contains(e.source_node_uuid.as_str())
                    && o.contains(e.target_node_uuid.as_str());
                let rev = o.contains(e.source_node_uuid.as_str())
                    && s.contains(e.target_node_uuid.as_str());
                if !fwd && !rev {
                    continue;
                }
                present = true;
                for exp in lm
                    .relation_types
                    .iter()
                    .filter(|r| normalize(&r.name) == edge_types[ei])
                {
                    typed = true;
                    let want_fwd = exp.direction == Direction::SubjectToObject;
                    if (want_fwd && fwd) || (!want_fwd && rev) {
                        dir_ok = true;
                    }
                }
            }
            if present {
                c.present.num += 1;
                total.present.num += 1;
            }
            if typed {
                c.typed.num += 1;
                total.typed.num += 1;
                c.direction_correct.den += 1;
                total.direction_correct.den += 1;
                if dir_ok {
                    c.direction_correct.num += 1;
                    total.direction_correct.num += 1;
                }
            }
        } else {
            // literal value fact
            if !value_seen.insert((subj, nattr, nval.clone())) {
                continue;
            }
            value_total += 1;
            let mut is_stated = false;
            let mut is_mis = false;
            let mut cands: BTreeSet<usize> = BTreeSet::new();
            for u in &ent_uuids[subj] {
                if let Some(v) = incident.get(u) {
                    cands.extend(v.iter().copied());
                }
            }
            for ei in cands {
                if !contains_bounded(&edge_facts[ei], &nval) {
                    continue;
                }
                is_stated = true;
                let e = &inp.edges[ei];
                let other = if ent_uuids[subj].contains(e.source_node_uuid.as_str()) {
                    &e.target_node_uuid
                } else {
                    &e.source_node_uuid
                };
                if let Some(&j) = node_ent.get(other.as_str()) {
                    if j != subj
                        && !contains_bounded(&edge_facts[ei], &normalize(&sk.entities[j].name))
                    {
                        is_mis = true;
                    }
                }
            }
            stated += usize::from(is_stated);
            misattached += usize::from(is_mis);
        }
    }
    let links = Links {
        total,
        by_attribute: by_attr,
        unmapped_attributes: unmapped_attrs.into_iter().collect(),
    };
    let value_facts = ValueFacts {
        stated: Ratio::new(stated, value_total),
        misattached: Ratio::new(misattached, value_total),
    };

    // ── junk ──
    let rule = JunkRule::new(
        sk.note_ids.iter().map(String::as_str),
        sk.entities.iter().map(|e| e.name.as_str()),
        map.junk.prefixes.iter().map(String::as_str),
    );
    let mut junk_names: Vec<String> = inp
        .nodes
        .iter()
        .filter(|n| rule.is_junk(&n.name))
        .map(|n| n.name.clone())
        .collect();
    junk_names.sort();
    let junk = Junk {
        nodes: junk_names.len(),
        names: junk_names,
    };

    // ── signature ──
    let mut marks_seen = false;
    let mut marked_total = 0;
    let mut marked_by_type: BTreeMap<String, usize> = BTreeMap::new();
    for e in inp.edges {
        let (present, set) = mark_state(&e.attributes);
        marks_seen |= present;
        if set {
            marked_total += 1;
            *marked_by_type
                .entry(effective_edge_type(e.relation_type.as_deref(), &e.name).to_string())
                .or_default() += 1;
        }
    }
    let fit = inp
        .signatures
        .filter(|s| {
            s.iter()
                .any(|r| !r.source_types.is_empty() || !r.target_types.is_empty())
        })
        .map(|sigs| fit_section(sigs, inp.nodes, inp.edges, &edge_types));
    let signature = Signature {
        mark_key: SIGNATURE_MARK_KEY.to_string(),
        marks_seen,
        marked_total,
        marked_by_type,
        fit,
    };

    Report {
        schema_version: SCHEMA_VERSION,
        inputs: Inputs {
            skeleton_sha256: sk.sha256.clone(),
            mapping_sha256: map.sha256.clone(),
            group: inp.group.to_string(),
            ontology_source: inp.ontology_source.to_string(),
            nodes: inp.nodes.len(),
            edges: inp.edges.len(),
        },
        typing,
        links,
        value_facts,
        junk,
        signature,
        run_meta: None,
    }
}

fn fit_section(
    sigs: &[RelSig],
    nodes: &[EntityRow],
    edges: &[RelatesToEdge],
    edge_types: &[String],
) -> FitSection {
    let labels: HashMap<&str, &Vec<String>> =
        nodes.iter().map(|n| (n.uuid.as_str(), &n.labels)).collect();
    let by_norm: HashMap<String, &RelSig> = sigs.iter().map(|s| (normalize(&s.name), s)).collect();
    let mut by_type: BTreeMap<String, SigCounts> = sigs
        .iter()
        .map(|s| (s.name.clone(), SigCounts::default()))
        .collect();
    let mut undeclared: BTreeMap<String, usize> = BTreeMap::new();
    let ok = |ts: &[String], uuid: &str| {
        ts.is_empty()
            || labels
                .get(uuid)
                .is_some_and(|l| ts.iter().any(|t| has_label(l, t)))
    };
    for (i, e) in edges.iter().enumerate() {
        let Some(sig) = by_norm.get(&edge_types[i]) else {
            *undeclared
                .entry(effective_edge_type(e.relation_type.as_deref(), &e.name).to_string())
                .or_default() += 1;
            continue;
        };
        let c = by_type.get_mut(&sig.name).expect("declared");
        let (s, d) = (e.source_node_uuid.as_str(), e.target_node_uuid.as_str());
        if sig.source_types.is_empty() && sig.target_types.is_empty() {
            c.unconstrained += 1;
        } else if ok(&sig.source_types, s) && ok(&sig.target_types, d) {
            c.fit += 1;
        } else if ok(&sig.source_types, d) && ok(&sig.target_types, s) {
            c.reversed += 1;
        } else {
            c.neither += 1;
        }
    }
    FitSection {
        by_type,
        undeclared_types: undeclared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skeleton(facts: &str) -> Skeleton {
        let t = format!(
            r#"{{"entities":[
              {{"name":"Cedar Core","kind":"project"}},{{"name":"Lena Moreau","kind":"person"}},
              {{"name":"Marlin Core","kind":"project"}},{{"name":"Tomas Kowalski","kind":"person"}},
              {{"name":"Orbit Core","kind":"project"}},{{"name":"Aiko Osei","kind":"person"}},
              {{"name":"Helix Core","kind":"project"}}],
             "notes":[{{"id":"recap-01-aiko-osei","facts":[{facts}]}},{{"id":"digest-047"}}]}}"#
        );
        Skeleton::parse(&t).unwrap()
    }

    fn fact(e: &str, a: &str, v: &str) -> String {
        format!(r#"{{"entity":"{e}","attribute":"{a}","value":"{v}","statement":"s"}}"#)
    }

    fn mapping() -> Mapping {
        Mapping::parse(
            r#"{"mapping_version":1,
            "kinds":{"project":{"node_types":["Project"]},"person":{"node_types":["Person"]}},
            "links":{"Owner":{"relation_types":[
                {"name":"OWNED_BY","direction":"subject_to_object"},
                {"name":"OWNS","direction":"object_to_subject"}]}},
            "junk":{"prefixes":["recap-","digest-"]}}"#,
        )
        .unwrap()
    }

    fn node(uuid: &str, name: &str, label: &str) -> EntityRow {
        EntityRow {
            uuid: uuid.into(),
            name: name.into(),
            labels: vec!["Entity".into(), label.into()],
            ..Default::default()
        }
    }

    fn edge(uuid: &str, ty: &str, src: &str, dst: &str, fact: &str) -> RelatesToEdge {
        RelatesToEdge {
            uuid: uuid.into(),
            name: ty.into(),
            source_node_uuid: src.into(),
            target_node_uuid: dst.into(),
            fact: fact.into(),
            relation_type: Some(ty.into()),
            attributes: "{}".into(),
            ..Default::default()
        }
    }

    fn sigs() -> Vec<RelSig> {
        vec![
            RelSig {
                name: "OWNED_BY".into(),
                source_types: vec!["Project".into()],
                target_types: vec!["Person".into()],
            },
            RelSig {
                name: "HAS_MEASURE".into(),
                source_types: vec![],
                target_types: vec![],
            },
        ]
    }

    /// One reversed link, one misattached value fact, one split entity, one junk node — each
    /// present exactly once — plus correct counterparts.
    fn hand_built() -> (Skeleton, Mapping, Vec<EntityRow>, Vec<RelatesToEdge>) {
        let facts = [
            fact("Cedar Core", "Owner", "Lena Moreau"),
            fact("Marlin Core", "Owner", "Tomas Kowalski"),
            fact("Orbit Core", "Owner", "Aiko Osei"),
            fact("Cedar Core", "Budget", "$5,250"),
            fact("Marlin Core", "Budget", "$4,200"),
            fact("Helix Core", "Launch", "March 2027"),
            // the same fact in a second note de-duplicates
            fact("Cedar Core", "Budget", "$5,250"),
        ]
        .join(",");
        let nodes = vec![
            node("cedar", "Cedar Core", "Project"),
            node("lena", "Lena Moreau", "Person"),
            node("marlin", "Marlin Core", "Project"),
            node("tomas", "Tomas Kowalski", "Person"),
            node("orbit1", "Orbit Core", "Project"),
            node("orbit2", "orbit_core", "Project"), // split entity
            node("aiko", "Aiko Osei", "Person"),
            node("budget", "$5,250 budget", "Measure"),
            node("junk", "recap-01-aiko-osei", "Note"), // junk node
        ];
        let edges = vec![
            edge(
                "e1",
                "OWNED_BY",
                "cedar",
                "lena",
                "Lena Moreau owns Cedar Core",
            ),
            // reversed link: type right, direction wrong
            edge(
                "e2",
                "OWNED_BY",
                "tomas",
                "marlin",
                "Tomas Kowalski owns Marlin Core",
            ),
            // link carried by the second node of the split entity
            edge(
                "e3",
                "OWNED_BY",
                "orbit2",
                "aiko",
                "Aiko Osei owns Orbit Core",
            ),
            // value fact stated on the right entity
            edge(
                "e4",
                "HAS_MEASURE",
                "cedar",
                "budget",
                "Cedar Core budget is $5,250",
            ),
            // misattached value fact: pinned to a different skeleton entity not named in the text
            edge(
                "e5",
                "HAS_MEASURE",
                "marlin",
                "aiko",
                "Marlin Core has a monthly budget of $4,200",
            ),
        ];
        (skeleton(&facts), mapping(), nodes, edges)
    }

    fn run(
        s: &Skeleton,
        m: &Mapping,
        n: &[EntityRow],
        e: &[RelatesToEdge],
        sg: Option<&[RelSig]>,
    ) -> Report {
        score(ScoreInput {
            skeleton: s,
            mapping: m,
            nodes: n,
            edges: e,
            signatures: sg,
            group: "g",
            ontology_source: "explicit",
        })
    }

    #[test]
    fn each_defect_is_counted_exactly_once() {
        let (s, m, n, e) = hand_built();
        let sg = sigs();
        let r = run(&s, &m, &n, &e, Some(&sg));

        // typing: 7 entities, 6 found (Helix absent), 1 split, 6/7 typed
        assert_eq!(r.typing.entities, 7);
        assert_eq!(r.typing.found, Ratio::new(6, 7));
        assert_eq!(r.typing.split, 1);
        assert_eq!(r.typing.typed, Ratio::new(6, 7));
        assert_eq!(r.typing.absent, vec!["Helix Core"]);

        // links: 3 distinct, all present and typed, exactly one reversed
        assert_eq!(r.links.total.present, Ratio::new(3, 3));
        assert_eq!(r.links.total.typed, Ratio::new(3, 3));
        assert_eq!(r.links.total.direction_correct, Ratio::new(2, 3));
        assert_eq!(r.links.by_attribute["Owner"], r.links.total);

        // value facts: 3 distinct; Cedar + Marlin stated, Helix not; exactly one misattached
        assert_eq!(r.value_facts.stated, Ratio::new(2, 3));
        assert_eq!(r.value_facts.misattached, Ratio::new(1, 3));

        // junk
        assert_eq!(r.junk.nodes, 1);
        assert_eq!(r.junk.names, vec!["recap-01-aiko-osei"]);

        // signature: no marks; exactly one reversed edge against OWNED_BY
        assert!(!r.signature.marks_seen);
        assert_eq!(r.signature.marked_total, 0);
        let f = r.signature.fit.as_ref().unwrap();
        assert_eq!(
            f.by_type["OWNED_BY"],
            SigCounts {
                fit: 2,
                reversed: 1,
                neither: 0,
                unconstrained: 0
            }
        );
        assert_eq!(f.by_type["HAS_MEASURE"].unconstrained, 2);
        assert!(f.undeclared_types.is_empty());
        assert!(r.links.unmapped_attributes.is_empty());
        assert!(r.typing.unmapped_kinds.is_empty());
    }

    #[test]
    fn scoring_is_deterministic() {
        let (s, m, n, e) = hand_built();
        let sg = sigs();
        let a = serde_json::to_string(&run(&s, &m, &n, &e, Some(&sg))).unwrap();
        let b = serde_json::to_string(&run(&s, &m, &n, &e, Some(&sg))).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn multi_edge_link_rules() {
        let s = skeleton(&fact("Cedar Core", "Owner", "Lena Moreau"));
        let m = mapping();
        let n = vec![
            node("c", "Cedar Core", "Project"),
            node("l", "Lena Moreau", "Person"),
        ];
        // forward and reversed edge of the expected type: direction counts correct
        let e = vec![
            edge("1", "OWNED_BY", "l", "c", "x"),
            edge("2", "OWNED_BY", "c", "l", "x"),
        ];
        let r = run(&s, &m, &n, &e, None);
        assert_eq!(r.links.total.direction_correct, Ratio::new(1, 1));
        // OWNS from the person is the expected orientation for OWNS
        let e = vec![edge("1", "OWNS", "l", "c", "x")];
        assert_eq!(
            run(&s, &m, &n, &e, None).links.total.direction_correct,
            Ratio::new(1, 1)
        );
        // present by a different type only: present but not typed, no direction denominator
        let e = vec![edge("1", "KNOWS", "c", "l", "x")];
        let r = run(&s, &m, &n, &e, None);
        assert_eq!(r.links.total.present, Ratio::new(1, 1));
        assert_eq!(r.links.total.typed, Ratio::new(0, 1));
        assert_eq!(r.links.total.direction_correct, Ratio::new(0, 0));
        // no edge at all
        let r = run(&s, &m, &n, &[], None);
        assert_eq!(r.links.total.present, Ratio::new(0, 1));
        // absent entity counts as absent, not an error
        let r = run(&s, &m, &n[..1], &[], None);
        assert_eq!(r.typing.found, Ratio::new(1, 7));
        assert_eq!(r.links.total.present, Ratio::new(0, 1));
    }

    #[test]
    fn value_fact_naming_the_other_entity_is_not_misattached() {
        let s = skeleton(&fact("Cedar Core", "Budget", "$5,250"));
        let m = mapping();
        let n = vec![
            node("c", "Cedar Core", "Project"),
            node("l", "Lena Moreau", "Person"),
        ];
        let e = vec![edge(
            "1",
            "HAS_MEASURE",
            "c",
            "l",
            "Lena Moreau set the Cedar Core budget at $5,250",
        )];
        let r = run(&s, &m, &n, &e, None);
        assert_eq!(r.value_facts.stated, Ratio::new(1, 1));
        assert_eq!(r.value_facts.misattached, Ratio::new(0, 1));
        // an edge that does not state the value is neither stated nor misattached
        let e = vec![edge(
            "1",
            "OWNED_BY",
            "c",
            "l",
            "Lena Moreau owns Cedar Core",
        )];
        let r = run(&s, &m, &n, &e, None);
        assert_eq!(r.value_facts.stated, Ratio::new(0, 1));
        assert_eq!(r.value_facts.misattached, Ratio::new(0, 1));
    }

    #[test]
    fn signature_fit_reversed_neither_and_marks() {
        let s = skeleton("");
        let m = mapping();
        let n = vec![
            node("c", "Cedar Core", "Project"),
            node("l", "Lena Moreau", "Person"),
            node("m", "Marlin Core", "Project"),
        ];
        let mut marked = edge("3", "OWNED_BY", "c", "m", "x"); // Project→Project: neither
        marked.attributes = r#"{"signature_mismatch": true}"#.into();
        let mut unmarked = edge("4", "OWNED_BY", "m", "l", "x");
        unmarked.attributes = r#"{"signature_mismatch": false}"#.into();
        let e = vec![
            edge("1", "OWNED_BY", "c", "l", "x"), // fit
            edge("2", "OWNED_BY", "l", "c", "x"), // reversed
            marked,                               // neither
            unmarked,                             // fit, mark key present but false
            edge("5", "FRIENDS_WITH", "c", "m", "x"),
        ];
        let sg = sigs();
        let r = run(&s, &m, &n, &e, Some(&sg));
        let f = r.signature.fit.as_ref().unwrap();
        assert_eq!(
            f.by_type["OWNED_BY"],
            SigCounts {
                fit: 2,
                reversed: 1,
                neither: 1,
                unconstrained: 0
            }
        );
        assert_eq!(f.undeclared_types["FRIENDS_WITH"], 1);
        assert!(r.signature.marks_seen);
        assert_eq!(r.signature.marked_total, 1);
        assert_eq!(r.signature.marked_by_type["OWNED_BY"], 1);
    }

    #[test]
    fn fit_not_applicable_without_ontology_or_signatures() {
        let (s, m, n, e) = hand_built();
        assert!(run(&s, &m, &n, &e, None).signature.fit.is_none());
        let no_sigs = vec![RelSig {
            name: "X".into(),
            source_types: vec![],
            target_types: vec![],
        }];
        assert!(run(&s, &m, &n, &e, Some(&no_sigs)).signature.fit.is_none());
    }

    #[test]
    fn fit_accepts_any_type_of_a_list_signature() {
        // #816: a side is a list; a node fits when its labels include any listed type.
        let (s, m, _, _) = hand_built();
        let sigs = vec![RelSig {
            name: "OWNS".into(),
            source_types: vec!["Person".into(), "Team".into()],
            target_types: vec!["Project".into()],
        }];
        let n = vec![
            node("p", "Lena Moreau", "Person"),
            node("t", "Core Team", "Team"),
            node("pr", "Cedar Core", "Project"),
        ];
        let e = vec![
            edge("1", "OWNS", "p", "pr", "f"),  // fits via Person
            edge("2", "OWNS", "t", "pr", "f"),  // fits via Team
            edge("3", "OWNS", "pr", "p", "f"),  // reversed
            edge("4", "OWNS", "pr", "pr", "f"), // neither
        ];
        let c = &run(&s, &m, &n, &e, Some(&sigs))
            .signature
            .fit
            .unwrap()
            .by_type["OWNS"];
        assert_eq!((c.fit, c.reversed, c.neither), (2, 1, 1));
    }

    #[test]
    fn unmapped_kind_and_attribute_are_excluded_not_failed() {
        let s = Skeleton::parse(
            r#"{"entities":[{"name":"A","kind":"gadget"},{"name":"B","kind":"person"}],
                "notes":[{"id":"n","facts":[{"entity":"A","attribute":"Friend","value":"B","statement":"s"}]}]}"#,
        )
        .unwrap();
        let m = mapping();
        let n = vec![node("a", "A", "Gadget"), node("b", "B", "Person")];
        let r = run(&s, &m, &n, &[], None);
        assert_eq!(r.typing.typed, Ratio::new(1, 1));
        assert_eq!(r.typing.unmapped_kinds, vec!["gadget"]);
        assert_eq!(r.links.unmapped_attributes, vec!["Friend"]);
        assert_eq!(r.links.total.present, Ratio::new(0, 0));
        assert_eq!(r.value_facts.stated.den, 0);
    }
}
