//! Integration tests for `knowledge_walk` and `knowledge_get_ontology` (issue #820, ADR-0820):
//! the IPC plumbing over a real DB with a scripted embedder. The algorithm itself (direction,
//! type filters, completion hop, …) is unit-tested on an in-memory graph in `walk.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use futures::future::BoxFuture;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::Db,
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{EmbedRole, Embedder},
    error::Error,
    extractor::MockExtractor,
    handlers,
    ipc::IpcRequest,
    ontology::{EntityTypeDef, OntologyMode, RelationTypeDef},
    telemetry::{NoopSink, TelemetrySink},
    EntityRow, Ontology, RelatesToEdge,
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

const DIM: usize = 4;
const GROUP: &str = "g";

/// Hop phrases / queries embed by keyword so cosines are exact.
struct Scripted;

fn vector_for(text: &str) -> Vec<f32> {
    let t = text.to_lowercase();
    if t.contains("owns") {
        vec![1.0, 0.0, 0.0, 0.0]
    } else if t.contains("supplier") {
        vec![0.0, 1.0, 0.0, 0.0]
    } else {
        vec![0.0, 0.0, 0.0, 1.0]
    }
}

impl Embedder for Scripted {
    fn embed<'a>(
        &'a self,
        text: &'a str,
        _role: EmbedRole,
    ) -> BoxFuture<'a, Result<Vec<f32>, Error>> {
        Box::pin(async move { Ok(vector_for(text)) })
    }

    fn dim(&self) -> usize {
        DIM
    }
}

fn make_state(
    db: Option<Arc<Db>>,
    ontology: Option<Arc<Ontology>>,
    wal_root: Option<std::path::PathBuf>,
) -> Arc<AppState> {
    let degraded = db.is_none();
    let sink: Arc<dyn TelemetrySink> = Arc::new(NoopSink);
    Arc::new(AppState {
        db: ArcSwapOption::from(db),
        degraded_reason: Arc::new(Mutex::new(degraded.then(|| "test-degraded".to_string()))),
        embedder: Arc::new(Scripted),
        extractor: Arc::new(MockExtractor),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(tokio::sync::RwLock::new(())),
        sink,
        db_path: "test.db".to_string(),
        wal_root,
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
        ontology,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        search_stages: Default::default(),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
    })
}

fn ent(uuid: &str, name: &str, ty: &str) -> EntityRow {
    EntityRow {
        uuid: uuid.into(),
        name: name.into(),
        group_id: GROUP.into(),
        labels: vec!["Entity".into(), ty.into()],
        kind: ty.into(),
        created_at: "2024-01-01 00:00:00".into(),
        name_embedding: vec![1.0, 0.0, 0.0, 0.0],
        ..Default::default()
    }
}

fn edge(uuid: &str, src: &str, dst: &str, rel: &str, emb: [f32; 4]) -> RelatesToEdge {
    RelatesToEdge {
        uuid: uuid.into(),
        name: rel.into(),
        source_node_uuid: src.into(),
        target_node_uuid: dst.into(),
        group_id: GROUP.into(),
        fact: format!("{src} {rel} {dst}"),
        fact_embedding: emb.to_vec(),
        created_at: "2024-01-01 00:00:00".into(),
        attributes: "{}".into(),
        relation_type: Some(rel.into()),
        ..Default::default()
    }
}

/// Bob —OWNS→ Apollo —SUPPLIED_BY→ Acme, plus an ambiguous name `Alice` (a Person and a Company).
fn make_db() -> (Arc<Db>, TempDir) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("w.db").to_str().unwrap()).unwrap());
    {
        let conn = db.connect().unwrap();
        conn.init_schema(DIM).unwrap();
        conn.create_vector_indexes().unwrap();
        conn.insert_entity(&ent("u-bob", "Bob", "Person")).unwrap();
        conn.insert_entity(&ent("u-apollo", "Apollo", "Project"))
            .unwrap();
        conn.insert_entity(&ent("u-acme", "Acme", "Company"))
            .unwrap();
        conn.insert_entity(&ent("u-alice-p", "Alice", "Person"))
            .unwrap();
        conn.insert_entity(&ent("u-alice-c", "Alice", "Company"))
            .unwrap();
        conn.insert_relates_to_edge(&edge(
            "e1",
            "u-bob",
            "u-apollo",
            "OWNS",
            [1.0, 0.0, 0.0, 0.0],
        ))
        .unwrap();
        conn.insert_relates_to_edge(&edge(
            "e2",
            "u-apollo",
            "u-acme",
            "SUPPLIED_BY",
            [0.0, 1.0, 0.0, 0.0],
        ))
        .unwrap();
        conn.insert_relates_to_edge(&edge(
            "e3",
            "u-alice-p",
            "u-bob",
            "KNOWS",
            [0.0, 0.0, 0.0, 1.0],
        ))
        .unwrap();
    }
    (db, dir)
}

async fn call(state: &Arc<AppState>, method: &str, params: Value) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: method.to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(state), None).await).unwrap()
}

