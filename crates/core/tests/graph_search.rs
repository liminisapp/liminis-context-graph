//! Issue #797 / ADR-0797: `knowledge_graph_search`, the graph-aware composite read.
//!
//! The fixture is inserted directly through `Conn` so every embedding is chosen by hand: the
//! query embedder is a `NameMapEmbedder` (wrapped in a `CountingEmbedder`), and each entity
//! name, summary, fact and episode gets a vector on a named axis. "Relevant" therefore means
//! "on the query's axis", which keeps every ranking assertion deterministic.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwapOption;
use lcg_core::{
    app_state::{AppState, OntologyDriftState},
    db::{Conn, Db},
    dedup_adapter::PassthroughDedupAdapter,
    embedder::{CountingEmbedder, Embedder, NameMapEmbedder},
    extractor::ConfigurableExtractor,
    handlers,
    ipc::IpcRequest,
    telemetry::NoopSink,
    types::{EntityRow, EpisodicRow, MentionsEdge, RelatesToEdge},
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

const DIM: usize = 8;
const G: &str = "notes";
const H: &str = "elsewhere";

// Axes: what each embedding dimension "means" in this fixture.
const PRICE: usize = 0;
const SUPPLY: usize = 1;
const ROSTER: usize = 2;
const STATUS: usize = 3;
const UPTIME: usize = 4;
const NOISE: usize = 5;
const NAME: usize = 6;
const SUMMARY: usize = 7;

fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0; DIM];
    v[i] = 1.0;
    v
}

const Q_MULTI: &str = "what does Cedar Core's supplier charge?";
const Q_HUB: &str = "Orbit Hub price";
const Q_DECAY: &str = "Delta Seed price";
const Q_PARA: &str = "the initiative covering shift rostering";
const Q_NAME: &str = "Marlin Core status";
const Q_NONE: &str = "zebra quantum";
const Q_BELIEF: &str = "Policy Desk decision";
const Q_ORIGIN: &str = "Kestrel Labs rate";
const Q_LEGACY: &str = "Old Ledger";
const Q_BRIDGE: &str = "Heron Desk price";
const Q_WREN: &str = "Wren Mill price";
const Q_BROOK: &str = "Brook Hall price";

fn query_map() -> HashMap<String, Vec<f32>> {
    let mut m = HashMap::new();
    let mut multi = vec![0.0; DIM];
    multi[PRICE] = 0.8;
    multi[SUPPLY] = 0.3;
    m.insert(Q_MULTI.to_string(), multi);
    for q in [
        Q_HUB, Q_DECAY, Q_BELIEF, Q_ORIGIN, Q_LEGACY, Q_BRIDGE, Q_WREN, Q_BROOK,
    ] {
        m.insert(q.to_string(), axis(PRICE));
    }
    m.insert(Q_PARA.to_string(), axis(ROSTER));
    m.insert(Q_NAME.to_string(), axis(STATUS));
    m.insert(Q_NONE.to_string(), axis(UPTIME));
    m
}

// ── Fixture builders ──────────────────────────────────────────────────────────

fn entity(c: &Conn<'_>, uuid: &str, name: &str, group: &str, summary_axis: usize) {
    c.insert_entity(&EntityRow {
        uuid: uuid.into(),
        name: name.into(),
        group_id: group.into(),
        labels: vec!["Entity".into()],
        created_at: "2026-01-01 00:00:00".into(),
        ingested_at: "2026-01-01 00:00:00".into(),
        name_embedding: axis(NAME),
        summary: format!("{name} entity"),
        summary_embedding: axis(summary_axis),
        attributes: "{}".into(),
        ..Default::default()
    })
    .unwrap();
}

struct Ep<'a> {
    uuid: &'a str,
    content: &'a str,
    emb: Vec<f32>,
    origin: Option<&'a str>,
    ingested: &'a str,
    superseded: Option<(&'a str, &'a str)>, // (at, kind)
    mentions: &'a [&'a str],
}

fn episode(c: &Conn<'_>, e: Ep<'_>) {
    c.insert_episodic(&EpisodicRow {
        uuid: e.uuid.into(),
        name: e.uuid.into(),
        group_id: G.into(),
        created_at: "2026-01-01 00:00:00".into(),
        source: "text".into(),
        source_description: format!("{}.md", e.uuid),
        content: e.content.into(),
        content_embedding: e.emb,
        valid_at: "2026-01-01 00:00:00".into(),
        ingested_at: e.ingested.into(),
        attributes: "{}".into(),
        origin: e.origin.map(str::to_string),
        superseded_at: e.superseded.map(|(at, _)| at.to_string()),
        superseded_by_kind: e.superseded.map(|(_, k)| k.to_string()),
        superseded_by: e.superseded.map(|_| "replacement".to_string()),
        ..Default::default()
    })
    .unwrap();
    for m in e.mentions {
        c.insert_mentions_edge(&MentionsEdge {
            episodic_uuid: e.uuid.into(),
            entity_uuid: m.to_string(),
            group_id: G.into(),
        })
        .unwrap();
    }
}

/// A primary episode in `group`, mentioning `mentions`, with an explicit origin.
fn episode_in(
    c: &Conn<'_>,
    group: &str,
    uuid: &str,
    emb: Vec<f32>,
    origin: Option<&str>,
    mentions: &[&str],
) {
    c.insert_episodic(&EpisodicRow {
        uuid: uuid.into(),
        name: uuid.into(),
        group_id: group.into(),
        created_at: "2026-01-01 00:00:00".into(),
        source: "text".into(),
        source_description: format!("{uuid}.md"),
        content: uuid.into(),
        content_embedding: emb,
        valid_at: "2026-01-01 00:00:00".into(),
        ingested_at: "2026-01-01 00:00:00".into(),
        attributes: "{}".into(),
        origin: origin.map(str::to_string),
        ..Default::default()
    })
    .unwrap();
    for m in mentions {
        c.insert_mentions_edge(&MentionsEdge {
            episodic_uuid: uuid.into(),
            entity_uuid: m.to_string(),
            group_id: group.into(),
        })
        .unwrap();
    }
}

/// `axis(main)` tilted toward `axis(tilt)` by `w` (cosine to `axis(main)` falls as `w` grows).
fn tilted(main: usize, tilt: usize, w: f32) -> Vec<f32> {
    let mut v = axis(main);
    v[tilt] = w;
    v
}

