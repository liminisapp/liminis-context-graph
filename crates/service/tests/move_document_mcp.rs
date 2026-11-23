//! Issue #903 / ADR-0903 over `--mcp-stdio`: `knowledge_move_document` is advertised as a
//! `write`-scope tool, `group_id` is required, a request is validated and refused before anything
//! is written, and the `move_document` feature string is reported. No extraction LLM is reachable
//! here, which is the point: a move must not need one.
#![cfg(unix)]

use std::process::Command;

use serde_json::{json, Value};
use tempfile::TempDir;

mod common;
use common::{binary_path, spawn_stub_embedder, McpClient};

fn spawn(dir: &TempDir, extra: &[&str]) -> McpClient {
    let url = format!("http://127.0.0.1:{}/v1/embeddings", spawn_stub_embedder());
    let mut cmd = Command::new(binary_path());
    cmd.env("LCG_DB_PATH", dir.path().join("test.db"))
        .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
        .env("LCG_WAL_DIR", dir.path().join("wal"))
        .env("LCG_SHUTDOWN_TIMEOUT_MS", "2000")
        .args(["--mcp-stdio", "--embedder-http", &url])
        .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"])
        .args(extra);
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

fn one_move() -> Value {
    json!([{
        "from": {"doc_id": "a", "source_path": "a.md"},
        "to": {"doc_id": "b", "source_path": "b.md"},
    }])
}

#[test]
fn move_document_is_a_write_tool_that_requires_a_group_and_refuses_cleanly() {
    let dir = TempDir::new().unwrap();
    let mut client = spawn(&dir, &[]);

    let tools = client.list_tools();
    let tool = tools
        .iter()
        .find(|t| t["name"] == "knowledge_move_document")
        .expect("knowledge_move_document not advertised");
    assert_eq!(
        tool["inputSchema"]["required"],
        json!(["group_id", "moves"]),
        "{tool}"
    );

    // group_id is required.
    let resp = client.call_tool("knowledge_move_document", json!({"moves": one_move()}));
    assert_eq!(resp["result"]["isError"], true, "{resp}");

    // An unknown source is refused with a stable code, and nothing is written.
    let resp = client.call_tool(
        "knowledge_move_document",
        json!({"group_id": "g", "moves": one_move()}),
    );
    let msg = error_text(&resp);
    assert!(
        msg.contains("move_refused") && msg.contains("unknown_source"),
        "{msg}"
    );

    // An empty batch is a request error.
    let resp = client.call_tool(
        "knowledge_move_document",
        json!({"group_id": "g", "moves": []}),
    );
    assert!(error_text(&resp).contains("empty_batch"), "{resp}");

    // The capability is detectable.
    let caps = client.call_tool("knowledge_capabilities", json!({}));
    let features = caps["result"]["structuredContent"]["features"].to_string();
    assert!(features.contains("\"move_document\""), "{features}");
    client.shutdown();
}

#[test]
fn move_document_is_absent_from_the_read_scope() {
    let dir = TempDir::new().unwrap();
    let mut client = spawn(&dir, &["--scope=read"]);
    let tools = client.list_tools();
    assert!(tools.iter().all(|t| t["name"] != "knowledge_move_document"));
    client.shutdown();
}