fn two_hop_params() -> Value {
    json!({
        "group_ids": [GROUP],
        "start": "Bob",
        "hops": [
            {"relation_types": ["OWNS"], "phrase": "owns the project", "direction": "out"},
            {"relation_types": ["SUPPLIED_BY"], "phrase": "supplier of the project", "direction": "out"}
        ],
        "answer_type": "Company",
        "query": "who is the supplier of the project Bob owns"
    })
}

fn files_under(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        files_under(&p)
                    } else {
                        1
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn walk_finds_two_hop_chain_over_ipc() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), None, None);
    let v = call(&state, "knowledge_walk", two_hop_params()).await;
    let r = &v["result"];
    assert_eq!(r["found"], true, "{v}");
    assert_eq!(r["start_entity_count"], 1);
    assert_eq!(r["start_entities"][0]["name"], "Bob");
    assert_eq!(r["start_entities"][0]["type"], "Person");
    assert_eq!(r["hops"][0]["edges"][0]["uuid"], "e1");
    assert_eq!(r["hops"][1]["edges"][0]["uuid"], "e2");
    assert_eq!(r["hops"][1]["edges"][0]["target"]["type"], "Company");
    assert_eq!(r["hops"][1]["edges"][0]["direction_matched"], true);
    assert_eq!(r["completion_hop_ran"], false);
    assert_eq!(
        r["facts_text"],
        "- u-bob OWNS u-apollo (Bob [Person] -> Apollo [Project])\n\
         - u-apollo SUPPLIED_BY u-acme (Apollo [Project] -> Acme [Company])"
    );
}

#[tokio::test]
async fn walk_resolves_start_by_uuid_and_walks_from_every_name_match() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), None, None);
    let mut p = two_hop_params();
    p["start"] = json!("u-bob");
    // A non-UUID-shaped uuid string falls back to name resolution and finds nothing.
    let v = call(&state, "knowledge_walk", p).await;
    assert_eq!(v["result"]["found"], false, "{v}");

    let real = uuid::Uuid::new_v4().to_string();
    {
        let db = state.db.load_full().unwrap();
        let conn = db.connect().unwrap();
        let mut e = ent(&real, "Zed", "Person");
        e.group_id = GROUP.into();
        conn.insert_entity(&e).unwrap();
    }
    let v = call(
        &state,
        "knowledge_walk",
        json!({"group_ids": [GROUP], "start": real,
               "hops": [{"phrase": "owns", "direction": "any"}]}),
    )
    .await;
    assert_eq!(v["result"]["found"], true, "{v}");
    assert_eq!(v["result"]["start_entities"][0]["name"], "Zed");
    assert_eq!(v["result"]["dead_end_at"], 1);

    // Ambiguous name: both kinds are start entities.
    let v = call(
        &state,
        "knowledge_walk",
        json!({"group_ids": [GROUP], "start": "Alice",
               "hops": [{"phrase": "knows", "direction": "any"}]}),
    )
    .await;
    assert_eq!(v["result"]["start_entity_count"], 2, "{v}");
    assert_eq!(v["result"]["hops"][0]["edges"][0]["uuid"], "e3");
}

#[tokio::test]
async fn walk_reports_not_found_explicitly() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), None, None);
    let mut p = two_hop_params();
    p["start"] = json!("Nobody");
    let v = call(&state, "knowledge_walk", p).await;
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["result"]["found"], false);
    assert_eq!(v["result"]["start_entity_count"], 0);
    assert!(v["result"]["message"].as_str().unwrap().contains("Nobody"));
}

#[tokio::test]
async fn walk_rejects_invalid_input() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), None, None);
    let base = two_hop_params;
    let cases: Vec<(&str, Value)> = vec![
        ("missing start", {
            let mut p = base();
            p.as_object_mut().unwrap().remove("start");
            p
        }),
        ("bad mode", {
            let mut p = base();
            p["mode"] = json!("bogus");
            p
        }),
        ("bad direction_mode", {
            let mut p = base();
            p["direction_mode"] = json!("bogus");
            p
        }),
        ("bad hop direction", {
            let mut p = base();
            p["hops"][0]["direction"] = json!("sideways");
            p
        }),
        ("hop without phrase", {
            let mut p = base();
            p["hops"][0].as_object_mut().unwrap().remove("phrase");
            p
        }),
        ("hops not an array", {
            let mut p = base();
            p["hops"] = json!("nope");
            p
        }),
        ("empty hops without answer_type+query", {
            let mut p = base();
            p["hops"] = json!([]);
            p["answer_type"] = Value::Null;
            p
        }),
    ];
    for (label, p) in cases {
        let v = call(&state, "knowledge_walk", p).await;
        assert_eq!(v["error"]["code"], -32000, "{label}: {v}");
    }
}

#[tokio::test]
async fn walk_with_empty_hops_runs_only_the_completion_hop() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), None, None);
    let v = call(
        &state,
        "knowledge_walk",
        json!({"group_ids": [GROUP], "start": "Apollo", "hops": [],
               "answer_type": "Company", "query": "supplier"}),
    )
    .await;
    assert_eq!(v["result"]["completion_hop_ran"], true, "{v}");
    assert_eq!(v["result"]["hops"][0]["completion"], true);
    assert_eq!(v["result"]["hops"][0]["edges"][0]["uuid"], "e2");
}

