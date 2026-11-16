//! Binary-level coverage for issue #346 (configurable Anthropic endpoint / auth).
//!
//! Spawns the real binary in standalone `--mcp-stdio` mode — the same `bootstrap_app_state`
//! extractor-resolution path the socket service uses — and asserts on the startup log
//! (effective redacted endpoint, auth scheme, notices), the startup error for each invalid
//! gateway setting (SC-005), lazy validation when another provider wins (ADR-0331), and that no
//! credential or query-string secret ever reaches stderr (SC-004 / FR-010).

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;

mod common;
use common::{binary_path, spawn_stub_embedder, McpClient};

/// Every env var that could leak in from a developer's shell and change provider resolution.
const SCRUBBED: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "LCG_ANTHROPIC_ENDPOINT",
    "LCG_ANTHROPIC_OMIT_MODEL",
    "LCG_ANTHROPIC_VERSION",
    "LCG_ANTHROPIC_VERSION_IN",
    "LCG_ANTHROPIC_PROMPT_CACHING",
    "LCG_EXTRACTION_URL",
    "LCG_EXTRACTION_API_KEY",
    "LCG_RECORD_LLM",
    "LCG_REPLAY_LLM",
];

fn base_cmd(dir: &TempDir, embedder_port: u16) -> Command {
    let mut cmd = Command::new(binary_path());
    cmd.env("LCG_DB_PATH", dir.path().join("test.db").to_str().unwrap())
        .env("LCG_WAL_DIR", dir.path().join("wal").to_str().unwrap())
        .env(
            "LCG_SOCKET_PATH",
            dir.path().join("unused.sock").to_str().unwrap(),
        )
        .args([
            "--mcp-stdio",
            "--embedder-http",
            &format!("http://127.0.0.1:{embedder_port}/v1/embeddings"),
        ]);
    for v in SCRUBBED {
        cmd.env_remove(v);
    }
    cmd
}

/// Starts the service, confirms it serves, SIGTERMs it and returns the full stderr.
fn run_and_capture_stderr(cmd: Command) -> String {
    let mut client = McpClient::spawn_capturing_stderr(cmd);
    client.initialize();
    let status = client.call_tool("knowledge_status", json!({}));
    assert!(
        status["result"]["isError"].as_bool() != Some(true),
        "startup should have succeeded: {status:?}"
    );
    let rc = unsafe { libc::kill(client.pid() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill failed: {}", std::io::Error::last_os_error());
    client.wait_for_exit(Duration::from_secs(10));
    client.collect_stderr().join("")
}

/// Runs a config expected to fail at startup; returns (success, stderr).
fn run_expect_failure(mut cmd: Command) -> (bool, String) {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .expect("spawn liminis-context-graph");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout),
    )
}

#[test]
fn default_config_logs_stock_endpoint_and_api_key_scheme() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_API_KEY", "KEY-DEFAULT-SECRET");
    let stderr = run_and_capture_stderr(cmd);
    assert!(
        stderr.contains(
            "extractor: provider=anthropic, transport=http, \
             endpoint=https://api.anthropic.com/v1/messages, auth=x-api-key"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("operator-chosen"), "{stderr}");
    assert!(!stderr.contains("KEY-DEFAULT-SECRET"), "{stderr}");
}