fn simple_ep(c: &Conn<'_>, uuid: &str, content: &str, emb: Vec<f32>, mentions: &[&str]) {
    episode(
        c,
        Ep {
            uuid,
            content,
            emb,
            origin: None,
            ingested: "2026-01-01 00:00:00",
            superseded: None,
            mentions,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn fact(
    c: &Conn<'_>,
    uuid: &str,
    src: &str,
    dst: &str,
    name: &str,
    text: &str,
    emb: Vec<f32>,
    episodes: &[&str],
    ingested: &str,
) {
    c.insert_relates_to_edge(&RelatesToEdge {
        uuid: uuid.into(),
        name: name.into(),
        source_node_uuid: src.into(),
        target_node_uuid: dst.into(),
        group_id: G.into(),
        fact: text.into(),
        fact_embedding: emb,
        created_at: "2026-01-01 00:00:00".into(),
        ingested_at: ingested.into(),
        attributes: "{}".into(),
        episodes: episodes.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    })
    .unwrap();
}

const T0: &str = "2026-01-01 00:00:00";
const T_CORR: &str = "2026-03-01 00:00:00";

/// The whole fixture. `with_derived` adds a derived page restating Kestrel's rate fact.
fn build(c: &Conn<'_>, with_derived: bool) {
    // Multi-hop: Cedar Core -SUPPLIED_BY-> Fable Supply -CHARGES-> Unit Pricing.
    entity(c, "cedar", "Cedar Core", G, SUMMARY);
    entity(c, "fable", "Fable Supply", G, SUMMARY);
    entity(c, "pricing", "Unit Pricing", G, SUMMARY);
    simple_ep(
        c,
        "ep-cedar",
        "Cedar Core is supplied by Fable Supply.",
        axis(SUPPLY),
        &["cedar", "fable"],
    );
    // States the price, never mentions Cedar Core.
    simple_ep(
        c,
        "ep-fable",
        "Fable Supply charges $87 per unit.",
        axis(PRICE),
        &["fable", "pricing"],
    );
    fact(
        c,
        "e-supply",
        "cedar",
        "fable",
        "SUPPLIED_BY",
        "Cedar Core is supplied by Fable Supply",
        axis(SUPPLY),
        &["ep-cedar"],
        T0,
    );
    fact(
        c,
        "e-price",
        "fable",
        "pricing",
        "CHARGES",
        "Fable Supply charges $87 per unit",
        axis(PRICE),
        &["ep-fable"],
        T0,
    );
    // Cross-group: highly relevant, but its far endpoint lives in another group.
    entity(c, "foreign", "Foreign Corp", H, SUMMARY);
    simple_ep(
        c,
        "ep-foreign",
        "Foreign Corp pricing memo.",
        axis(NOISE),
        &["cedar"],
    );
    fact(
        c,
        "e-cross",
        "cedar",
        "foreign",
        "PRICED_BY",
        "Cedar Core is priced by Foreign Corp",
        axis(PRICE),
        &["ep-foreign"],
        T0,
    );

    // Paraphrase / exact name: Marlin Core's summary is about scheduling.
    entity(c, "marlin", "Marlin Core", G, ROSTER);
    let mut marlin_emb = axis(STATUS);
    marlin_emb[ROSTER] = 0.5;
    simple_ep(
        c,
        "ep-marlin",
        "Marlin Core rollout notes.",
        marlin_emb,
        &["marlin"],
    );

    // No entity at all: an uptime note mentioning nothing.
    simple_ep(
        c,
        "ep-uptime",
        "Uptime was 99.9% last quarter.",
        axis(UPTIME),
        &[],
    );

    // Hub: one relevant edge among many unrelated ones.
    entity(c, "orbit", "Orbit Hub", G, SUMMARY);
    entity(c, "target", "Target Node", G, SUMMARY);
    simple_ep(
        c,
        "ep-orbit-price",
        "Orbit Hub price sheet.",
        axis(PRICE),
        &["orbit", "target"],
    );
    fact(
        c,
        "e-orbit-price",
        "orbit",
        "target",
        "PRICED_AT",
        "Orbit Hub is priced at target",
        axis(PRICE),
        &["ep-orbit-price"],
        T0,
    );
    for i in 1..=5 {
        let s = format!("spoke{i}");
        entity(c, &s, &format!("Spoke {i}"), G, SUMMARY);
        let ep = format!("ep-{s}");
        simple_ep(
            c,
            &ep,
            "Orbit Hub links somewhere.",
            axis(NOISE),
            &["orbit", s.as_str()],
        );
        fact(
            c,
            &format!("e-{s}"),
            "orbit",
            &s,
            "LINKS",
            &format!("Orbit Hub links to Spoke {i}"),
            axis(NOISE),
            &[ep.as_str()],
            T0,
        );
    }

    // Hop decay: Delta Seed -> Near Point (relevant, hop 1); Delta -> Bridge -> Far (hop 2).
    entity(c, "delta", "Delta Seed", G, SUMMARY);
    entity(c, "near", "Near Point", G, SUMMARY);
    entity(c, "bridge", "Bridge Point", G, SUMMARY);
    entity(c, "far", "Far Point", G, SUMMARY);
    simple_ep(c, "ep-near", "near", axis(NOISE), &["delta", "near"]);
    simple_ep(c, "ep-bridge", "bridge", axis(NOISE), &["delta", "bridge"]);
    simple_ep(c, "ep-far", "far", axis(NOISE), &["bridge", "far"]);
    fact(
        c,
        "e-near",
        "delta",
        "near",
        "QUOTES",
        "Delta Seed quotes a cost at Near Point",
        axis(PRICE),
        &["ep-near"],
        T0,
    );
    fact(
        c,
        "e-bridge",
        "delta",
        "bridge",
        "USES",
        "Delta Seed uses Bridge Point",
        axis(SUPPLY),
        &["ep-bridge"],
        T0,
    );
    fact(
        c,
        "e-far",
        "bridge",
        "far",
        "QUOTES",
        "Bridge Point quotes a cost at Far Point",
        axis(PRICE),
        &["ep-far"],
        T0,
    );

    // Belief: a decision corrected later, and one superseded only in the far future.
    entity(c, "desk", "Policy Desk", G, SUMMARY);
    entity(c, "va", "Vendor A", G, SUMMARY);
    entity(c, "vb", "Vendor B", G, SUMMARY);
    entity(c, "vc", "Vendor C", G, SUMMARY);
    episode(
        c,
        Ep {
            uuid: "ep-old",
            content: "Policy Desk chose Vendor A.",
            emb: axis(PRICE),
            origin: None,
            ingested: T0,
            superseded: Some((T_CORR, "episode")),
            mentions: &["desk", "va"],
        },
    );
    episode(
        c,
        Ep {
            uuid: "ep-new",
            content: "Correction: Policy Desk chose Vendor B.",
            emb: axis(PRICE),
            origin: None,
            ingested: T_CORR,
            superseded: None,
            mentions: &["desk", "vb"],
        },
    );
    episode(
        c,
        Ep {
            uuid: "ep-future",
            content: "Policy Desk also uses Vendor C.",
            emb: axis(PRICE),
            origin: None,
            ingested: T0,
            superseded: Some(("2099-01-01 00:00:00", "document")),
            mentions: &["desk", "vc"],
        },
    );
    fact(
        c,
        "e-old",
        "desk",
        "va",
        "CHOSE",
        "Policy Desk chose Vendor A",
        axis(PRICE),
        &["ep-old"],
        T0,
    );
    fact(
        c,
        "e-new",
        "desk",
        "vb",
        "CHOSE",
        "Policy Desk chose Vendor B",
        axis(PRICE),
        &["ep-new"],
        T_CORR,
    );
    // Reachable only through the superseded e-old (Policy Desk -> Vendor A -> Vendor X), so it
    // tests that the walk never continues past a fact no longer believed.
    entity(c, "vx", "Vendor X", G, SUMMARY);
    simple_ep(
        c,
        "ep-vx",
        "Vendor A resells through Vendor X.",
        axis(NOISE),
        &["va", "vx"],
    );
    fact(
        c,
        "e-vx",
        "va",
        "vx",
        "RESELLS_VIA",
        "Vendor A resells through Vendor X at list price",
        axis(PRICE),
        &["ep-vx"],
        T0,
    );
    fact(
        c,
        "e-future",
        "desk",
        "vc",
        "USES",
        "Policy Desk uses Vendor C",
        axis(PRICE),
        &["ep-future"],
        T0,
    );

    // Origin: a primary rate fact, optionally restated by a derived page.
    entity(c, "kestrel", "Kestrel Labs", G, SUMMARY);
    entity(c, "ratecard", "Rate Card", G, SUMMARY);
    simple_ep(
        c,
        "ep-kp",
        "Kestrel Labs rate card: $40/h.",
        axis(PRICE),
        &["kestrel", "ratecard"],
    );
    fact(
        c,
        "e-k1",
        "kestrel",
        "ratecard",
        "HAS_RATE",
        "Kestrel Labs has rate $40/h",
        axis(PRICE),
        &["ep-kp"],
        T0,
    );
    if with_derived {
        episode(
            c,
            Ep {
                uuid: "ep-kd",
                content: "Summary: Kestrel Labs charges $40/h.",
                emb: axis(PRICE),
                origin: Some("derived"),
                ingested: T0,
                superseded: None,
                mentions: &["kestrel", "ratecard"],
            },
        );
        fact(
            c,
            "e-k2",
            "kestrel",
            "ratecard",
            "HAS_RATE",
            "Kestrel Labs has rate $40/h",
            axis(PRICE),
            &["ep-kd"],
            T0,
        );
    }

    // Derived-only bridge: Heron Desk -> Quill Partner is stated only by an agent-written page.
    // Quill Partner's own primary note never mentions Heron Desk.
    entity(c, "heron", "Heron Desk", G, SUMMARY);
    entity(c, "quill", "Quill Partner", G, SUMMARY);
    episode(
        c,
        Ep {
            uuid: "ep-hd",
            content: "Summary: Heron Desk buys from Quill Partner.",
            emb: axis(NOISE),
            origin: Some("derived"),
            ingested: T0,
            superseded: None,
            mentions: &["heron", "quill"],
        },
    );
    simple_ep(
        c,
        "ep-quill",
        "Quill Partner quote sheet.",
        axis(NOISE),
        &["quill"],
    );
    fact(
        c,
        "e-heron",
        "heron",
        "quill",
        "BUYS_FROM",
        "Heron Desk buys from Quill Partner at list price",
        axis(PRICE),
        &["ep-hd"],
        T0,
    );

    // Corrected primary + live derived: Wren Mill -> Lark Yard was stated by a primary note that
    // has since been corrected away, and is still restated by a live agent-written page. Lark
    // Yard's own primary note never mentions Wren Mill.
    entity(c, "wren", "Wren Mill", G, SUMMARY);
    entity(c, "lark", "Lark Yard", G, SUMMARY);
    episode(
        c,
        Ep {
            uuid: "ep-wren-old",
            content: "Wren Mill sources from Lark Yard.",
            emb: axis(NOISE),
            origin: None,
            ingested: T0,
            superseded: Some((T_CORR, "episode")),
            mentions: &["wren", "lark"],
        },
    );
    episode(
        c,
        Ep {
            uuid: "ep-wren-d",
            content: "Summary: Wren Mill sources from Lark Yard.",
            emb: axis(NOISE),
            origin: Some("derived"),
            ingested: T0,
            superseded: None,
            mentions: &["wren", "lark"],
        },
    );
    simple_ep(
        c,
        "ep-lark",
        "Lark Yard price list.",
        axis(NOISE),
        &["lark"],
    );
    fact(
        c,
        "e-wren",
        "wren",
        "lark",
        "SOURCES_FROM",
        "Wren Mill sources from Lark Yard at list price",
        axis(PRICE),
        &["ep-wren-old", "ep-wren-d"],
        T0,
    );

    // Legacy edge: no producing episode, no warrant.
    entity(c, "ledger", "Old Ledger", G, SUMMARY);
    entity(c, "archive", "Archive Box", G, SUMMARY);
    fact(
        c,
        "e-legacy",
        "ledger",
        "archive",
        "STORED_IN",
        "Old Ledger is stored in Archive Box",
        axis(PRICE),
        &[],
        T0,
    );
}

struct Fx {
    state: Arc<AppState>,
    embedder: Arc<CountingEmbedder>,
    _dir: TempDir,
}

fn fixture_with(populate: bool, with_derived: bool) -> Fx {
    fixture_built(|c| {
        if populate {
            build(c, with_derived);
        }
    })
}

/// A fixture populated by `populate` alone (for tests that need a graph of their own shape).
fn fixture_built(populate: impl FnOnce(&Conn<'_>)) -> Fx {
    fixture_built_with(populate, Arc::new(NameMapEmbedder::new(DIM, query_map())))
}

/// [`fixture_built`] with the query embedder given (it is still wrapped in a `CountingEmbedder`).
fn fixture_built_with(populate: impl FnOnce(&Conn<'_>), inner: Arc<dyn Embedder>) -> Fx {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(Db::open(dir.path().join("t.db").to_str().unwrap()).unwrap());
    {
        let c = db.connect().unwrap();
        c.init_schema(DIM).unwrap();
        c.build_indices_and_constraints().unwrap();
        populate(&c);
    }
    let embedder = Arc::new(CountingEmbedder::new(inner));
    let state = Arc::new(AppState {
        db: ArcSwapOption::from(Some(db)),
        degraded_reason: Arc::new(Mutex::new(None)),
        embedder: embedder.clone(),
        extractor: Arc::new(ConfigurableExtractor::new(vec![])),
        dedup: Arc::new(PassthroughDedupAdapter),
        write_lock: Arc::new(RwLock::new(())),
        sink: Arc::new(NoopSink),
        db_path: "test.db".to_string(),
        wal_root: None,
        wal_max_events_per_file: 10_000,
        wal_max_bytes_per_file: 5 * 1024 * 1024,
        embedding_model: "bge-base-en-v1.5".to_string(),
        wal_writers: Arc::new(Mutex::new(HashMap::new())),
        active_writes: Arc::new(AtomicUsize::new(0)),
        rebuild_jobs: Arc::new(Mutex::new(HashMap::new())),
        workspace_root: None,
        indices_built: Arc::new(AtomicBool::new(true)),
        cancel_token: CancellationToken::new(),
        cancelled_chunks: Arc::new(AtomicUsize::new(0)),
        ontology: None,
        ontology_drift: Arc::new(Mutex::new(OntologyDriftState::default())),
        group_ontologies: Arc::new(Mutex::new(HashMap::new())),
        embedding_cache: Arc::new(lcg_core::EmbeddingCache::new()),
        search_stages: Default::default(),
    });
    Fx {
        state,
        embedder,
        _dir: dir,
    }
}

fn fixture() -> Fx {
    fixture_with(true, true)
}

async fn raw(fx: &Fx, params: Value) -> Value {
    let req = IpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "knowledge_graph_search".to_string(),
        params,
    };
    serde_json::to_value(handlers::dispatch(req, Arc::clone(&fx.state), None).await).unwrap()
}

async fn search(fx: &Fx, params: Value) -> Value {
    let v = raw(fx, params).await;
    assert!(v.get("error").is_none(), "graph search errored: {v}");
    v["result"].clone()
}

fn results(r: &Value) -> &Vec<Value> {
    r["results"].as_array().unwrap()
}

/// `p:<uuid>` / `f:<uuid>` keys of the results, in order.
fn keys(r: &Value) -> Vec<String> {
    results(r)
        .iter()
        .map(|x| match x["kind"].as_str().unwrap() {
            "fact" => format!("f:{}", x["fact"]["uuid"].as_str().unwrap()),
            _ => format!("p:{}", x["passage"]["uuid"].as_str().unwrap()),
        })
        .collect()
}

fn find<'a>(r: &'a Value, key: &str) -> Option<&'a Value> {
    let i = keys(r).iter().position(|k| k == key)?;
    Some(&results(r)[i])
}

fn rank(r: &Value, key: &str) -> Option<usize> {
    keys(r).iter().position(|k| k == key)
}

fn path_edges(x: &Value) -> Vec<String> {
    x["evidence"]["path"]
        .as_array()
        .map(|p| {
            p.iter()
                .map(|s| s["edge_uuid"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Seed scores are normalised so the best seed scores exactly 1.0 and none exceeds it.
fn assert_seed_scores_normalised(r: &Value) {
    let scores: Vec<f64> = r["seeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["score"].as_f64().unwrap())
        .collect();
    if scores.is_empty() {
        return;
    }
    let max = scores.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (max - 1.0).abs() < 1e-12,
        "best seed scores 1.0: {scores:?}"
    );
    assert!(scores.iter().all(|s| *s > 0.0 && *s <= 1.0), "{scores:?}");
}

fn seed_uuids(r: &Value) -> Vec<String> {
    r["seeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["uuid"].as_str().unwrap().to_string())
        .collect()
}

fn list_names(x: &Value) -> Vec<String> {
    x["evidence"]["lists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["list"].as_str().unwrap().to_string())
        .collect()
}

// ── Story 1: multi-hop ────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_hop_reaches_the_note_that_never_names_the_seed() {
    let fx = fixture();
    let r = search(&fx, json!({"query": Q_MULTI, "group_ids": [G]})).await;
    assert_seed_scores_normalised(&r);
    assert!(
        seed_uuids(&r).contains(&"cedar".to_string()),
        "{}",
        r["seeds"]
    );
    let cedar = r["seeds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["uuid"] == "cedar")
        .unwrap();
    assert!(cedar["matched"]
        .as_array()
        .unwrap()
        .contains(&json!("name")));
    assert!(
        !cedar["matched"]
            .as_array()
            .unwrap()
            .contains(&json!("name_vector")),
        "ANN filler below the floor is not reported as a match: {cedar}"
    );

    let note =
        find(&r, "p:ep-fable").unwrap_or_else(|| panic!("ep-fable not in top k: {:?}", keys(&r)));
    assert_eq!(path_edges(note), vec!["e-supply", "e-price"]);
    assert_eq!(note["evidence"]["seeds"][0]["uuid"], "cedar");
    assert_eq!(
        note["evidence"]["supporting_episodes"][0]["source_description"],
        "ep-fable.md"
    );

    let price = find(&r, "f:e-price").expect("the price fact is returned");
    assert_eq!(path_edges(price), vec!["e-supply", "e-price"]);
    assert_eq!(price["evidence"]["path"][1]["hop"], 2);
    assert_eq!(price["evidence"]["path"][0]["from_name"], "Cedar Core");
    assert_eq!(price["evidence"]["path"][1]["to_name"], "Unit Pricing");
    // Walking hop 2 out of Fable Supply meets the hop-1 edge again from its far side. That is
    // the same path doubling back, not a second path to the fact.
    let supply = find(&r, "f:e-supply").expect("the bridging fact is returned");
    assert!(
        !seed_uuids(&r).contains(&"fable".to_string()),
        "Fable Supply is reached, not seeded: {}",
        r["seeds"]
    );
    assert_eq!(supply["evidence"]["alternate_paths"], 0, "{supply}");
    // The price note is the best passage for the question.
    let first_passage = keys(&r).into_iter().find(|k| k.starts_with("p:")).unwrap();
    assert_eq!(first_passage, "p:ep-fable", "{:?}", keys(&r));
}

#[tokio::test]
async fn hub_seed_ranks_the_relevant_edge_above_unrelated_ones() {
    let fx = fixture();
    let r = search(
        &fx,
        // The maximum, so nothing the walk found is cut: every spoke fact is in the response, and
        // the ordering below is checked against each of them rather than vacuously.
        json!({"query": Q_HUB, "group_ids": [G], "num_results": 100}),
    )
    .await;
    let relevant = rank(&r, "f:e-orbit-price").expect("relevant edge returned");
    for i in 1..=5 {
        let other = rank(&r, &format!("f:e-spoke{i}"))
            .unwrap_or_else(|| panic!("spoke {i} fact must be returned: {:?}", keys(&r)));
        assert!(
            relevant < other,
            "relevant edge must outrank spoke {i}: {:?}",
            keys(&r)
        );
    }
}

#[tokio::test]
async fn one_hop_fact_ranks_at_least_as_high_as_an_equally_relevant_two_hop_fact() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_DECAY, "group_ids": [G], "stages": "expand"}),
    )
    .await;
    let near = rank(&r, "f:e-near").expect("hop-1 fact returned");
    let far = rank(&r, "f:e-far").expect("hop-2 fact returned");
    assert!(near < far, "{:?}", keys(&r));
    assert_eq!(
        path_edges(find(&r, "f:e-far").unwrap()),
        vec!["e-bridge", "e-far"]
    );
}

// ── Story 2: paraphrase and exact names ───────────────────────────────────────

#[tokio::test]
async fn role_description_seeds_the_entity_through_its_summary() {
    let fx = fixture();
    let r = search(&fx, json!({"query": Q_PARA, "group_ids": [G]})).await;
    assert_seed_scores_normalised(&r);
    let seeds = r["seeds"].as_array().unwrap();
    assert_eq!(
        seeds.len(),
        1,
        "only the summary match passes the gate: {seeds:?}"
    );
    assert_eq!(seeds[0]["uuid"], "marlin");
    assert!(seeds[0]["matched"]
        .as_array()
        .unwrap()
        .contains(&json!("summary_vector")));
    let note = find(&r, "p:ep-marlin").expect("Marlin's note in the top k");
    assert_eq!(note["evidence"]["seeds"][0]["uuid"], "marlin");
    assert_eq!(note["evidence"]["via"]["kind"], "mention");
}

#[tokio::test]
async fn exact_name_is_seeded_and_reported_as_a_name_match() {
    let fx = fixture();
    let r = search(&fx, json!({"query": Q_NAME, "group_ids": [G]})).await;
    let marlin = r["seeds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["uuid"] == "marlin")
        .expect("Marlin Core seeded");
    assert!(marlin["matched"]
        .as_array()
        .unwrap()
        .contains(&json!("name")));
    assert_eq!(
        r["seeds"][0]["uuid"], "marlin",
        "the exact match is the top seed"
    );
}

#[tokio::test]
async fn query_linking_no_entity_returns_plain_passages() {
    let fx = fixture();
    let r = search(&fx, json!({"query": Q_NONE, "group_ids": [G]})).await;
    assert_eq!(r["stats"]["no_seeds"], true, "{}", r["seeds"]);
    assert!(r["seeds"].as_array().unwrap().is_empty());
    assert!(r["count"].as_u64().unwrap() > 0);
    assert_eq!(keys(&r)[0], "p:ep-uptime");
    for x in results(&r) {
        assert_eq!(x["kind"], "passage");
        assert_eq!(list_names(x), vec!["passages"]);
        assert!(x["evidence"].get("seeds").is_none() && x["evidence"].get("path").is_none());
    }
}

// ── Story 3: belief time ──────────────────────────────────────────────────────

#[tokio::test]
async fn corrected_fact_is_neither_returned_nor_traversed_by_default() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_BELIEF, "group_ids": [G], "num_results": 50}),
    )
    .await;
    assert_eq!(r["stats"]["believed_only"], true);
    assert!(find(&r, "f:e-old").is_none(), "{:?}", keys(&r));
    assert!(find(&r, "p:ep-old").is_none(), "{:?}", keys(&r));
    for x in results(&r) {
        assert!(!path_edges(x).contains(&"e-old".to_string()), "{x}");
    }
    assert!(
        find(&r, "f:e-vx").is_none(),
        "only reachable through the corrected fact: {:?}",
        keys(&r)
    );
    let new = find(&r, "f:e-new").expect("the correction's fact is returned");
    assert_eq!(new["belief"]["state"], "believed");
    // Future-effective document supersession: still believed now (ADR-0707).
    let future = find(&r, "f:e-future").expect("future-superseded fact still believed");
    assert_eq!(future["belief"]["believed"], true);
}

#[tokio::test]
async fn as_of_before_the_correction_flips_which_fact_is_believed() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_BELIEF, "group_ids": [G], "num_results": 50, "as_of": "2026-02-01T00:00:00Z"}),
    )
    .await;
    assert!(find(&r, "f:e-old").is_some(), "{:?}", keys(&r));
    assert!(find(&r, "f:e-new").is_none(), "{:?}", keys(&r));
    assert!(find(&r, "p:ep-new").is_none(), "not yet ingested at as_of");
    // Positive control for the traversal tests below: while e-old is believed, the walk goes
    // through it to Vendor X.
    let vx = find(&r, "f:e-vx")
        .unwrap_or_else(|| panic!("reached through believed e-old: {:?}", keys(&r)));
    assert_eq!(path_edges(vx), vec!["e-old", "e-vx"]);
}

