// Issue #737: LCG_BUFFER_POOL_BYTES / LCG_MAX_THREADS are validated once at startup, before any
// database open. These tests run the real binary in its own process (own environment), so they
// cannot race with the in-process test suite that calls `Db::open`.

#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
mod lbug_tuning_env_tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use tempfile::TempDir;

    use super::common::ChildGuard;

    fn base_cmd(dir: &TempDir) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_liminis-context-graph"));
        cmd.current_dir(dir.path())
            .env_remove("LCG_BUFFER_POOL_BYTES")
            .env_remove("LCG_MAX_THREADS")
            .env("LCG_DB_PATH", dir.path().join("db/liminis.db"))
            .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
            .args(["--embedder-http", "http://127.0.0.1:1/v1/embeddings"])
            .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"]);
        cmd
    }

    fn assert_fails_fast(var: &str, value: &str) {
        let dir = TempDir::new().unwrap();
        let out = base_cmd(&dir)
            .env(var, value)
            .output()
            .expect("spawn binary");
        assert!(!out.status.success(), "{var}={value:?} should fail startup");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("FATAL"), "{var}={value:?}: {stderr}");
        assert!(stderr.contains(var), "{var}={value:?}: {stderr}");
        assert!(
            stderr.contains(&format!("\"{value}\"")),
            "{var}={value:?}: {stderr}"
        );
        // No DB opened, no socket bound.
        assert!(
            !dir.path().join("db").exists(),
            "db created for {var}={value:?}"
        );
        assert!(!dir.path().join("service.sock").exists());
    }

    #[test]
    fn invalid_buffer_pool_values_fail_fast() {
        for v in ["banana", "0", "", "-1", "2.5GiB", "2GB", "2 gigs"] {
            assert_fails_fast("LCG_BUFFER_POOL_BYTES", v);
        }
    }

    #[test]
    fn invalid_max_threads_values_fail_fast() {
        for v in ["0", "", "two", "-3"] {
            assert_fails_fast("LCG_MAX_THREADS", v);
        }
    }

    /// Spawns the service with the given env and returns the first startup log line that
    /// mentions the lbug buffer pool.
    fn startup_pool_line(pool: Option<&str>, threads: Option<&str>) -> String {
        let dir = TempDir::new().unwrap();
        let mut cmd = base_cmd(&dir);
        if let Some(p) = pool {
            cmd.env("LCG_BUFFER_POOL_BYTES", p);
        }
        if let Some(t) = threads {
            cmd.env("LCG_MAX_THREADS", t);
        }
        cmd.stderr(Stdio::piped());
        let mut child = ChildGuard::spawn(cmd);
        let stderr = child.stderr.take().expect("child stderr");
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("read stderr");
            if line.contains("lbug buffer pool") {
                return line;
            }
        }
        panic!("service exited without logging the lbug buffer pool line");
    }

    #[test]
    fn valid_values_are_logged_at_startup() {
        let line = startup_pool_line(Some("1GiB"), Some("2"));
        assert!(line.contains("1.0 GiB (1073741824 bytes)"), "{line}");
        assert!(line.contains("max threads: 2"), "{line}");
        // Equivalent plain byte count logs identically.
        assert_eq!(line, startup_pool_line(Some("1073741824"), Some("2")));
    }

    #[test]
    fn unset_logs_default() {
        let line = startup_pool_line(None, None);
        assert!(line.contains("buffer pool: default"), "{line}");
        assert!(line.contains("max threads: default"), "{line}");
    }
}
