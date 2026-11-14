//! Binary-level tests for the per-group write allow-list (issue #742, ADR-0742): all three entry
//! points (IPC socket, standalone `--mcp-stdio`, attached `--mcp-stdio --connect`) enforce it, and
//! startup validates it strictly. Each test runs the real binary in its own process, so the
//! env-var path is exercised without mutating the test process's environment.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

mod common;
use common::{binary_path, spawn_stub_embedder, ChildGuard, McpClient};

const EXTRACTOR_URL: &str = "http://127.0.0.1:1/v1/chat/completions";

fn wait_for_socket(socket_path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if socket_path.exists() && UnixStream::connect(socket_path).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn base_cmd(dir: &TempDir, embedder_url: &str) -> Command {
    let mut cmd = Command::new(binary_path());
    cmd.env_remove("LCG_WRITABLE_GROUPS")
        .env("LCG_DB_PATH", dir.path().join("test.db"))
        .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
        .env("LCG_WAL_DIR", dir.path().join("wal"))
        .env("LCG_SHUTDOWN_TIMEOUT_MS", "2000")
        .args(["--embedder-http", embedder_url])
        .args(["--extractor-http", EXTRACTOR_URL]);
    cmd
}

fn embedder_url() -> String {
    format!("http://127.0.0.1:{}/v1/embeddings", spawn_stub_embedder())
}

/// Starts the socket daemon; `configure` can add flags/env before spawn.
fn spawn_daemon(
    dir: &TempDir,
    url: &str,
    configure: impl FnOnce(&mut Command),
) -> (ChildGuard, PathBuf) {
    let mut cmd = base_cmd(dir, url);
    configure(&mut cmd);
    let socket = dir.path().join("service.sock");
    let child = ChildGuard::spawn(cmd);
    assert!(
        wait_for_socket(&socket, Duration::from_secs(20)),
        "daemon did not become ready"
    );
    (child, socket)
}

fn socket_request(socket: &Path, method: &str, params: Value) -> Value {
    let mut stream = UnixStream::connect(socket).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    writeln!(
        stream,
        "{}",
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    )
    .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).expect("parse response")
}

fn assert_entity_params(group: &str) -> Value {
    json!({"name": "Widget", "group_id": group, "summary": "a widget"})
}

fn node_count(socket: &Path, group: &str) -> usize {
    let v = socket_request(
        socket,
        "knowledge_get_nodes_by_group",
        json!({"group_ids": [group]}),
    );
    assert!(v["error"].is_null(), "{v}");
    v["result"]["nodes"]
        .as_array()
        .or_else(|| v["result"].as_array())
        .map(|a| a.len())
        .unwrap_or_else(|| panic!("unexpected nodes shape: {v}"))
}

fn assert_ipc_refusal(v: &Value, group: &str) {
    assert_eq!(v["error"]["code"], -32004, "{v}");
    assert_eq!(v["error"]["data"]["reason"], "group_not_writable", "{v}");
    assert_eq!(v["error"]["data"]["group_ids"], json!([group]), "{v}");
    assert_eq!(v["error"]["data"]["setting"], "--writable-groups", "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains(group),
        "{v}"
    );
}

/// The structured error an MCP `tools/call` returns for a refused write.
fn assert_mcp_refusal(resp: &Value, group: &str) {
    let result = &resp["result"];
    assert_eq!(result["isError"], true, "{resp}");
    let payload = &result["structuredContent"];
    assert_eq!(payload["code"], -32004, "{resp}");
    assert_eq!(payload["data"]["reason"], "group_not_writable", "{resp}");
    assert_eq!(payload["data"]["group_ids"], json!([group]), "{resp}");
    assert_eq!(payload["data"]["setting"], "--writable-groups", "{resp}");
}

fn assert_mcp_ok(resp: &Value) {
    assert!(resp["error"].is_null(), "{resp}");
    assert_ne!(resp["result"]["isError"], true, "{resp}");
}