#[tokio::test]
async fn believed_only_false_returns_superseded_facts_with_their_state() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_BELIEF, "group_ids": [G], "num_results": 50, "believed_only": false}),
    )
    .await;
    assert_eq!(r["stats"]["believed_only"], false);
    let old = find(&r, "f:e-old").expect("superseded fact returned when not filtering");
    assert_eq!(old["belief"]["state"], "superseded");
    assert_eq!(old["belief"]["believed"], false);
    assert_eq!(old["belief"]["supersession"]["cause"], "corrected");
    // The superseded passage is returned too, and says so.
    let p = find(&r, "p:ep-old").unwrap_or_else(|| {
        panic!(
            "superseded passage returned when not filtering: {:?}",
            keys(&r)
        )
    });
    assert_eq!(p["belief"]["state"], "superseded");
    assert_eq!(p["belief"]["believed"], false);
    // Returned, but never a traversal step: Vendor X's fact is reachable only through e-old, so
    // it must not come back, and a path may end at e-old but never continue past it.
    assert!(find(&r, "f:e-vx").is_none(), "{:?}", keys(&r));
    for x in results(&r) {
        let path = x["evidence"]["path"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (i, s) in path.iter().enumerate() {
            if s["edge_uuid"] == "e-old" {
                assert_eq!(
                    i + 1,
                    path.len(),
                    "walk continued through superseded e-old: {x}"
                );
            }
        }
    }
}