#[test]
fn bedrock_style_config_logs_redacted_endpoint_and_bearer_scheme_without_secrets() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env(
        "LCG_ANTHROPIC_ENDPOINT",
        "https://user:PW-SECRET@gw.example.com/dep/v1/invoke?code=QUERY-SECRET",
    )
    .env("ANTHROPIC_AUTH_TOKEN", "TOKEN-SECRET")
    .env("ANTHROPIC_API_KEY", "KEY-SECRET")
    .env("LCG_ANTHROPIC_OMIT_MODEL", "true")
    .env("LCG_ANTHROPIC_VERSION", "bedrock-2023-05-31")
    .env("LCG_ANTHROPIC_VERSION_IN", "body")
    .env("LCG_ANTHROPIC_PROMPT_CACHING", "off");
    let stderr = run_and_capture_stderr(cmd);

    assert!(
        stderr.contains("provider=anthropic, transport=http, endpoint=https://gw.example.com/dep/v1/invoke?<redacted>, auth=bearer"),
        "{stderr}"
    );
    for frag in [
        "model_in_body=false",
        "anthropic_version_in=body",
        "anthropic_version=bedrock-2023-05-31",
        "prompt_caching=off",
        "ANTHROPIC_API_KEY ignored",
        "operator-chosen endpoint",
    ] {
        assert!(stderr.contains(frag), "missing `{frag}` in: {stderr}");
    }
    // SC-004: no credential or query/userinfo secret anywhere in stderr.
    for secret in ["TOKEN-SECRET", "KEY-SECRET", "QUERY-SECRET", "PW-SECRET"] {
        assert!(!stderr.contains(secret), "{secret} leaked: {stderr}");
    }
}

#[test]
fn bearer_token_alone_selects_the_anthropic_provider() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_AUTH_TOKEN", "TOKEN-ONLY-SECRET")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9/");
    let stderr = run_and_capture_stderr(cmd);
    assert!(
        stderr.contains(
            "provider=anthropic, transport=http, endpoint=http://127.0.0.1:9/v1/messages, \
             auth=bearer"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("TOKEN-ONLY-SECRET"), "{stderr}");
}

#[test]
fn bearer_token_does_not_displace_a_configured_local_extractor() {
    // ANTHROPIC_AUTH_TOKEN is commonly exported for other Anthropic tooling; with
    // LCG_EXTRACTION_URL configured and no gateway URL setting it must not silently reroute
    // extraction (and the token) to api.anthropic.com.
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_AUTH_TOKEN", "TOKEN-DEFER-SECRET").env(
        "LCG_EXTRACTION_URL",
        "http://127.0.0.1:9/v1/chat/completions",
    );
    let stderr = run_and_capture_stderr(cmd);
    assert!(stderr.contains("provider=local"), "{stderr}");
    assert!(!stderr.contains("provider=anthropic"), "{stderr}");
    assert!(
        stderr.contains("ANTHROPIC_AUTH_TOKEN is set but LCG_EXTRACTION_URL selects"),
        "{stderr}"
    );
    assert!(!stderr.contains("TOKEN-DEFER-SECRET"), "{stderr}");
}

#[test]
fn bearer_token_with_gateway_url_wins_over_local_extraction_url() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_AUTH_TOKEN", "TOKEN-GW-SECRET")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9/")
        .env(
            "LCG_EXTRACTION_URL",
            "http://127.0.0.1:9/v1/chat/completions",
        );
    let stderr = run_and_capture_stderr(cmd);
    assert!(stderr.contains("provider=anthropic"), "{stderr}");
    assert!(!stderr.contains("TOKEN-GW-SECRET"), "{stderr}");
}

#[test]
fn bearer_token_to_stock_endpoint_does_not_claim_operator_chosen_endpoint() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_AUTH_TOKEN", "TOKEN-STOCK-SECRET");
    let stderr = run_and_capture_stderr(cmd);
    assert!(
        stderr.contains("endpoint=https://api.anthropic.com/v1/messages, auth=bearer"),
        "{stderr}"
    );
    assert!(!stderr.contains("operator-chosen"), "{stderr}");
}

#[test]
fn base_url_alone_does_not_select_anthropic() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_BASE_URL", "https://gw.example.com");
    let stderr = run_and_capture_stderr(cmd);
    assert!(stderr.contains("provider=none"), "{stderr}");
    assert!(
        stderr.contains("Anthropic gateway settings ignored")
            && stderr.contains("ANTHROPIC_BASE_URL"),
        "{stderr}"
    );
}