// ── IPC ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn ipc_enforces_the_allow_list() {
    let dir = TempDir::new().unwrap();
    let url = embedder_url();
    let (_daemon, socket) = spawn_daemon(&dir, &url, |c| {
        c.arg("--writable-groups=a");
    });

    let v = socket_request(
        &socket,
        "knowledge_assert_entity",
        assert_entity_params("b"),
    );
    assert_ipc_refusal(&v, "b");
    assert_eq!(
        node_count(&socket, "b"),
        0,
        "refused write must not persist"
    );

    let v = socket_request(
        &socket,
        "knowledge_assert_entity",
        assert_entity_params("a"),
    );
    assert!(v["error"].is_null(), "{v}");
    assert_eq!(node_count(&socket, "a"), 1);

    // Omitted group_id → default group, which is not listed.
    let v = socket_request(
        &socket,
        "knowledge_assert_entity",
        json!({"name": "W2", "summary": "s"}),
    );
    assert_ipc_refusal(&v, "liminis");

    // Raw Cypher is refused outright.
    let v = socket_request(
        &socket,
        "knowledge_query_cypher",
        json!({"query": "MATCH (n) DETACH DELETE n"}),
    );
    assert_eq!(v["error"]["code"], -32004, "{v}");
    assert_eq!(v["error"]["data"]["reason"], "raw_cypher_disabled", "{v}");
    assert_eq!(node_count(&socket, "a"), 1);

    // Reads of a non-listed group still work.
    assert_eq!(node_count(&socket, "b"), 0);
}

#[test]
fn unset_leaves_ipc_unchanged() {
    let dir = TempDir::new().unwrap();
    let url = embedder_url();
    let (_daemon, socket) = spawn_daemon(&dir, &url, |_| {});
    let v = socket_request(
        &socket,
        "knowledge_assert_entity",
        assert_entity_params("b"),
    );
    assert!(v["error"].is_null(), "{v}");
}

// ── attached MCP (and the env-var path) ─────────────────────────────────────────────────────

#[test]
fn attached_mcp_is_enforced_by_the_daemon_configured_via_env() {
    let dir = TempDir::new().unwrap();
    let url = embedder_url();
    let (_daemon, socket) = spawn_daemon(&dir, &url, |c| {
        c.env("LCG_WRITABLE_GROUPS", "a");
    });

    let mut cmd = Command::new(binary_path());
    // The attached process itself is not configured; the daemon enforces.
    cmd.env_remove("LCG_WRITABLE_GROUPS").args([
        "--mcp-stdio",
        "--connect",
        socket.to_str().unwrap(),
    ]);
    let mut client = McpClient::spawn(cmd);
    client.initialize();

    let resp = client.call_tool("knowledge_assert_entity", assert_entity_params("b"));
    assert_mcp_refusal(&resp, "b");
    assert_eq!(node_count(&socket, "b"), 0);

    let resp = client.call_tool("knowledge_assert_entity", assert_entity_params("a"));
    assert_mcp_ok(&resp);
    assert_eq!(node_count(&socket, "a"), 1);

    let resp = client.call_tool(
        "knowledge_query_cypher",
        json!({"query": "MATCH (n) RETURN n"}),
    );
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    assert_eq!(
        resp["result"]["structuredContent"]["data"]["reason"], "raw_cypher_disabled",
        "{resp}"
    );
    client.shutdown();
}

// ── standalone MCP ──────────────────────────────────────────────────────────────────────────

