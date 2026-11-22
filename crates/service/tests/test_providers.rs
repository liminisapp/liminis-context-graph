//! Binary-level coverage for the built-in test providers (#851): `--embedder test` and
//! `--extractor test` start the real engine with no sidecar, no credentials and no network, in
//! socket and `--mcp-stdio` modes, and announce themselves in `knowledge_status`/`health_check`.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

mod common;
use common::{binary_path, spawn_stub_embedder, ChildGuard, McpClient};

/// A dead port: any attempt to dial a provider here fails immediately.
const DEAD_URL: &str = "http://127.0.0.1:1/v1/embeddings";

fn base_cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::new(binary_path());
    cmd.env("LCG_DB_PATH", dir.path().join("test.db"))
        .env("LCG_WAL_DIR", dir.path().join("wal"))
        .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
        .env("LCG_SHUTDOWN_TIMEOUT_MS", "2000")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("LCG_RECORD_LLM")
        .env_remove("LCG_REPLAY_LLM")
        .env_remove("LCG_EMBEDDING_DIM")
        // If either flag ever leaked through to a real provider it would dial here and fail.
        .env("LCG_EMBEDDING_URL", DEAD_URL)
        .env(
            "LCG_EXTRACTION_URL",
            "http://127.0.0.1:1/v1/chat/completions",
        );
    cmd
}

fn mcp_test_client(dir: &TempDir, extra: &[&str]) -> McpClient {
    let mut cmd = base_cmd(dir);
    cmd.args(["--mcp-stdio", "--embedder", "test", "--extractor", "test"])
        .args(extra);
    let mut c = McpClient::spawn(cmd);
    c.initialize();
    c
}

fn structured(resp: &Value) -> &Value {
    assert!(
        resp["result"]["isError"].as_bool() != Some(true),
        "tool call failed: {resp}"
    );
    &resp["result"]["structuredContent"]
}

