// Issue #293: end-to-end request-line limit on a real Unix socket against the real binary.

#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
mod request_line_limit_tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};
    use tempfile::TempDir;

    use super::common::{binary_path, spawn_stub_embedder, ChildGuard};

    fn wait_for_socket(path: &std::path::Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if path.exists() && UnixStream::connect(path).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn read_json(reader: &mut BufReader<UnixStream>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        serde_json::from_str(line.trim()).expect("parse response")
    }

    #[test]
    fn oversized_line_is_refused_and_the_connection_keeps_serving() {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("service.sock");
        let port = spawn_stub_embedder();
        let mut cmd = Command::new(binary_path());
        // A small document cap lets the line limit be small too (limit >= 6 x doc + 64 KiB).
        cmd.env("LCG_DB_PATH", dir.path().join("test.db"))
            .env("LCG_SOCKET_PATH", &socket_path)
            .env("LCG_WAL_DIR", dir.path().join("wal"))
            .env("LCG_SHUTDOWN_TIMEOUT_MS", "2000")
            .env("LCG_DOCUMENT_MAX_BYTES", "1KiB")
            .env("LCG_REQUEST_MAX_BYTES", "128KiB")
            .args([
                "--embedder-http",
                &format!("http://127.0.0.1:{port}/v1/embeddings"),
            ])
            .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"]);
        let _child = ChildGuard::spawn(cmd);
        assert!(
            wait_for_socket(&socket_path),
            "service did not become ready"
        );

        let stream = UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);

        // 300 KiB of one line, well over the 128 KiB limit.
        let big = "x".repeat(300 * 1024);
        let req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "knowledge_find_entities",
            "params": {"query": big}
        });
        writeln!(writer, "{req}").unwrap();
        let refused = read_json(&mut reader);
        assert!(refused["id"].is_null(), "{refused}");
        assert_eq!(refused["error"]["code"], -32005, "{refused}");
        assert_eq!(refused["error"]["data"]["reason"], "request_too_large");
        assert_eq!(refused["error"]["data"]["limit_bytes"], 128 * 1024);
        assert_eq!(refused["error"]["data"]["setting"], "LCG_REQUEST_MAX_BYTES");

        // The same connection serves the next well-formed line.
        let ok = json!({"jsonrpc": "2.0", "id": 2, "method": "knowledge_status", "params": {}});
        writeln!(writer, "{ok}").unwrap();
        let status = read_json(&mut reader);
        assert_eq!(status["id"], 2, "{status}");
        assert!(status.get("result").is_some(), "{status}");

        // A request just under the limit is read in full and dispatched (it is not refused for
        // size; whatever the handler answers, it is not -32005).
        let under = "y".repeat(100 * 1024);
        let req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "knowledge_find_entities",
            "params": {"query": under, "num_results": 1}
        });
        writeln!(writer, "{req}").unwrap();
        let answered = read_json(&mut reader);
        assert_eq!(answered["id"], 3, "{answered}");
        assert_ne!(answered["error"]["code"], -32005, "{answered}");
    }
}