// ── Story 4: evidence ─────────────────────────────────────────────────────────

#[tokio::test]
async fn every_result_has_evidence_that_resolves_to_real_graph_objects() {
    let fx = fixture();
    let r = search(&fx, json!({"query": Q_MULTI, "group_ids": [G]})).await;
    let db = fx.state.db.load_full().unwrap();
    let c = db.connect().unwrap();
    let mut multi_list = false;
    for x in results(&r) {
        let lists = list_names(x);
        assert!(!lists.is_empty(), "{x}");
        multi_list |= lists.len() > 1;
        let ev = &x["evidence"];
        if let Some(seeds) = ev["seeds"].as_array() {
            let uuids: Vec<String> = seeds
                .iter()
                .map(|s| s["uuid"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(c.get_entities_by_uuids(&uuids).unwrap().len(), uuids.len());
            assert!(seeds
                .iter()
                .all(|s| !s["matched"].as_array().unwrap().is_empty()));
        }
        let path = path_edges(x);
        if !path.is_empty() {
            assert_eq!(c.get_relates_to_by_uuids(&path).unwrap().len(), path.len());
        }
        if let Some(eps) = ev["supporting_episodes"].as_array() {
            let uuids: Vec<String> = eps
                .iter()
                .map(|e| e["uuid"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(
                c.get_episode_passages(&uuids, false).unwrap().len(),
                uuids.len()
            );
        }
        if x["kind"] == "fact"
            && x["fact"]["episodes"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
        {
            assert!(ev["supporting_episodes"].as_array().is_some(), "{x}");
            assert!(!path.is_empty(), "a graph-reached fact names its path");
        }
    }
    assert!(
        multi_list,
        "some result was returned by several lists: {:?}",
        keys(&r)
    );
    let fable = find(&r, "p:ep-fable").unwrap();
    let names = list_names(fable);
    assert!(
        names.contains(&"supporting".into()) && names.contains(&"passages".into()),
        "{names:?}"
    );
}

// ── Story 5: origin ───────────────────────────────────────────────────────────

#[tokio::test]
async fn origin_filter_keeps_derived_content_out_of_every_stage() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_ORIGIN, "group_ids": [G], "num_results": 50, "origins": ["primary", "external"]}),
    )
    .await;
    let s = r.to_string();
    assert!(
        !s.contains("ep-kd"),
        "derived page must not appear anywhere: {s}"
    );
    assert!(
        !s.contains("e-k2"),
        "derived edge must not appear anywhere: {s}"
    );
    assert!(find(&r, "f:e-k1").is_some());
}

#[tokio::test]
async fn derived_restatement_cites_but_does_not_change_the_facts_rank() {
    let with = fixture_with(true, true);
    let without = fixture_with(true, false);
    let params = json!({"query": Q_ORIGIN, "group_ids": [G], "num_results": 50,
                        "as_of": "2026-06-01T00:00:00Z"});
    let a = search(&with, params.clone()).await;
    let b = search(&without, params).await;
    let fa = find(&a, "f:e-k1").expect("primary fact returned");
    let fb = find(&b, "f:e-k1").expect("primary fact returned");
    assert_eq!(
        fa["score"], fb["score"],
        "derived restatement must add no rank"
    );
    assert!(
        find(&a, "f:e-k2").is_none(),
        "a derived-only edge is never ranked"
    );
    assert_eq!(fa["corroboration"], 1);
    let cites: Vec<&Value> = fa["evidence"]["supporting_episodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["uuid"] == "ep-kd")
        .collect();
    assert_eq!(cites.len(), 1, "the derived page is attached as a citation");
    assert_eq!(cites[0]["corroborating"], false);
    assert_eq!(cites[0]["origin"], "derived");
    // Never in the supporting list.
    // With no filter the derived page may surface on its own merit through plain passage search,
    // but never through the supporting list.
    let p = find(&a, "p:ep-kd")
        .unwrap_or_else(|| panic!("derived page surfaces via plain passages: {:?}", keys(&a)));
    assert!(!list_names(p).contains(&"supporting".to_string()), "{p}");
    // Only plain passage search returned it: the vector lists and/or BM25 (#797), never a graph
    // list.
    let names = list_names(p);
    assert!(names.contains(&"passages".to_string()), "{p}");
    assert!(names.iter().all(|n| n.starts_with("passages")), "{p}");
}

#[tokio::test]
async fn derived_only_bridge_is_not_traversed_into_primary_passages() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_BRIDGE, "group_ids": [G], "num_results": 50,
                               "stages": "expand"}),
    )
    .await;
    assert!(
        seed_uuids(&r).contains(&"heron".to_string()),
        "{}",
        r["seeds"]
    );
    // The derived page may not carry the walk to Quill Partner, so Quill's primary note gains
    // no rank from it (ADR-0752: derived content cites, never corroborates).
    assert!(
        find(&r, "p:ep-quill").is_none(),
        "reached only through a derived-only edge: {:?}",
        keys(&r)
    );
    assert!(
        find(&r, "f:e-heron").is_none(),
        "a derived-only edge is never ranked"
    );
    for x in results(&r) {
        assert!(
            !path_edges(x).contains(&"e-heron".to_string()),
            "no result's path crosses the derived-only edge: {x}"
        );
    }

    // Positive control: the same shape with a *primary* bridge note does carry the walk to
    // Quill Partner, so the absences above are the origin rule at work, not an empty walk.
    let ctl = fixture_built(|c| {
        entity(c, "heron", "Heron Desk", G, SUMMARY);
        entity(c, "quill", "Quill Partner", G, SUMMARY);
        simple_ep(
            c,
            "ep-hd",
            "Heron Desk buys from Quill Partner.",
            axis(NOISE),
            &["heron", "quill"],
        );
        simple_ep(
            c,
            "ep-quill",
            "Quill Partner quote sheet.",
            axis(NOISE),
            &["quill"],
        );
        fact(
            c,
            "e-heron",
            "heron",
            "quill",
            "BUYS_FROM",
            "Heron Desk buys from Quill Partner at list price",
            axis(PRICE),
            &["ep-hd"],
            T0,
        );
    });
    let r = search(
        &ctl,
        json!({"query": Q_BRIDGE, "group_ids": [G], "num_results": 50, "stages": "expand"}),
    )
    .await;
    assert!(find(&r, "f:e-heron").is_some(), "{:?}", keys(&r));
    let quill = find(&r, "p:ep-quill")
        .unwrap_or_else(|| panic!("a primary bridge reaches Quill's note: {:?}", keys(&r)));
    assert_eq!(path_edges(quill), vec!["e-heron"]);
}

