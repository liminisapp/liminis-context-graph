// Issue #812: LCG_SEARCH_MIN_SCORE is validated once at startup, before any database open or
// socket bind. These tests run the real binary in its own process (own environment).

#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
mod search_min_score_env_tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use tempfile::TempDir;

    use super::common::ChildGuard;

    const VAR: &str = "LCG_SEARCH_MIN_SCORE";

    fn base_cmd(dir: &TempDir) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_liminis-context-graph"));
        cmd.current_dir(dir.path())
            .env_remove(VAR)
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
        for v in ["abc", "", "NaN", "inf", "-inf", "infinity"] {
            assert_fails_fast(v);
        }
    }

    /// Spawns the service and returns the startup log line describing the default min_score.
    fn startup_line(value: Option<&str>) -> String {
        let dir = TempDir::new().unwrap();
        let mut cmd = base_cmd(&dir);
        if let Some(v) = value {
            cmd.env(VAR, v);
        }
        cmd.stderr(Stdio::piped());
        let mut child = ChildGuard::spawn(cmd);
        let stderr = child.stderr.take().expect("child stderr");
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("read stderr");
            if line.contains("passage search default min_score") {
                return line;
            }
        }
        panic!("service exited without logging the default min_score line");
    }

    #[test]
    fn effective_default_is_logged_at_startup() {
        let unset = startup_line(None);
        assert!(
            unset.ends_with("min_score: 0.5 (LCG_SEARCH_MIN_SCORE unset)"),
            "{unset}"
        );
        let set = startup_line(Some("0.7"));
        assert!(
            set.ends_with("min_score: 0.7 (LCG_SEARCH_MIN_SCORE)"),
            "{set}"
        );
        // Out-of-range values are clamped, not rejected.
        let high = startup_line(Some("1.5"));
        assert!(
            high.ends_with("min_score: 1 (LCG_SEARCH_MIN_SCORE)"),
            "{high}"
        );
        let low = startup_line(Some("-0.2"));
        assert!(
            low.ends_with("min_score: 0 (LCG_SEARCH_MIN_SCORE)"),
            "{low}"
        );
    }
}