#[test]
fn explicit_extractor_flag_wins_and_does_not_validate_gateway_settings() {
    // ADR-0331: a stale/garbage ANTHROPIC_BASE_URL inherited from the shell must not break an
    // explicit --extractor-http run; the log names the ignored setting, never its value.
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.args(["--extractor-http", "http://127.0.0.1:9/v1"])
        .env("ANTHROPIC_API_KEY", "k")
        .env("ANTHROPIC_BASE_URL", "garbage-VALUE-xyz");
    let stderr = run_and_capture_stderr(cmd);
    assert!(stderr.contains("provider=local"), "{stderr}");
    assert!(
        stderr.contains("Anthropic gateway settings ignored")
            && stderr.contains("ANTHROPIC_BASE_URL"),
        "{stderr}"
    );
    assert!(!stderr.contains("garbage-VALUE-xyz"), "{stderr}");
}

#[test]
fn anthropic_base_url_flag_wins_over_env() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.args(["--anthropic-base-url", "http://127.0.0.1:9/flag"])
        .env("ANTHROPIC_API_KEY", "k")
        .env("ANTHROPIC_BASE_URL", "http://127.0.0.1:9/env");
    let stderr = run_and_capture_stderr(cmd);
    assert!(
        stderr.contains("endpoint=http://127.0.0.1:9/flag/v1/messages"),
        "{stderr}"
    );
}

/// (env vars, extra CLI args, substring the startup error must contain).
type Case<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str], &'a str);

/// SC-005: each documented invalid configuration fails startup with an error naming the setting.
#[test]
fn invalid_gateway_config_fails_startup_naming_the_setting() {
    let cases: &[Case] = &[
        (
            &[("ANTHROPIC_BASE_URL", "not a url")],
            &[],
            "ANTHROPIC_BASE_URL",
        ),
        (
            &[("ANTHROPIC_BASE_URL", "https://gw/v1/messages")],
            &[],
            "LCG_ANTHROPIC_ENDPOINT",
        ),
        (
            &[("LCG_ANTHROPIC_ENDPOINT", "ftp://gw/x")],
            &[],
            "LCG_ANTHROPIC_ENDPOINT",
        ),
        (&[("ANTHROPIC_AUTH_TOKEN", "")], &[], "ANTHROPIC_AUTH_TOKEN"),
        (&[("ANTHROPIC_BASE_URL", "")], &[], "ANTHROPIC_BASE_URL"),
        (
            &[("LCG_ANTHROPIC_VERSION_IN", "body")],
            &[],
            "LCG_ANTHROPIC_VERSION",
        ),
        (
            &[("LCG_ANTHROPIC_VERSION_IN", "query")],
            &[],
            "LCG_ANTHROPIC_VERSION_IN",
        ),
        (
            &[("LCG_ANTHROPIC_OMIT_MODEL", "maybe")],
            &[],
            "LCG_ANTHROPIC_OMIT_MODEL",
        ),
        (
            &[("LCG_ANTHROPIC_PROMPT_CACHING", "sometimes")],
            &[],
            "LCG_ANTHROPIC_PROMPT_CACHING",
        ),
        (
            &[],
            &["--anthropic-base-url", "file:///etc"],
            "--anthropic-base-url",
        ),
        (&[], &["--anthropic-endpoint", ""], "--anthropic-endpoint"),
    ];
    for (env, args, needle) in cases {
        let dir = TempDir::new().unwrap();
        let mut cmd = base_cmd(&dir, spawn_stub_embedder());
        cmd.env("ANTHROPIC_API_KEY", "k").args(*args);
        for (k, v) in *env {
            cmd.env(k, v);
        }
        let (ok, stderr) = run_expect_failure(cmd);
        assert!(!ok, "{env:?} {args:?} should fail startup: {stderr}");
        assert!(
            stderr.contains(needle),
            "{env:?} {args:?}: error should name `{needle}`: {stderr}"
        );
    }
}

#[test]
fn invalid_config_error_does_not_leak_query_secret() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base_cmd(&dir, spawn_stub_embedder());
    cmd.env("ANTHROPIC_API_KEY", "k")
        .env("LCG_ANTHROPIC_ENDPOINT", "ftp://gw/x?code=QUERY-SECRET");
    let (ok, stderr) = run_expect_failure(cmd);
    assert!(!ok);
    assert!(!stderr.contains("QUERY-SECRET"), "{stderr}");
}