#[test]
fn standalone_mcp_enforces_the_allow_list() {
    let dir = TempDir::new().unwrap();
    let url = embedder_url();
    let mut cmd = base_cmd(&dir, &url);
    cmd.args(["--mcp-stdio", "--writable-groups", "a"]);
    let mut client = McpClient::spawn(cmd);
    client.initialize();

    let resp = client.call_tool("knowledge_assert_entity", assert_entity_params("b"));
    assert_mcp_refusal(&resp, "b");

    let resp = client.call_tool("knowledge_assert_entity", assert_entity_params("a"));
    assert_mcp_ok(&resp);

    // Cross-group edge: authored in `a` is fine, authored in `b` is refused.
    let resp = client.call_tool(
        "knowledge_add_cross_group_edge",
        json!({
            "name": "KNOWS", "group_id": "b", "fact": "x",
            "source": {"uuid": "u1"}, "target": {"uuid": "u2"},
        }),
    );
    assert_mcp_refusal(&resp, "b");
    client.shutdown();
}

// ── startup validation ──────────────────────────────────────────────────────────────────────

fn startup_failure(dir: &TempDir, configure: impl FnOnce(&mut Command)) -> String {
    let mut cmd = base_cmd(dir, "http://127.0.0.1:1/v1/embeddings");
    configure(&mut cmd);
    let out = cmd.output().expect("spawn binary");
    assert!(!out.status.success(), "startup should fail");
    assert!(!dir.path().join("service.sock").exists(), "socket bound");
    assert!(!dir.path().join("test.db").exists(), "database opened");
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn empty_or_malformed_values_fail_startup() {
    let dir = TempDir::new().unwrap();
    let e = startup_failure(&dir, |c| {
        c.arg("--writable-groups=");
    });
    assert!(
        e.contains("FATAL") && e.contains("--writable-groups"),
        "{e}"
    );

    let e = startup_failure(&dir, |c| {
        c.args(["--writable-groups", "a,,b"]);
    });
    assert!(e.contains("FATAL") && e.contains("empty group id"), "{e}");

    let e = startup_failure(&dir, |c| {
        c.env("LCG_WRITABLE_GROUPS", "");
    });
    assert!(
        e.contains("FATAL") && e.contains("LCG_WRITABLE_GROUPS"),
        "{e}"
    );

    let e = startup_failure(&dir, |c| {
        c.env("LCG_WRITABLE_GROUPS", "a, ,b");
    });
    assert!(
        e.contains("FATAL") && e.contains("LCG_WRITABLE_GROUPS"),
        "{e}"
    );
}

#[test]
fn writable_groups_with_connect_is_a_cli_error() {
    let out = Command::new(binary_path())
        .args([
            "--mcp-stdio",
            "--connect",
            "/nonexistent.sock",
            "--writable-groups=a",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let e = String::from_utf8_lossy(&out.stderr);
    assert!(
        e.contains("--writable-groups") && e.contains("daemon"),
        "{e}"
    );
}

fn startup_line(dir: &TempDir, configure: impl FnOnce(&mut Command)) -> String {
    let mut cmd = base_cmd(dir, "http://127.0.0.1:1/v1/embeddings");
    configure(&mut cmd);
    cmd.stderr(Stdio::piped());
    let mut child = ChildGuard::spawn(cmd);
    let stderr = child.stderr.take().expect("stderr");
    for line in BufReader::new(stderr).lines() {
        let line = line.expect("read stderr");
        if line.contains("writable groups:") {
            return line;
        }
    }
    panic!("service exited without logging the writable groups line");
}

#[test]
fn valid_value_is_logged_and_flag_beats_env() {
    let dir = TempDir::new().unwrap();
    let line = startup_line(&dir, |c| {
        c.arg("--writable-groups=b, a");
    });
    assert!(line.contains("[a, b]"), "{line}");

    // The flag wins; a malformed env value is not even parsed.
    let dir = TempDir::new().unwrap();
    let line = startup_line(&dir, |c| {
        c.arg("--writable-groups=x")
            .env("LCG_WRITABLE_GROUPS", "a,,b");
    });
    assert!(line.contains("[x]"), "{line}");

    // Env alone.
    let dir = TempDir::new().unwrap();
    let line = startup_line(&dir, |c| {
        c.env("LCG_WRITABLE_GROUPS", "env-group");
    });
    assert!(line.contains("[env-group]"), "{line}");
}
