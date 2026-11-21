//! Direction repair via `knowledge_canonicalize_relations` (#816, ADR-0816): the same
//! swap / `signature_mismatch` rules as ingest, applied to a stored graph.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::MockEmbedder,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    ontology::{compute_ancestor_map, EntityTypeDef, Ontology, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    EntityRow, RelatesToEdge,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DIM: usize = 4;
const TS: &str = "2026-01-01T00:00:00Z";
const GRP: &str = "liminis";

fn etype(name: &str, parent: Option<&str>) -> EntityTypeDef {
    EntityTypeDef {
        name: name.to_string(),
        description: None,
        parent: parent.map(str::to_string),
        identity: false,
        extract: true,
    }
}

fn rtype(
    name: &str,
    src: &[&str],
    tgt: &[&str],
    inverse: &[&str],
    aliases: &[&str],
) -> RelationTypeDef {
    RelationTypeDef {
        name: name.to_string(),
        description: None,
        source_types: src.iter().map(|s| s.to_string()).collect(),
        target_types: tgt.iter().map(|s| s.to_string()).collect(),
        inverse_aliases: inverse.iter().map(|s| s.to_string()).collect(),
        symmetric: false,
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
        keywords: vec![],
        extract: true,
    }
}

fn ontology(with_direction: bool) -> Arc<Ontology> {
    let entity_types = vec![
        etype("Person", None),
        etype("Employee", Some("Person")),
        etype("Policy", None),
        etype("Project", None),
        etype("Topic", None),
    ];
    let ancestor_map = compute_ancestor_map(&entity_types);
    let relation_types = if with_direction {
        vec![
            rtype("APPROVES", &["Person"], &[], &["APPROVED_BY"], &[]),
            rtype("WORKS_ON", &["Person"], &["Project"], &[], &[]),
        ]
    } else {
        vec![rtype("APPROVES", &[], &[], &[], &[])]
    };
    Arc::new(Ontology {
        mode: OntologyMode::Open,
        entity_types,
        relation_types,
        ancestor_map,
    })
}

fn make_state(db: Arc<Db>, onto: Arc<Ontology>) -> Arc<AppState> {
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: Arc::new(MockEmbedder::new(DIM)),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(false)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: Some(onto),
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn entity(name: &str, labels: &[&str]) -> EntityRow {
    EntityRow {
        uuid: Uuid::new_v4().to_string(),
        name: name.to_string(),
        group_id: GRP.to_string(),
        labels: labels.iter().map(|s| s.to_string()).collect(),
        created_at: TS.to_string(),
        name_embedding: vec![1.0, 0.0, 0.0, 0.0],
        summary: format!("summary of {name}"),
        attributes: "{}".to_string(),
        ..Default::default()
    }
}

fn edge(src: &EntityRow, dst: &EntityRow, name: &str, rt: &str, attrs: &str) -> RelatesToEdge {
    RelatesToEdge {
        ingested_at: String::new(),
        uuid: Uuid::new_v4().to_string(),
        name: name.to_string(),
        source_node_uuid: src.uuid.clone(),
        target_node_uuid: dst.uuid.clone(),
        group_id: GRP.to_string(),
        fact: format!("{} {} {}", src.name, rt, dst.name),
        fact_embedding: vec![1.0, 0.0, 0.0, 0.0],
        created_at: TS.to_string(),
        valid_at: None,
        invalid_at: None,
        attributes: attrs.to_string(),
        relation_type: Some(rt.to_string()),
        episode_uuids: vec![],
        source_descriptions: vec![],
        ..Default::default()
    }
}

struct Fixture {
    db: Arc<Db>,
    _dir: TempDir,
    lena: EntityRow,
    policy: EntityRow,
    quill: EntityRow,
    pricing: EntityRow,
}

/// Lena (Person), Lumen Policy (Policy), Quill (Project), Pricing (Topic).
fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    db.connect().unwrap().init_schema(DIM).unwrap();
    let f = Fixture {
        db,
        _dir: dir,
        lena: entity("Lena", &["Entity", "Person"]),
        policy: entity("Lumen Policy", &["Entity", "Policy"]),
        quill: entity("Quill", &["Entity", "Project"]),
        pricing: entity("Pricing", &["Entity", "Topic"]),
    };
    {
        let conn = f.db.connect().unwrap();
        for e in [&f.lena, &f.policy, &f.quill, &f.pricing] {
            conn.insert_entity(e).unwrap();
        }
    }
    f
}

