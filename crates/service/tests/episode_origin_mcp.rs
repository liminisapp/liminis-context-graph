//! Issue #752 / ADR-0752 over `--mcp-stdio`: the `origin` / `origins` parameters and
//! `knowledge_stamp_origin` are reachable on the MCP tool surface, validated before anything runs.
//! No extraction LLM is reachable here, so every call below must succeed or fail without one.
#![cfg(unix)]

use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

mod common;
use common::{binary_path, spawn_stub_embedder, McpClient};

fn spawn(dir: &TempDir) -> McpClient {
    let url = format!("http://127.0.0.1:{}/v1/embeddings", spawn_stub_embedder());
    let mut cmd = Command::new(binary_path());
    cmd.env("LCG_DB_PATH", dir.path().join("test.db"))
        .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
        .env("LCG_WAL_DIR", dir.path().join("wal"))
        .env("LCG_SHUTDOWN_TIMEOUT_MS", "2000")
        .args(["--mcp-stdio", "--embedder-http", &url])
        .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"]);
    let mut client = McpClient::spawn(cmd);
    client.initialize();
    client
}

fn error_text(resp: &Value) -> String {
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    resp["result"]["structuredContent"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| resp.to_string())
}

#[test]
fn origin_surface_is_advertised_and_validated() {
    let dir = TempDir::new().unwrap();
    let mut client = spawn(&dir);

    let tools = client.list_tools();
    let schema_of = |name: &str| -> Value {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name} not advertised"))["inputSchema"]["properties"]
            .clone()
    };
    for name in [
        "knowledge_process_chunk",
        "knowledge_add_episode",
        "knowledge_mark_chunk_removed",
    ] {
        assert!(
            schema_of(name).get("origin").is_some(),
            "{name} lacks `origin`"
        );
    }
    for name in [
        "knowledge_find_relationships",
        "knowledge_find_entities",
        "knowledge_search_passages",
    ] {
        assert!(
            schema_of(name).get("origins").is_some(),
            "{name} lacks `origins`"
        );
    }
    assert!(schema_of("knowledge_stamp_origin").get("rules").is_some());

    // An invalid origin is refused before extraction (which would fail: no LLM is reachable).
    let resp = client.call_tool(
        "knowledge_process_chunk",
        json!({"chunk_text": "t", "chunk_id": "c", "source_file": "f.md", "origin": "wiki"}),
    );
    assert!(
        error_text(&resp).contains("primary, external, derived, answer"),
        "{resp}"
    );

    // An empty `origins` filter is rejected on a read.
    let resp = client.call_tool(
        "knowledge_find_relationships",
        json!({"query": "q", "origins": []}),
    );
    assert_eq!(resp["result"]["isError"], true, "{resp}");

    // The stamp tool runs (dry run, empty graph) and reports its counts.
    let resp = client.call_tool(
        "knowledge_stamp_origin",
        json!({"group_id": "g", "dry_run": true,
               "rules": [{"origin": "derived", "source_path_prefix": "wiki/"}]}),
    );
    assert_ne!(resp["result"]["isError"], true, "{resp}");
    let payload = &resp["result"]["structuredContent"];
    assert_eq!(payload["dry_run"], true, "{resp}");
    assert_eq!(payload["changed"], 0, "{resp}");
    client.shutdown();
}