#[tokio::test]
async fn edge_whose_only_live_producer_is_derived_is_citation_only() {
    let fx = fixture();
    // Believed now only on the derived page's word: its primary producer was corrected away.
    let r = search(
        &fx,
        json!({"query": Q_WREN, "group_ids": [G], "num_results": 50, "stages": "expand"}),
    )
    .await;
    assert!(
        seed_uuids(&r).contains(&"wren".to_string()),
        "{}",
        r["seeds"]
    );
    assert!(
        find(&r, "f:e-wren").is_none(),
        "an edge whose only live producer is derived is never ranked: {:?}",
        keys(&r)
    );
    assert!(
        find(&r, "p:ep-lark").is_none(),
        "it carries the walk nowhere, so Lark Yard's note gains no rank: {:?}",
        keys(&r)
    );
    for x in results(&r) {
        assert!(
            !path_edges(x).contains(&"e-wren".to_string()),
            "no result's path crosses it: {x}"
        );
    }

    // Before the correction the primary producer is live, so the same edge ranks and walks.
    let r = search(
        &fx,
        json!({"query": Q_WREN, "group_ids": [G], "num_results": 50, "stages": "expand",
               "as_of": "2026-02-01T00:00:00Z"}),
    )
    .await;
    assert!(
        find(&r, "f:e-wren").is_some(),
        "with its primary producer live the edge is an ordinary fact: {:?}",
        keys(&r)
    );
    assert!(
        find(&r, "p:ep-lark").is_some(),
        "and it carries the walk to Lark Yard: {:?}",
        keys(&r)
    );
}

