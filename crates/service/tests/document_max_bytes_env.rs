// Issue #862: LCG_DOCUMENT_MAX_BYTES is validated once at startup, before any database open or
// socket bind. These tests run the real binary in its own process (own environment).

#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
mod document_max_bytes_env_tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use tempfile::TempDir;

    use super::common::ChildGuard;

    const VAR: &str = "LCG_DOCUMENT_MAX_BYTES";

    fn base_cmd(dir: &TempDir) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_liminis-context-graph"));
        cmd.current_dir(dir.path())
            .env_remove(VAR)
            .env_remove("LCG_REQUEST_MAX_BYTES")
            .env_remove("LCG_CHUNK_TEXT_MAX_BYTES")
            .env_remove("LCG_SEARCH_MIN_SCORE")
            .env_remove("LCG_BUFFER_POOL_BYTES")
            .env_remove("LCG_MAX_THREADS")
            .env("LCG_DB_PATH", dir.path().join("db/liminis.db"))
            .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
            .args(["--embedder-http", "http://127.0.0.1:1/v1/embeddings"])
            .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"]);
        cmd
    }

    fn assert_fails_fast(value: &str) {
        let dir = TempDir::new().unwrap();
        let out = base_cmd(&dir)
            .env(VAR, value)
            .output()
            .expect("spawn binary");
        assert!(!out.status.success(), "{VAR}={value:?} should fail startup");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("FATAL"), "{VAR}={value:?}: {stderr}");
        assert!(stderr.contains(VAR), "{VAR}={value:?}: {stderr}");
        assert!(
            stderr.contains(&format!("\"{value}\"")),
            "{VAR}={value:?}: {stderr}"
        );
        // No DB opened, no socket bound.
        assert!(
            !dir.path().join("db").exists(),
            "db created for {VAR}={value:?}"
        );
        assert!(!dir.path().join("service.sock").exists());
    }

    #[test]
    fn invalid_values_fail_fast() {
        for v in [
            "abc", "", "0", "-1", "4MB", "4m", "1.5MiB", "4 MiB", "1GiB", "512MiB",
        ] {
            assert_fails_fast(v);
        }
    }

    /// Spawns the service and returns the startup log line describing the document cap.
    fn startup_line(value: Option<&str>) -> String {
        startup_line_with(value, &[])
    }

    fn startup_line_with(value: Option<&str>, extra_env: &[(&str, &str)]) -> String {
        let dir = TempDir::new().unwrap();
        let mut cmd = base_cmd(&dir);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        if let Some(v) = value {
            cmd.env(VAR, v);
        }
        cmd.stderr(Stdio::piped());
        let mut child = ChildGuard::spawn(cmd);
        let stderr = child.stderr.take().expect("child stderr");
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("read stderr");
            if line.contains("document size cap") {
                return line;
            }
        }
        panic!("service exited without logging the document size cap line");
    }

    #[test]
    fn effective_cap_is_logged_at_startup() {
        let unset = startup_line(None);
        assert!(
            unset.ends_with("document size cap: 2097152 bytes (LCG_DOCUMENT_MAX_BYTES unset)"),
            "{unset}"
        );
        // A 4 MiB cap needs a larger line limit than the 16 MiB default (#293); the default
        // limit with a 4 MiB cap is refused (see request_limits_env.rs).
        let set = startup_line_with(Some("4MiB"), &[("LCG_REQUEST_MAX_BYTES", "32MiB")]);
        assert!(
            set.ends_with("document size cap: 4194304 bytes (LCG_DOCUMENT_MAX_BYTES)"),
            "{set}"
        );
        let plain = startup_line(Some("1048576"));
        assert!(
            plain.ends_with("document size cap: 1048576 bytes (LCG_DOCUMENT_MAX_BYTES)"),
            "{plain}"
        );
    }
}
