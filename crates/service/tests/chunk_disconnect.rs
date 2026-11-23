//! End-to-end: `knowledge_process_chunk` progress and cancel-on-disconnect (issue #861,
//! ADR-0861), against the real binary and a deliberately slow stub extractor.
//!
//! A client that times out and drops its connection must leave no episode behind (the service
//! cancels the abandoned extraction before commit), the service must stay healthy, and a retry on
//! a new connection must extract and commit exactly once. With a `_progress_token` the phases
//! arrive in order before the result.

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

#[test]
fn disconnect_during_extraction_leaves_no_trace_and_retry_commits_once() {
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
    let dir = TempDir::new().unwrap();
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
    let _child = ChildGuard::spawn(cmd);
    wait_healthy(&socket);

    // Send a non-streaming chunk, let extraction start, then drop the connection (a client
    // timeout). Polling the stub's request counter avoids sleeping on a guess.
    {
        let mut c = Client::open(&socket).unwrap();
        c.send(
            2,
            "knowledge_process_chunk",
            chunk_params("SLOW_MARKER text"),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while cfg.lock().unwrap().requests == 0 {
            assert!(Instant::now() < deadline, "extraction never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    } // dropped: the connection closes while the extraction is in flight

    // Outlast the abandoned stub request(s). Without cancel-on-disconnect the first attempt would
    // finish extracting and commit its episode in this window.
    std::thread::sleep(Duration::from_millis(EXTRACT_DELAY_MS * 3));
    assert_eq!(
        episode_count(&socket),
        0,
        "abandoned ingest must not commit"
    );
    wait_healthy(&socket);

    // Timeout-and-retry on a new connection: extracts and commits exactly once.
    let mut retry = Client::open(&socket).unwrap();
    let v = retry.call(
        3,
        "knowledge_process_chunk",
        chunk_params("SLOW_MARKER text"),
    );
    assert_eq!(v.pointer("/result/success"), Some(&json!(true)), "{v}");
    assert_eq!(episode_count(&socket), 1, "retry commits once");
}

#[test]
fn progress_token_streams_phases_in_order_and_plain_request_is_unchanged() {
    let (extractor_port, cfg) = spawn_stub_extractor();
    {
        let mut c = cfg.lock().unwrap();
        c.extraction.insert(
            "FAST_MARKER".to_string(),
            ExtractionFixture {
                entities: vec![("Bob".into(), "Person".into(), "A person".into())],
                edges: vec![],
            },
        );
    }
    let dir = TempDir::new().unwrap();
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
    let _child = ChildGuard::spawn(cmd);
    wait_healthy(&socket);

    let mut c = Client::open(&socket).unwrap();
    let mut params = chunk_params("FAST_MARKER streamed");
    params["_progress_token"] = json!("tok");
    c.send(5, "knowledge_process_chunk", params);
    let mut phases = Vec::new();
    let result = loop {
        let v = c.read();
        if v["type"] == "progress" {
            phases.push(v["phase"].as_str().unwrap().to_string());
        } else {
            break v;
        }
    };
    assert_eq!(phases, vec!["embed", "extract", "commit"]);
    assert_eq!(
        result.pointer("/result/success"),
        Some(&json!(true)),
        "{result}"
    );

    // Without a token: a single response line, no frames.
    let mut plain = chunk_params("FAST_MARKER plain");
    plain["chunk_id"] = json!("c2");
    let v = c.call(6, "knowledge_process_chunk", plain);
    assert_eq!(v["id"], 6);
    assert_eq!(v.pointer("/result/success"), Some(&json!(true)), "{v}");
}
