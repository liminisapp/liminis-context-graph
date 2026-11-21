// Issue #808: LCG_REPLAY_EMBED_RETRIES / LCG_REPLAY_EMBED_BACKOFF_MS /
// LCG_REPLAY_EMBED_BACKOFF_MAX_MS are validated once at startup, before any database open or
// socket bind, and an invalid value names itself. The real binary runs in its own process so the
// environment cannot race the in-process suite.

#[cfg(unix)]
#[path = "common/mod.rs"]
mod common;

#[cfg(unix)]
mod replay_embed_retry_env_tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use tempfile::TempDir;

    use super::common::ChildGuard;

    const VARS: [&str; 3] = [
        "LCG_REPLAY_EMBED_RETRIES",
        "LCG_REPLAY_EMBED_BACKOFF_MS",
        "LCG_REPLAY_EMBED_BACKOFF_MAX_MS",
    ];

    fn base_cmd(dir: &TempDir) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_liminis-context-graph"));
        cmd.current_dir(dir.path())
            .env("LCG_DB_PATH", dir.path().join("db/liminis.db"))
            .env("LCG_SOCKET_PATH", dir.path().join("service.sock"))
            .args(["--embedder-http", "http://127.0.0.1:1/v1/embeddings"])
            .args(["--extractor-http", "http://127.0.0.1:1/v1/chat/completions"]);
        for v in VARS {
            cmd.env_remove(v);
        }
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
            !dir.path().join("db").exists(),
            "db created for {var}={value:?}"
        );
        assert!(!dir.path().join("service.sock").exists());
    }

    #[test]
    fn invalid_retries_fail_fast() {
        for v in ["many", "-1", "11", "", "2.5"] {
            assert_fails_fast("LCG_REPLAY_EMBED_RETRIES", v);
        }
    }

    #[test]
    fn invalid_backoff_values_fail_fast() {
        for v in ["fast", "0", "60001", ""] {
            assert_fails_fast("LCG_REPLAY_EMBED_BACKOFF_MS", v);
        }
        for v in ["slow", "0", "300001", ""] {
            assert_fails_fast("LCG_REPLAY_EMBED_BACKOFF_MAX_MS", v);
        }
    }

    #[test]
    fn cap_below_base_fails_fast_naming_the_cap() {
        let dir = TempDir::new().unwrap();
        let out = base_cmd(&dir)
            .env("LCG_REPLAY_EMBED_BACKOFF_MS", "2000")
            .env("LCG_REPLAY_EMBED_BACKOFF_MAX_MS", "1000")
            .output()
            .expect("spawn binary");
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("FATAL"), "{stderr}");
        assert!(
            stderr.contains("LCG_REPLAY_EMBED_BACKOFF_MAX_MS"),
            "{stderr}"
        );
    }

    #[test]
    fn valid_values_are_logged_at_startup() {
        let dir = TempDir::new().unwrap();
        let mut cmd = base_cmd(&dir);
        cmd.env("LCG_REPLAY_EMBED_RETRIES", "7")
            .env("LCG_REPLAY_EMBED_BACKOFF_MS", "250")
            .env("LCG_REPLAY_EMBED_BACKOFF_MAX_MS", "4000")
            .stderr(Stdio::piped());
        let mut child = ChildGuard::spawn(cmd);
        let stderr = child.stderr.take().expect("child stderr");
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("read stderr");
            if line.contains("replay embedding retry") {
                assert!(line.contains("7 retries"), "{line}");
                assert!(line.contains("250ms base"), "{line}");
                assert!(line.contains("4000ms cap"), "{line}");
                return;
            }
        }
        panic!("service exited without logging the replay embedding retry line");
    }
}