#[tokio::test]
async fn plain_passages_of_a_minority_group_are_not_crowded_out_by_other_groups() {
    // 100 passages in another group are all nearer the query than the one passage in the
    // requested group, so an un-oversampled probe (3 × 30 = 90) would see only the other group.
    let fx = fixture_built(|c| {
        for i in 0..100 {
            episode_in(
                c,
                H,
                &format!("ep-h-{i:03}"),
                tilted(UPTIME, NOISE, 0.001 * i as f32),
                None,
                &[],
            );
        }
        episode_in(c, G, "ep-g-target", tilted(UPTIME, NOISE, 1.3), None, &[]);
    });
    let r = search(&fx, json!({"query": Q_NONE, "group_ids": [G]})).await;
    assert_eq!(r["stats"]["no_seeds"], true);
    assert!(
        find(&r, "p:ep-g-target").is_some(),
        "the requested group's passage must survive the group filter: {:?}",
        keys(&r)
    );
    assert!(
        keys(&r).iter().all(|k| !k.starts_with("p:ep-h-")),
        "nothing from the other group: {:?}",
        keys(&r)
    );
}

#[tokio::test]
async fn hub_mentions_are_bounded_ranked_by_relevance_and_filtered_before_the_cut() {
    // Brook Hall is mentioned 111 times: 60 off-topic primary notes, 50 on-topic derived pages,
    // and one on-topic primary note whose uuid sorts last. A per-entity cut in uuid order, or one
    // that let derived pages take slots, would drop that note.
    let fx = fixture_built(|c| {
        entity(c, "brook", "Brook Hall", G, SUMMARY);
        for i in 0..60 {
            episode_in(c, G, &format!("m-{i:02}"), axis(NOISE), None, &["brook"]);
        }
        for i in 0..50 {
            episode_in(
                c,
                G,
                &format!("d-{i:02}"),
                axis(PRICE),
                Some("derived"),
                &["brook"],
            );
        }
        episode_in(c, G, "zz-relevant", axis(PRICE), None, &["brook"]);
    });
    let r = search(
        &fx,
        json!({"query": Q_BROOK, "group_ids": [G], "num_results": 5, "stages": "seeds"}),
    )
    .await;
    assert!(
        seed_uuids(&r).contains(&"brook".to_string()),
        "{}",
        r["seeds"]
    );
    assert_eq!(
        rank(&r, "p:zz-relevant"),
        Some(0),
        "the one on-topic primary note wins: {:?}",
        keys(&r)
    );
    assert!(
        keys(&r).iter().all(|k| !k.starts_with("p:d-")),
        "derived mentions never rank: {:?}",
        keys(&r)
    );
    let cut = r["stats"]["truncations"].as_array().unwrap();
    assert!(
        cut.iter()
            .any(|t| t["cap"] == "mentions_per_entity" && t["limit"] == 50),
        "the per-entity bound is reported: {}",
        r["stats"]
    );
}

#[tokio::test]
async fn a_hub_whose_qualifying_mentions_all_fit_reports_no_cut() {
    // 60 mentions make Brook Hall a hub (more than 50), but 20 are derived pages the ranked
    // query filters out. The 40 that qualify all fit, so nothing was cut and none is reported.
    let fx = fixture_built(|c| {
        entity(c, "brook", "Brook Hall", G, SUMMARY);
        for i in 0..40 {
            episode_in(c, G, &format!("m-{i:02}"), axis(PRICE), None, &["brook"]);
        }
        for i in 0..20 {
            episode_in(
                c,
                G,
                &format!("d-{i:02}"),
                axis(PRICE),
                Some("derived"),
                &["brook"],
            );
        }
    });
    let r = search(
        &fx,
        json!({"query": Q_BROOK, "group_ids": [G], "num_results": 100, "stages": "seeds"}),
    )
    .await;
    assert!(
        seed_uuids(&r).contains(&"brook".to_string()),
        "{}",
        r["seeds"]
    );
    let cut = r["stats"]["truncations"].as_array().unwrap();
    assert!(
        !cut.iter().any(|t| t["cap"] == "mentions_per_entity"),
        "no qualifying mention was dropped, so no cut is reported: {}",
        r["stats"]
    );
    // All 40 primary mentions compete; no derived page does.
    let primary = keys(&r).iter().filter(|k| k.starts_with("p:m-")).count();
    assert_eq!(primary, 40, "{:?}", keys(&r));
    assert!(
        keys(&r).iter().all(|k| !k.starts_with("p:d-")),
        "{:?}",
        keys(&r)
    );
}

// ── Story 7: one model call ───────────────────────────────────────────────────

#[tokio::test]
async fn exactly_one_embed_call_per_request() {
    let fx = fixture();
    let before = fx.embedder.call_count();
    let r = search(&fx, json!({"query": Q_MULTI})).await;
    assert_eq!(fx.embedder.call_count() - before, 1);
    assert_eq!(r["stats"]["embed_calls"], 1);
    assert!(r["stats"]["db_queries"].as_u64().unwrap() > 0);
    assert_eq!(r["stats"]["passage_mode"], "vector_belief");
}

/// #797: with a query instruction in effect, the read embeds the bare and the instructed query in
/// one batch and runs a plain-passage vector list for each. Here the instructed query points
/// nowhere useful (the NOISE axis) and the bare one at the uptime note, so only the bare list can
/// find it.
#[tokio::test]
async fn an_instructed_profile_adds_a_bare_query_list_from_the_same_model_call() {
    use lcg_core::embedder::{InstructedEmbedder, InstructionConfig, InstructionProfile};
    let eff = InstructionConfig {
        profile: InstructionProfile::BgeV15,
        ..Default::default()
    }
    .resolve(None)
    .unwrap();
    let mut map = query_map();
    map.insert(format!("{}{Q_NONE}", eff.query_prefix), axis(NOISE));
    let inner: Arc<dyn Embedder> = Arc::new(InstructedEmbedder::new(
        Arc::new(NameMapEmbedder::new(DIM, map)),
        eff,
    ));
    let fx = fixture_built_with(|c| build(c, true), inner);

    let r = search(&fx, json!({"query": Q_NONE, "group_ids": [G]})).await;
    assert_eq!(fx.embedder.call_count(), 0, "no single-text embed");
    assert_eq!(
        fx.embedder.batch_call_count(),
        1,
        "one batch: one model call"
    );
    assert_eq!(r["stats"]["embed_calls"], 1);
    assert_eq!(r["stats"]["query_vectors"], 2);
    assert_eq!(
        r["stats"]["passage_lists"],
        json!(["passages", "passages_raw", "passages_lexical"])
    );
    let up = find(&r, "p:ep-uptime")
        .unwrap_or_else(|| panic!("the bare-query list finds the uptime note: {:?}", keys(&r)));
    assert!(list_names(up).contains(&"passages_raw".to_string()), "{up}");

    // Without an instruction there is one vector, one `embed`, and no bare list.
    let plain = fixture();
    let r = search(&plain, json!({"query": Q_NONE, "group_ids": [G]})).await;
    assert_eq!(plain.embedder.call_count(), 1);
    assert_eq!(r["stats"]["query_vectors"], 1);
    assert_eq!(
        r["stats"]["passage_lists"],
        json!(["passages", "passages_lexical"])
    );
}

