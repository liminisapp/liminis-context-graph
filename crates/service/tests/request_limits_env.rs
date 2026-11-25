// Issue #293: LCG_REQUEST_MAX_BYTES and LCG_CHUNK_TEXT_MAX_BYTES are validated once at startup
// (before any database open or socket bind), together with the cross-check that the line limit
// can carry a maximal document at LCG_DOCUMENT_MAX_BYTES. These tests run the real binary in its
// own process (own environment).

#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
mod request_limits_env_tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use tempfile::TempDir;

    use super::common::ChildGuard;

    const LINE: &str = "LCG_REQUEST_MAX_BYTES";
    const CHUNK: &str = "LCG_CHUNK_TEXT_MAX_BYTES";
    const DOC: &str = "LCG_DOCUMENT_MAX_BYTES";

    fn base_cmd(dir: &TempDir, env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_liminis-context-graph"));
        cmd.current_dir(dir.path())
            .env_remove(LINE)
            .env_remove(CHUNK)
            .env_remove(DOC)
            .env_remove("LCG_SEARCH_MIN_SCORE")
            .env_remove("LCG_BUFFER_POOL_BYTES")
            .env_remove("LCG_MAX_THREADS")
            .env("LCG_DB_PATH", dir.path().join("db/liminis.db"))
            .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
            .args(["--embedder-http", "http://127.0.0.1:1/v1/embeddings"])
            .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"]);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd
    }

    /// Startup must fail, naming every string in `must_contain`, without touching the DB or socket.
    fn assert_fatal(env: &[(&str, &str)], must_contain: &[&str]) {
        let dir = TempDir::new().unwrap();
        let out = base_cmd(&dir, env).output().expect("spawn binary");
        assert!(!out.status.success(), "{env:?} should fail startup");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("FATAL"), "{env:?}: {stderr}");
        for needle in must_contain {
            assert!(
                stderr.contains(needle),
                "{env:?} missing {needle:?}: {stderr}"
            );
        }
        assert!(!dir.path().join("db").exists(), "db created for {env:?}");
        assert!(!dir.path().join("service.sock").exists());
    }

    #[test]
    fn invalid_request_limit_values_fail_fast() {
        for v in [
            "abc", "", "0", "-1", "4MB", "1.5MiB", "4 MiB", "3GiB", "1TiB",
        ] {
            assert_fatal(&[(LINE, v)], &[LINE, &format!("\"{v}\"")]);
        }
    }

    #[test]
    fn invalid_chunk_cap_values_fail_fast() {
        for v in ["abc", "", "0", "-1", "4MB", "1.5MiB", "1GiB", "512MiB"] {
            assert_fatal(&[(CHUNK, v)], &[CHUNK, &format!("\"{v}\"")]);
        }
    }

    #[test]
    fn raising_the_document_cap_without_the_line_limit_is_refused() {
        // 6 x 4 MiB + 64 KiB does not fit the 16 MiB default line limit.
        assert_fatal(&[(DOC, "4MiB")], &[LINE, DOC, "25231360"]);
    }

    #[test]
    fn too_small_explicit_line_limit_is_refused_naming_both_settings() {
        assert_fatal(&[(LINE, "1MiB")], &[LINE, DOC, "1048576", "12648448"]);
    }

    #[test]
    fn document_cap_at_its_upper_bound_needs_a_large_line_limit() {
        assert_fatal(&[(DOC, "256MiB"), (LINE, "1GiB")], &[LINE, DOC]);
    }

    /// Spawns the service and collects the two startup log lines for the new settings.
    fn startup_lines(env: &[(&str, &str)]) -> (String, String) {
        let dir = TempDir::new().unwrap();
        let mut cmd = base_cmd(&dir, env);
        cmd.stderr(Stdio::piped());
        let mut child = ChildGuard::spawn(cmd);
        let stderr = child.stderr.take().expect("child stderr");
        let (mut line_log, mut chunk_log) = (None, None);
        for l in BufReader::new(stderr).lines() {
            let l = l.expect("read stderr");
            if l.contains("request line limit") {
                line_log = Some(l);
            } else if l.contains("chunk_text size cap") {
                chunk_log = Some(l);
            }
            if line_log.is_some() && chunk_log.is_some() {
                return (line_log.unwrap(), chunk_log.unwrap());
            }
        }
        panic!("service exited without logging both settings: {line_log:?} {chunk_log:?}");
    }

    #[test]
    fn defaults_are_logged_at_startup() {
        let (line, chunk) = startup_lines(&[]);
        assert!(
            line.ends_with("request line limit: 16777216 bytes (LCG_REQUEST_MAX_BYTES unset)"),
            "{line}"
        );
        assert!(
            chunk.ends_with("chunk_text size cap: 524288 bytes (LCG_CHUNK_TEXT_MAX_BYTES unset)"),
            "{chunk}"
        );
    }

    #[test]
    fn a_valid_consistent_set_starts_and_is_logged() {
        let (line, chunk) = startup_lines(&[(DOC, "4MiB"), (LINE, "32MiB"), (CHUNK, "256KiB")]);
        assert!(
            line.ends_with("request line limit: 33554432 bytes (LCG_REQUEST_MAX_BYTES)"),
            "{line}"
        );
        assert!(
            chunk.ends_with("chunk_text size cap: 262144 bytes (LCG_CHUNK_TEXT_MAX_BYTES)"),
            "{chunk}"
        );
    }
}