fn names(resp: &Value) -> Vec<String> {
    let mut n: Vec<String> = structured(resp)["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    n.sort();
    n
}

#[test]
fn mcp_mode_ingests_fact_lines_offline_and_announces_test_mode() {
    let dir = TempDir::new().unwrap();
    let mut c = mcp_test_client(&dir, &[]);

    let status = c.call_tool("knowledge_status", json!({}));
    assert_eq!(
        structured(&status)["providers"],
        json!({"embedder": "test", "extractor": "test"}),
        "{status}"
    );
    assert_eq!(structured(&status)["embedding_model"], "lcg-test-bow");
    assert_eq!(structured(&status)["embedding_dim"], 768);

    for (name, body) in [
        ("a", "prose line\nAlice -> Acme: Alice works at Acme\nWiki -[HOSTED_ON]-> ServerA: Wiki runs on ServerA"),
        ("b", "Alice -> Acme: Alice works at Acme"),
        ("c", "no fact lines here at all"),
    ] {
        let r = c.call_tool(
            "knowledge_add_episode",
            json!({"name": name, "episode_body": body, "group_id": "g"}),
        );
        structured(&r);
    }

    let nodes = c.call_tool("knowledge_get_nodes_by_group", json!({"group_ids": ["g"]}));
    assert_eq!(names(&nodes), ["Acme", "Alice", "ServerA", "Wiki"]);
    let edges = c.call_tool("knowledge_get_edges_by_group", json!({"group_ids": ["g"]}));
    let edges = structured(&edges)["edges"].as_array().unwrap().clone();
    // The repeated fact line (documents `a` and `b`) resolves to the same entity pair; the
    // engine keeps one edge record per document, so there are three records over two pairs.
    assert_eq!(edges.len(), 3, "{edges:?}");
    let pairs: std::collections::BTreeSet<(String, String)> = edges
        .iter()
        .map(|e| {
            (
                e["source_node_uuid"].as_str().unwrap().to_string(),
                e["target_node_uuid"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(pairs.len(), 2, "{edges:?}");
    let rel = |fact: &str| {
        edges
            .iter()
            .find(|e| e["fact"] == fact)
            .unwrap_or_else(|| panic!("no edge {fact}: {edges:?}"))["relation_type"]
            .clone()
    };
    assert_eq!(rel("Alice works at Acme"), "RELATED_TO");
    assert_eq!(rel("Wiki runs on ServerA"), "HOSTED_ON");

    c.shutdown();
}

#[test]
fn mcp_mode_honours_configured_dimension() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir);
    cmd.env("LCG_EMBEDDING_DIM", "128").args([
        "--mcp-stdio",
        "--embedder",
        "test",
        "--extractor",
        "test",
    ]);
    let mut c = McpClient::spawn(cmd);
    c.initialize();
    let status = c.call_tool("knowledge_status", json!({}));
    assert_eq!(structured(&status)["embedding_dim"], 128, "{status}");
    c.shutdown();
}

struct Sock {
    w: UnixStream,
    r: BufReader<UnixStream>,
}

impl Sock {
    fn call(&mut self, method: &str, params: Value) -> Value {
        let req = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        writeln!(self.w, "{req}").unwrap();
        let mut line = String::new();
        self.r.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
}

fn connect_healthy(path: &Path) -> Sock {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(path) {
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let mut sock = Sock {
                r: BufReader::new(s.try_clone().unwrap()),
                w: s,
            };
            let h = sock.call("health_check", json!({}));
            if h["result"]["healthy"] == true {
                return sock;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("service not healthy in time");
}

#[test]
fn socket_mode_starts_healthy_with_test_providers_and_reports_them() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir);
    cmd.args(["--embedder", "test", "--extractor", "test"]);
    let _child = ChildGuard::spawn(cmd);
    let mut s = connect_healthy(&dir.path().join("service.sock"));

    let h = s.call("health_check", json!({}));
    assert_eq!(
        h["result"]["providers"],
        json!({"embedder": "test", "extractor": "test"}),
        "{h}"
    );
    let st = s.call("knowledge_status", json!({}));
    assert_eq!(
        st["result"]["providers"],
        json!({"embedder": "test", "extractor": "test"}),
        "{st}"
    );
}

#[test]
fn without_the_flags_status_reports_real_providers_even_with_test_looking_env() {
    let dir = TempDir::new().unwrap();
    let port = spawn_stub_embedder();
    let url = format!("http://127.0.0.1:{port}/v1/embeddings");
    let mut cmd = base_cmd(&dir);
    cmd.env("LCG_EMBEDDER", "test")
        .env("LCG_EXTRACTOR", "test")
        .env("LCG_TEST_PROVIDERS", "1")
        .env("LCG_EMBEDDING_MODEL", "test")
        .args(["--mcp-stdio", "--embedder-http", &url]);
    let mut c = McpClient::spawn(cmd);
    c.initialize();
    let status = c.call_tool("knowledge_status", json!({}));
    let p = &structured(&status)["providers"];
    assert_eq!(p["embedder"], "real", "{status}");
    assert_ne!(p["extractor"], "test", "{status}");
    c.shutdown();
}

#[test]
fn embedder_test_alone_leaves_extractor_resolution_unchanged() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir);
    cmd.env_remove("LCG_EXTRACTION_URL")
        .args(["--mcp-stdio", "--embedder", "test"]);
    let mut c = McpClient::spawn(cmd);
    c.initialize();
    let status = c.call_tool("knowledge_status", json!({}));
    assert_eq!(
        structured(&status)["providers"],
        json!({"embedder": "test", "extractor": "none"}),
        "{status}"
    );
    c.shutdown();
}

fn run(dir: &TempDir, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = base_cmd(dir);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn binary")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn invalid_flag_combinations_fail_at_startup_with_clear_errors() {
    let dir = TempDir::new().unwrap();
    for (args, needle) in [
        (
            &["--embedder", "real"][..],
            "unknown value 'real' for --embedder",
        ),
        (
            &["--extractor", "mock"][..],
            "unknown value 'mock' for --extractor",
        ),
        (
            &["--embedder", "test", "--embedder-http", "http://x"][..],
            "cannot be combined",
        ),
        (
            &["--extractor", "test", "--extractor-uds", "/s"][..],
            "cannot be combined",
        ),
        (
            &["--mcp-stdio", "--connect", "/s", "--embedder", "test"][..],
            "--connect",
        ),
    ] {
        let o = run(&dir, args, &[]);
        assert!(!o.status.success(), "{args:?} should fail");
        assert!(stderr(&o).contains(needle), "{args:?}: {}", stderr(&o));
    }
}

#[test]
fn extractor_test_with_a_cassette_variable_is_a_startup_error() {
    for var in ["LCG_RECORD_LLM", "LCG_REPLAY_LLM"] {
        let dir = TempDir::new().unwrap();
        let cassette = dir.path().join("c.jsonl");
        let o = run(
            &dir,
            &["--mcp-stdio", "--embedder", "test", "--extractor", "test"],
            &[(var, cassette.to_str().unwrap())],
        );
        assert!(!o.status.success(), "{var} should be rejected");
        assert!(stderr(&o).contains("LCG_RECORD_LLM"), "{}", stderr(&o));
    }
}

#[test]
fn startup_logs_a_prominent_test_mode_line() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir);
    cmd.args(["--mcp-stdio", "--embedder", "test", "--extractor", "test"]);
    let mut c = McpClient::spawn_capturing_stderr(cmd);
    c.initialize();
    // SIGTERM (not `shutdown()`, which consumes the client) so stderr reaches EOF.
    let rc = unsafe { libc::kill(c.pid() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0);
    c.wait_for_exit(Duration::from_secs(10));
    let lines = c.collect_stderr();
    assert!(
        lines.iter().any(|l| l.contains("TEST PROVIDERS ACTIVE")),
        "{lines:?}"
    );
}