// ── Edge cases ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_graph_returns_an_empty_list() {
    let fx = fixture_with(false, false);
    let r = search(&fx, json!({"query": Q_MULTI})).await;
    assert_eq!(r["count"], 0);
    assert_eq!(r["stats"]["no_seeds"], true);
}

#[tokio::test]
async fn empty_group_list_matches_nothing() {
    let fx = fixture();
    let before = fx.embedder.call_count();
    let r = search(&fx, json!({"query": Q_MULTI, "group_ids": []})).await;
    assert_eq!(r["count"], 0);
    assert_eq!(
        fx.embedder.call_count(),
        before,
        "nothing to search, nothing embedded"
    );
}

#[tokio::test]
async fn invalid_input_is_rejected() {
    let fx = fixture();
    for params in [
        json!({"query": ""}),
        json!({"query": "   "}),
        json!({}),
        json!({"query": "x", "stages": "everything"}),
        json!({"query": "x", "believed_only": "yes"}),
        json!({"query": "x", "origins": []}),
        json!({"query": "x", "num_results": 0}),
        // A wrongly typed `group_ids` must not silently widen the search to every group.
        json!({"query": "x", "group_ids": 5}),
        json!({"query": "x", "group_ids": {"a": 1}}),
        json!({"query": "x", "group_ids": [5]}),
    ] {
        let v = raw(&fx, params.clone()).await;
        assert!(v.get("error").is_some(), "{params} must be rejected: {v}");
    }
}

#[tokio::test]
async fn traversal_never_leaves_the_requested_groups() {
    let fx = fixture();
    let scoped = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "num_results": 50}),
    )
    .await;
    assert!(
        !scoped.to_string().contains("e-cross"),
        "{:?}",
        keys(&scoped)
    );
    let all = search(&fx, json!({"query": Q_MULTI, "num_results": 50})).await;
    assert!(
        find(&all, "f:e-cross").is_some(),
        "unscoped, the edge is reachable"
    );
}

#[tokio::test]
async fn legacy_edge_is_traversable_and_returned_without_a_passage() {
    let fx = fixture();
    let r = search(&fx, json!({"query": Q_LEGACY, "group_ids": [G]})).await;
    let f = find(&r, "f:e-legacy").expect("legacy fact returned");
    assert_eq!(f["belief"]["state"], "legacy");
    assert!(f["evidence"].get("supporting_episodes").is_none(), "{f}");
    assert_eq!(path_edges(f), vec!["e-legacy"]);
}

#[tokio::test]
async fn k_larger_than_the_pool_is_not_padded_and_items_are_unique() {
    let fx = fixture();
    let r = search(
        &fx,
        json!({"query": Q_LEGACY, "group_ids": [G], "num_results": 100}),
    )
    .await;
    let k = keys(&r);
    // Not vacuous: the legacy fact is found, and fewer than k items exist.
    assert!(k.contains(&"f:e-legacy".to_string()), "{k:?}");
    assert!(k.len() < 100);
    assert_eq!(r["count"].as_u64().unwrap() as usize, k.len());
    let mut dedup = k.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), k.len(), "each item appears once");
}

#[tokio::test]
async fn identical_requests_give_identical_results() {
    let fx = fixture();
    let params = json!({"query": Q_MULTI, "as_of": "2026-06-01T00:00:00Z"});
    let mut a = search(&fx, params.clone()).await;
    let mut b = search(&fx, params).await;
    // Wall-clock stage timings are reported, not ranked on; everything else must be byte-equal.
    for r in [&mut a, &mut b] {
        let t = r["stats"]
            .as_object_mut()
            .unwrap()
            .remove("timings_ms")
            .expect("stats.timings_ms is always reported");
        for stage in [
            "embed",
            "seeds",
            "expand",
            "supporting",
            "passages",
            "fusion",
            "total",
        ] {
            assert!(t[stage].as_f64().is_some_and(|v| v >= 0.0), "{stage}: {t}");
        }
    }
    assert_eq!(a.to_string(), b.to_string());
}

#[tokio::test]
async fn explain_reports_every_seed_candidate_and_why() {
    let fx = fixture();
    let plain = search(&fx, json!({"query": Q_MULTI, "group_ids": [G]})).await;
    assert!(
        plain.get("seed_trace").is_none(),
        "the trace is opt-in: {plain}"
    );

    let r = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "explain": true}),
    )
    .await;
    let trace = r["seed_trace"].as_array().expect("seed_trace with explain");
    assert!(!trace.is_empty());
    // Every seed appears in the trace as seeded, with an admitting reason.
    for s in seed_uuids(&r) {
        let c = trace
            .iter()
            .find(|c| c["uuid"] == s.as_str())
            .unwrap_or_else(|| panic!("seed {s} missing from trace"));
        assert_eq!(c["seeded"], true);
        assert!(
            ["exact_name", "bm25_rank", "vector", "summary_vector"]
                .contains(&c["reason"].as_str().unwrap()),
            "{c}"
        );
    }
    // Exactly the seeds are marked seeded; every other candidate names a rejection reason.
    let seeds = seed_uuids(&r);
    for c in trace {
        let seeded = seeds.contains(&c["uuid"].as_str().unwrap().to_string());
        assert_eq!(c["seeded"].as_bool().unwrap(), seeded, "{c}");
        if !seeded {
            assert!(
                [
                    "no_evidence",
                    "merged",
                    "no_row",
                    "origin_filter",
                    "seed_cap"
                ]
                .contains(&c["reason"].as_str().unwrap()),
                "{c}"
            );
        }
    }
    // Asking for the trace changes nothing else.
    assert_eq!(keys(&r), keys(&plain));
    // A non-boolean `explain` is an input error.
    let v = raw(&fx, json!({"query": Q_MULTI, "explain": "yes"})).await;
    assert!(
        v["error"].to_string().contains("explain must be a boolean"),
        "{v}"
    );
}

// ── Ablation ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn stages_switch_the_fused_lists() {
    let fx = fixture();
    let seeds = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "stages": "seeds"}),
    )
    .await;
    assert_eq!(seeds["stats"]["stages"], "seeds");
    assert_eq!(seeds["stats"]["hops"], 0);
    // Not vacuous: Cedar Core is seeded and its own note is returned.
    assert!(
        seed_uuids(&seeds).contains(&"cedar".to_string()),
        "{}",
        seeds["seeds"]
    );
    assert!(find(&seeds, "p:ep-cedar").is_some(), "{:?}", keys(&seeds));
    assert!(results(&seeds)
        .iter()
        .all(|x| x["kind"] == "passage" && list_names(x) == vec!["supporting".to_string()]));
    // Seeds-only cannot reach the price note, which never mentions a seed... unless a seed
    // happens to be mentioned in it; Fable Supply is not a seed here.
    assert!(!seed_uuids(&seeds).contains(&"fable".to_string()));
    assert!(find(&seeds, "p:ep-fable").is_none(), "{:?}", keys(&seeds));

    let expand = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "stages": "expand"}),
    )
    .await;
    assert!(results(&expand)
        .iter()
        .all(|x| !list_names(x).contains(&"passages".to_string())));
    assert!(results(&expand).iter().any(|x| x["kind"] == "fact"));
    assert!(find(&expand, "p:ep-fable").is_some());

    let full = search(&fx, json!({"query": Q_MULTI, "group_ids": [G]})).await;
    assert_eq!(full["stats"]["stages"], "full");
    assert!(results(&full)
        .iter()
        .any(|x| list_names(x).contains(&"passages".to_string())));
}