async fn repair(state: &Arc<AppState>, dry_run: bool) -> Value {
    let resp = handlers::dispatch(
        IpcRequest {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: "knowledge_canonicalize_relations".into(),
            params: json!({ "group_id": GRP, "dry_run": dry_run }),
        },
        Arc::clone(state),
        None,
    )
    .await;
    let v = serde_json::to_value(resp).unwrap();
    assert!(v.get("error").is_none() || v["error"].is_null(), "{v}");
    v["result"].clone()
}

/// `(source name, target name, relation_type, name, attributes)` per stored edge.
fn stored(f: &Fixture) -> Vec<(String, String, String, String, Value)> {
    let conn = f.db.connect().unwrap();
    let names: HashMap<String, String> = [&f.lena, &f.policy, &f.quill, &f.pricing]
        .iter()
        .map(|e| (e.uuid.clone(), e.name.clone()))
        .collect();
    let mut out: Vec<_> = conn
        .get_edges_by_group_ids(Some(&[GRP]))
        .unwrap()
        .into_iter()
        .map(|e| {
            (
                names[&e.source_node_uuid].clone(),
                names[&e.target_node_uuid].clone(),
                e.relation_type.unwrap_or_default(),
                e.name,
                serde_json::from_str(&e.attributes).unwrap_or(Value::Null),
            )
        })
        .collect();
    out.sort_by(|a, b| a.3.cmp(&b.3));
    out
}

/// Reversed APPROVES (Policy → Lena), correct APPROVES (Lena → Policy), mismatched WORKS_ON.
fn seed_basic(f: &Fixture) {
    let conn = f.db.connect().unwrap();
    conn.insert_relates_to_edge(&edge(
        &f.policy,
        &f.lena,
        "Lumen Policy → Lena",
        "APPROVES",
        "{}",
    ))
    .unwrap();
    conn.insert_relates_to_edge(&edge(
        &f.lena,
        &f.policy,
        "Lena → Lumen Policy",
        "APPROVES",
        "{}",
    ))
    .unwrap();
    conn.insert_relates_to_edge(&edge(
        &f.quill,
        &f.pricing,
        "Quill → Pricing",
        "WORKS_ON",
        r#"{"original_relation_type":"works on"}"#,
    ))
    .unwrap();
}

#[tokio::test]
async fn dry_run_reports_counts_and_writes_nothing() {
    let f = fixture();
    seed_basic(&f);
    let state = make_state(Arc::clone(&f.db), ontology(true));
    let before = stored(&f);

    let r = repair(&state, true).await;
    assert_eq!(r["dry_run"], true);
    assert_eq!(r["swapped_count"], 1, "{r}");
    assert_eq!(r["signature_mismatch_count"], 1, "{r}");
    assert_eq!(
        r["by_type"]["APPROVES"],
        json!({"checked": 2, "swapped": 1, "mismatched": 0})
    );
    assert_eq!(
        r["by_type"]["WORKS_ON"],
        json!({"checked": 1, "swapped": 0, "mismatched": 1})
    );
    assert_eq!(stored(&f), before, "a dry run must not change the graph");
}

