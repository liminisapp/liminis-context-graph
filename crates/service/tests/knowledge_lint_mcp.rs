//! Issue #755 / ADR-0755 over `--mcp-stdio`: `knowledge_lint` is a `read`-scope tool, reachable
//! without any extraction LLM or embedding call, validated before any work, and absent from
//! scopes that exclude `read`.
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

#[test]
fn lint_is_a_read_tool_that_runs_without_any_model() {
    let dir = TempDir::new().unwrap();
    let mut client = spawn(&dir, &["--scope=read"]);

    let tools = client.list_tools();
    let lint = tools
        .iter()
        .find(|t| t["name"] == "knowledge_lint")
        .expect("knowledge_lint must be advertised under --scope=read");
    let props = &lint["inputSchema"]["properties"];
    for key in [
        "group_ids",
        "categories",
        "limit",
        "cursor",
        "single_valued_relations",
        "near_duplicate_threshold",
        "min_mentions",
    ] {
        assert!(props.get(key).is_some(), "schema lacks `{key}`");
    }

    // Empty graph: a well-formed empty report (no LLM is reachable here).
    let resp = client.call_tool("knowledge_lint", json!({}));
    assert_ne!(resp["result"]["isError"], true, "{resp}");
    let payload = &resp["result"]["structuredContent"];
    assert_eq!(payload["count"], 0, "{resp}");
    assert_eq!(payload["summary"]["total"], 0, "{resp}");
    assert!(payload["next_cursor"].is_null(), "{resp}");
    assert_eq!(
        payload["summary"]["categories"]["contradiction"]["status"], "unavailable",
        "{resp}"
    );

    // Invalid input is rejected before any work.
    let resp = client.call_tool("knowledge_lint", json!({"categories": ["bogus"]}));
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    let resp = client.call_tool("knowledge_lint", json!({"limit": 0}));
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    client.shutdown();
}

#[test]
fn lint_is_not_advertised_without_read_scope() {
    let dir = TempDir::new().unwrap();
    let mut client = spawn(&dir, &["--scope=write"]);
    let tools = client.list_tools();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&"knowledge_lint"), "{names:?}");

    let resp: Value = client.call_tool("knowledge_lint", json!({}));
    assert!(
        resp.get("error").is_some() || resp["result"]["isError"] == true,
        "an out-of-scope tool call must be rejected: {resp}"
    );
    client.shutdown();
}