// ── Rerank stage (#796 on top of the graph read) ──────────────────────────────

fn install_reranker(fx: &Fx, reranker: Option<Arc<lcg_core::rerank::MockReranker>>) {
    use lcg_core::search_stages::{SearchStages, SearchStagesConfig};
    fx.state.set_search_stages(SearchStages::new(
        SearchStagesConfig {
            // The cross-encoder alone orders the head (belief and recency still apply), so these
            // tests pin the stage plumbing; the blend itself is #796's and tested there.
            blend: lcg_core::passage_rerank::BlendConfig::cross_encoder_only(),
            ..Default::default()
        },
        reranker.map(|r| r as Arc<dyn lcg_core::rerank::Reranker>),
        None,
    ));
}

#[tokio::test]
async fn rerank_reorders_the_fused_pool_and_reports_the_stage() {
    let fx = fixture();
    let plain = search(&fx, json!({"query": Q_MULTI, "group_ids": [G]})).await;
    assert!(
        plain["stats"].get("rerank").is_none(),
        "absent unless requested"
    );

    // The cross-encoder loves the uptime note, which plain fusion ranks low or not at all.
    let mock = Arc::new(lcg_core::rerank::MockReranker::new(|_, d| {
        if d.contains("Uptime") {
            10.0
        } else {
            0.0
        }
    }));
    install_reranker(&fx, Some(Arc::clone(&mock)));
    let r = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "rerank": true}),
    )
    .await;
    assert_eq!(mock.calls(), 1, "one cross-encoder call");
    assert_eq!(r["stats"]["rerank"]["applied"], true, "{}", r["stats"]);
    let pooled = r["stats"]["rerank"]["candidates"].as_u64().unwrap();
    assert!(pooled as usize >= keys(&r).len(), "{}", r["stats"]);
    assert!(keys(&r).len() <= 10, "cut back to num_results");
    assert_eq!(keys(&r)[0], "p:ep-uptime", "{:?}", keys(&r));
    assert!(results(&r)[0]["rerank_score"].as_f64().is_some());
    // Facts are scored on `from → to: fact`.
    assert!(
        mock.last_documents()
            .iter()
            .any(|d| d.contains("Fable Supply → Unit Pricing:")),
        "{:?}",
        mock.last_documents()
    );
}

#[tokio::test]
async fn rerank_degrades_to_the_fused_order_when_unconfigured_or_failing() {
    let fx = fixture();
    let plain = keys(&search(&fx, json!({"query": Q_MULTI, "group_ids": [G]})).await);

    let r = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "rerank": true}),
    )
    .await;
    assert_eq!(r["stats"]["rerank"]["applied"], false);
    assert_eq!(r["stats"]["rerank"]["reason"], "not_configured");
    assert_eq!(keys(&r), plain);

    install_reranker(
        &fx,
        Some(Arc::new(lcg_core::rerank::MockReranker::failing())),
    );
    let r = search(
        &fx,
        json!({"query": Q_MULTI, "group_ids": [G], "rerank": true}),
    )
    .await;
    assert_eq!(r["stats"]["rerank"]["reason"], "unavailable");
    assert_eq!(keys(&r), plain, "a failed stage never changes the results");
}

#[tokio::test]
async fn rerank_applies_belief_after_the_cross_encoder() {
    let fx = fixture();
    // The cross-encoder slightly prefers the superseded decision; belief must still win.
    let mock = Arc::new(lcg_core::rerank::MockReranker::new(|_, d| {
        if d.contains("chose Vendor A") {
            10.0
        } else if d.contains("chose Vendor B") {
            9.9
        } else {
            0.0
        }
    }));
    install_reranker(&fx, Some(mock));
    let r = search(
        &fx,
        json!({"query": Q_BELIEF, "group_ids": [G], "num_results": 50,
               "believed_only": false, "rerank": true}),
    )
    .await;
    assert_eq!(r["stats"]["rerank"]["applied"], true);
    let new = rank(&r, "p:ep-new").expect("the correction is returned");
    let old = rank(&r, "p:ep-old").expect("the superseded note is returned when not filtering");
    assert!(
        new < old,
        "superseded must not sit above its correction: {:?}",
        keys(&r)
    );
}

#[tokio::test]
async fn evidence_never_discloses_a_producer_outside_the_callers_filter() {
    // One fact with two primary producers: a live note and one corrected away at T_CORR.
    let fx = fixture_built(|c| {
        entity(c, "iris", "Iris Port", G, SUMMARY);
        entity(c, "jade", "Jade Dock", G, SUMMARY);
        simple_ep(
            c,
            "ep-live",
            "Iris Port ships to Jade Dock.",
            axis(PRICE),
            &["iris", "jade"],
        );
        episode(
            c,
            Ep {
                uuid: "ep-gone",
                content: "Iris Port ships to Jade Dock (old note).",
                emb: axis(PRICE),
                origin: None,
                ingested: T0,
                superseded: Some((T_CORR, "episode")),
                mentions: &["iris", "jade"],
            },
        );
        fact(
            c,
            "e-ship",
            "iris",
            "jade",
            "SHIPS_TO",
            "Iris Port ships to Jade Dock at list price",
            axis(PRICE),
            &["ep-live", "ep-gone"],
            T0,
        );
    });
    let cited = |r: &Value| -> Vec<String> {
        find(r, "f:e-ship").expect("the fact is returned")["evidence"]["supporting_episodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["uuid"].as_str().unwrap().to_string())
            .collect()
    };
    let q = json!({"query": "Iris Port price", "group_ids": [G], "num_results": 50});
    let r = search(&fx, q.clone()).await;
    assert_eq!(
        cited(&r),
        vec!["ep-live"],
        "the corrected producer is not disclosed"
    );
    let mut all = q;
    all["believed_only"] = json!(false);
    let r = search(&fx, all).await;
    assert_eq!(
        cited(&r),
        vec!["ep-live", "ep-gone"],
        "both, when not filtering"
    );
}

#[tokio::test]
async fn rerank_judges_a_fact_by_a_live_producer_not_just_its_first() {
    // The fact's first producer was corrected away, but it is still believed through the second.
    let fx = fixture_built(|c| {
        entity(c, "iris", "Iris Port", G, SUMMARY);
        entity(c, "jade", "Jade Dock", G, SUMMARY);
        episode(
            c,
            Ep {
                uuid: "ep-gone",
                content: "Iris Port ships to Jade Dock (old note).",
                emb: axis(PRICE),
                origin: None,
                ingested: T0,
                superseded: Some((T_CORR, "episode")),
                mentions: &["iris", "jade"],
            },
        );
        simple_ep(
            c,
            "ep-live",
            "Iris Port ships to Jade Dock.",
            axis(PRICE),
            &["iris", "jade"],
        );
        fact(
            c,
            "e-ship",
            "iris",
            "jade",
            "SHIPS_TO",
            "Iris Port ships to Jade Dock at list price",
            axis(PRICE),
            &["ep-gone", "ep-live"],
            T0,
        );
    });
    // The cross-encoder slightly prefers the fact over the live note.
    let mock = Arc::new(lcg_core::rerank::MockReranker::new(|_, d| {
        if d.contains('→') {
            10.0
        } else if d.contains("Iris Port") {
            9.9
        } else {
            0.0
        }
    }));
    install_reranker(&fx, Some(mock));
    let r = search(
        &fx,
        json!({"query": "Iris Port price", "group_ids": [G], "rerank": true}),
    )
    .await;
    assert_eq!(r["stats"]["rerank"]["applied"], true);
    assert_eq!(
        keys(&r)[0],
        "f:e-ship",
        "a believed fact is not demoted for a superseded sibling producer: {:?}",
        keys(&r)
    );
}