#[tokio::test]
async fn real_run_swaps_marks_preserves_attributes_and_is_idempotent() {
    let f = fixture();
    seed_basic(&f);
    let state = make_state(Arc::clone(&f.db), ontology(true));

    let r = repair(&state, false).await;
    assert_eq!(r["swapped_count"], 1, "{r}");
    assert_eq!(r["signature_mismatch_count"], 1, "{r}");

    let edges = stored(&f);
    assert_eq!(edges.len(), 3, "no edge is dropped");
    for e in edges.iter().filter(|e| e.2 == "APPROVES") {
        assert_eq!(
            (e.0.as_str(), e.1.as_str()),
            ("Lena", "Lumen Policy"),
            "{e:?}"
        );
    }
    let works = edges.iter().find(|e| e.2 == "WORKS_ON").unwrap();
    assert_eq!(
        (works.0.as_str(), works.1.as_str()),
        ("Quill", "Pricing"),
        "unchanged"
    );
    assert_eq!(works.4["signature_mismatch"], true);
    assert_eq!(
        works.4["original_relation_type"], "works on",
        "attributes are merged, not replaced"
    );
    // The swapped edge's ingest-form name follows its orientation (best-effort: lbug may
    // refuse a SET on the FTS-indexed name column, in which case the old name stays).
    assert!(edges.iter().filter(|e| e.2 == "APPROVES").count() == 2);

    // Second run: nothing left to swap, nothing re-marked.
    let again = repair(&state, false).await;
    assert_eq!(again["swapped_count"], 0, "{again}");
    assert_eq!(again["by_type"]["APPROVES"]["swapped"], 0);
    assert_eq!(
        stored(&f),
        edges,
        "a repaired graph is unchanged by a second run"
    );
}

#[tokio::test]
async fn inverse_alias_edge_is_retyped_and_swapped_in_one_pass_then_stable() {
    let f = fixture();
    let conn = f.db.connect().unwrap();
    // Stored as the raw inverse label, endpoints in the sentence orientation: Policy → Lena.
    conn.insert_relates_to_edge(&edge(
        &f.policy,
        &f.lena,
        "APPROVED_BY",
        "APPROVED_BY",
        "{}",
    ))
    .unwrap();
    let state = make_state(Arc::clone(&f.db), ontology(true));

    let r = repair(&state, false).await;
    assert_eq!(r["swapped_count"], 1, "{r}");
    let edges = stored(&f);
    assert_eq!(edges.len(), 1);
    assert_eq!(
        (edges[0].0.as_str(), edges[0].1.as_str()),
        ("Lena", "Lumen Policy")
    );
    assert_eq!(edges[0].2, "APPROVES");

    // The name still spells the inverse alias, but the retype consumed that reading: no re-swap.
    let again = repair(&state, false).await;
    assert_eq!(again["swapped_count"], 0, "{again}");
    assert_eq!(stored(&f), edges);
}

#[tokio::test]
async fn stale_mismatch_mark_is_cleared_when_the_edge_fits() {
    let f = fixture();
    let conn = f.db.connect().unwrap();
    // Marked mismatched earlier, but Lena → Quill fits WORKS_ON (Person → Project).
    conn.insert_relates_to_edge(&edge(
        &f.lena,
        &f.quill,
        "Lena → Quill",
        "WORKS_ON",
        r#"{"signature_mismatch":true,"original_relation_type":"works on"}"#,
    ))
    .unwrap();
    let state = make_state(Arc::clone(&f.db), ontology(true));

    let r = repair(&state, false).await;
    assert_eq!(r["swapped_count"], 0);
    assert_eq!(r["signature_mismatch_count"], 0);
    let edges = stored(&f);
    assert!(
        edges[0].4.get("signature_mismatch").is_none(),
        "{:?}",
        edges[0]
    );
    assert_eq!(edges[0].4["original_relation_type"], "works on");
}

#[tokio::test]
async fn ontology_without_direction_info_behaves_as_before() {
    let f = fixture();
    seed_basic(&f);
    let state = make_state(Arc::clone(&f.db), ontology(false));
    let before = stored(&f);

    let r = repair(&state, false).await;
    assert_eq!(r["swapped_count"], 0);
    assert_eq!(r["signature_mismatch_count"], 0);
    assert_eq!(r["by_type"], json!({}));
    let after = stored(&f);
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(before.iter()) {
        assert_eq!(
            (&a.0, &a.1),
            (&b.0, &b.1),
            "no endpoint changes without signatures"
        );
    }
}
