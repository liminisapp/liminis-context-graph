//! End-to-end: `knowledge_process_chunk` `deadline_ms` and joining a duplicate in-flight request
//! (issue #914, ADR-0914), against the real binary and a deliberately slow stub extractor. The
//! stub's request counter is the number of extraction calls the service made.

#![cfg(unix)]

#[path = "common/mod.rs"]
mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use common::{
    binary_path, spawn_stub_embedder, spawn_stub_extractor, ChildGuard, ExtractionFixture,
};

/// Stub latency per HTTP request. The stub serves one request at a time, so this is also how long
/// an abandoned extraction request keeps it busy.
const EXTRACT_DELAY_MS: u64 = 1500;

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    fn open(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(60)))?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
        })
    }

    fn send(&mut self, id: u64, method: &str, params: Value) {
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.writer, "{req}").unwrap();
        self.writer.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).expect("read line");
        assert!(n > 0, "connection closed by server");
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(id, method, params);
        self.read()
    }
}

fn wait_healthy(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Ok(mut c) = Client::open(path) {
            let v = c.call(1, "health_check", json!({}));
            if v.pointer("/result/healthy") == Some(&Value::Bool(true)) {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("service not healthy in time");
}

fn episode_count(path: &Path) -> u64 {
    let mut c = Client::open(path).unwrap();
    let v = c.call(1, "knowledge_status", json!({}));
    v.pointer("/result/episode_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("no episode_count: {v}"))
}

fn chunk_params(text: &str) -> Value {
    json!({
        "chunk_text": text, "chunk_id": "c1", "source_file": "doc.md",
        "reference_time": "2026-08-04T00:00:00Z",
    })
}

fn start_service(
    dir: &TempDir,
) -> (
    ChildGuard,
    std::path::PathBuf,
    std::sync::Arc<std::sync::Mutex<common::StubExtractorConfig>>,
) {
    let (extractor_port, cfg) = spawn_stub_extractor();
    {
        let mut c = cfg.lock().unwrap();
        c.delay_ms = EXTRACT_DELAY_MS;
        c.extraction.insert(
            "SLOW_MARKER".to_string(),
            ExtractionFixture {
                entities: vec![("Alice".into(), "Person".into(), "A person".into())],
                edges: vec![],
            },
        );
    }
    let socket = dir.path().join("service.sock");
    let mut cmd = Command::new(binary_path());
    cmd.env("LCG_DB_PATH", dir.path().join("test.db"))
        .env("LCG_SOCKET_PATH", &socket)
        .env("LCG_WAL_DIR", dir.path().join("wal"))
        .env("LCG_SHUTDOWN_TIMEOUT_MS", "2000")
        .args([
            "--embedder-http",
            &format!("http://127.0.0.1:{}/v1/embeddings", spawn_stub_embedder()),
        ])
        .args([
            "--extractor-http",
            &format!("http://127.0.0.1:{extractor_port}/v1/chat/completions"),
        ]);
    let child = ChildGuard::spawn(cmd);
    wait_healthy(&socket);
    (child, socket, cfg)
}

fn wait_requests(cfg: &std::sync::Arc<std::sync::Mutex<common::StubExtractorConfig>>, n: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while cfg.lock().unwrap().requests < n {
        assert!(Instant::now() < deadline, "extraction {n} never started");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn deadline_returns_structured_error_before_commit_and_service_stays_healthy() {
    let dir = TempDir::new().unwrap();
    let (_child, socket, _cfg) = start_service(&dir);

    let mut c = Client::open(&socket).unwrap();
    let mut p = chunk_params("SLOW_MARKER text");
    p["deadline_ms"] = json!(300);
    let v = c.call(2, "knowledge_process_chunk", p);
    assert_eq!(v["id"], 2, "{v}");
    assert_eq!(v["error"]["code"], -32005, "{v}");
    assert_eq!(v["error"]["data"]["reason"], "deadline_exceeded", "{v}");
    assert_eq!(v["error"]["data"]["committed"], false, "{v}");
    assert!(
        ["embed", "extract"].contains(&v["error"]["data"]["phase"].as_str().unwrap()),
        "{v}"
    );
    // The same connection still works, and nothing was written.
    let h = c.call(3, "health_check", json!({}));
    assert_eq!(h["result"]["healthy"], true, "{h}");
    assert_eq!(episode_count(&socket), 0);
}

#[test]
fn identical_resubmit_on_a_second_connection_joins_the_first() {
    let dir = TempDir::new().unwrap();
    let (_child, socket, cfg) = start_service(&dir);

    let mut a = Client::open(&socket).unwrap();
    a.send(
        2,
        "knowledge_process_chunk",
        chunk_params("SLOW_MARKER text"),
    );
    wait_requests(&cfg, 1);
    let mut b = Client::open(&socket).unwrap();
    let rb = b.call(
        7,
        "knowledge_process_chunk",
        chunk_params("SLOW_MARKER text"),
    );
    let ra = a.read();
    assert_eq!(ra["result"]["success"], true, "{ra}");
    assert_eq!(rb["result"]["success"], true, "{rb}");
    assert_eq!(rb["id"], 7, "{rb}");
    assert_eq!(rb["result"]["joined"], true, "{rb}");
    assert!(ra["result"].get("joined").is_none(), "{ra}");
    assert_eq!(ra["result"]["episode_uuid"], rb["result"]["episode_uuid"]);
    assert_eq!(episode_count(&socket), 1);
    // One chunk costs some number of stub requests; two joined requests cost exactly one chunk's
    // worth. Measure that cost with a distinct chunk ingested alone.
    let joined_cost = cfg.lock().unwrap().requests;
    let mut solo = chunk_params("SLOW_MARKER solo text");
    solo["chunk_id"] = json!("c-solo");
    let rs = b.call(8, "knowledge_process_chunk", solo);
    assert_eq!(rs["result"]["success"], true, "{rs}");
    let solo_cost = cfg.lock().unwrap().requests - joined_cost;
    assert_eq!(
        joined_cost, solo_cost,
        "the joined pair extracted exactly once"
    );
}

#[test]
fn first_connection_dropping_does_not_cancel_the_joined_request() {
    let dir = TempDir::new().unwrap();
    let (_child, socket, cfg) = start_service(&dir);

    let mut b = {
        let mut a = Client::open(&socket).unwrap();
        a.send(
            2,
            "knowledge_process_chunk",
            chunk_params("SLOW_MARKER text"),
        );
        wait_requests(&cfg, 1);
        let mut b = Client::open(&socket).unwrap();
        b.send(
            7,
            "knowledge_process_chunk",
            chunk_params("SLOW_MARKER text"),
        );
        std::thread::sleep(Duration::from_millis(300)); // let B register as a follower
        b
    }; // A is dropped here: the leader's client disconnects before commit
    let rb = b.read();
    assert_eq!(rb["result"]["success"], true, "{rb}");
    assert_eq!(episode_count(&socket), 1, "exactly one live episode");
}