#[tokio::test]
async fn walk_and_get_ontology_are_read_only() {
    let (db, _d) = make_db();
    let wal = TempDir::new().unwrap();
    let state = make_state(Some(db), None, Some(wal.path().to_path_buf()));
    async fn count(state: &Arc<AppState>) -> (Value, Value) {
        let v = call(state, "knowledge_status", json!({})).await;
        (
            v["result"]["entity_count"].clone(),
            v["result"]["relationship_count"].clone(),
        )
    }
    let before = count(&state).await;
    for _ in 0..2 {
        let v = call(&state, "knowledge_walk", two_hop_params()).await;
        assert_eq!(v["result"]["found"], true);
        let v = call(&state, "knowledge_get_ontology", json!({"group_id": GROUP})).await;
        assert!(v.get("result").is_some());
    }
    assert_eq!(count(&state).await, before, "graph unchanged");
    assert_eq!(before.0, json!(5));
    assert_eq!(files_under(wal.path()), 0, "no WAL writes");
}

#[tokio::test]
async fn walk_and_get_ontology_return_degraded_error_when_db_is_down() {
    let state = make_state(None, None, None);
    for m in ["knowledge_walk", "knowledge_get_ontology"] {
        let v = call(&state, m, two_hop_params()).await;
        assert!(v.get("error").is_some(), "{m}: {v}");
    }
}

fn sample_ontology() -> Arc<Ontology> {
    Arc::new(Ontology {
        mode: OntologyMode::Strict,
        entity_types: vec![
            EntityTypeDef {
                name: "Person".into(),
                description: Some("A human".into()),
                parent: None,
                identity: true,
                extract: true,
            },
            EntityTypeDef {
                name: "Employee".into(),
                description: None,
                parent: Some("Person".into()),
                identity: false,
                extract: true,
            },
            EntityTypeDef {
                name: "Asserted".into(),
                description: Some("assert-only".into()),
                parent: None,
                identity: false,
                extract: false,
            },
        ],
        relation_types: vec![
            RelationTypeDef {
                name: "REPORTS_TO".into(),
                description: Some("X reports to Y".into()),
                source_type: Some("Employee".into()),
                target_type: Some("Person".into()),
                aliases: vec!["MANAGED_BY".into()],
                keywords: vec!["report".into()],
                extract: true,
            },
            RelationTypeDef {
                name: "ASSERTED_REL".into(),
                description: None,
                source_type: None,
                target_type: None,
                aliases: vec![],
                keywords: vec![],
                extract: false,
            },
        ],
        ancestor_map: HashMap::new(),
    })
}

#[tokio::test]
async fn get_ontology_round_trips_fields_and_honours_include_asserted() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), Some(sample_ontology()), None);
    let v = call(&state, "knowledge_get_ontology", json!({"group_id": GROUP})).await;
    let r = &v["result"];
    assert_eq!(r["has_ontology"], true, "{v}");
    assert_eq!(r["mode"], "strict");
    let ets = r["entity_types"].as_array().unwrap();
    assert_eq!(ets.len(), 2, "assert-only type omitted by default");
    assert_eq!(ets[0]["name"], "Person");
    assert_eq!(ets[0]["description"], "A human");
    assert_eq!(ets[1]["parent"], "Person");
    let rts = r["relation_types"].as_array().unwrap();
    assert_eq!(rts.len(), 1);
    assert_eq!(rts[0]["name"], "REPORTS_TO");
    assert_eq!(rts[0]["description"], "X reports to Y");
    assert_eq!(rts[0]["source_type"], json!(["Employee"]));
    assert_eq!(rts[0]["target_type"], json!(["Person"]));
    assert_eq!(rts[0]["aliases"], json!(["MANAGED_BY"]));
    assert!(rts[0].get("inverse_aliases").is_some());
    assert!(rts[0].get("symmetric").is_some());

    let v = call(
        &state,
        "knowledge_get_ontology",
        json!({"group_id": GROUP, "include_asserted": true}),
    )
    .await;
    assert_eq!(v["result"]["entity_types"].as_array().unwrap().len(), 3);
    assert_eq!(v["result"]["relation_types"].as_array().unwrap().len(), 2);
    assert_eq!(v["result"]["entity_types"][2]["extract"], false);

    let v = call(
        &state,
        "knowledge_get_ontology",
        json!({"include_asserted": "yes"}),
    )
    .await;
    assert!(
        v.get("error").is_some(),
        "non-boolean include_asserted: {v}"
    );
}

#[tokio::test]
async fn get_ontology_without_an_ontology_is_a_well_formed_free_form_response() {
    let (db, _d) = make_db();
    let state = make_state(Some(db), None, None);
    let v = call(&state, "knowledge_get_ontology", json!({"group_id": GROUP})).await;
    assert!(v.get("error").is_none(), "{v}");
    assert_eq!(v["result"]["has_ontology"], false);
    assert_eq!(v["result"]["mode"], "free-form");
    assert_eq!(v["result"]["entity_types"], json!([]));
    assert_eq!(v["result"]["relation_types"], json!([]));
}
