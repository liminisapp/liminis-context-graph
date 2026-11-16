// Integration tests for the configurable Anthropic endpoint / auth / request shape (#346).
//
// A loopback stub captures the raw request (request line, headers, JSON body) for every
// Anthropic-backed call path — extract (entities + edges), classify_entities, classify_relations,
// consolidate_summary and judge_duplicates — so each path is asserted against the same
// configured URL, auth and body shape (FR-009), and the default configuration is pinned
// byte-for-byte (FR-003 / SC-003).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use lcg_core::anthropic_config::{AnthropicCliArgs, AnthropicEndpointConfig};
use lcg_core::dedup_judge::{DedupSide, DuplicatePair};
use lcg_core::extractor::{AnthropicExtractor, ExtractOptions, Extractor};
use lcg_core::telemetry::{NoopSink, TelemetrySink};
use lcg_core::types::SourceType;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
struct Captured {
    request_line: String,
    /// Lower-cased `name: value` lines.
    headers: Vec<String>,
    body: Value,
}

impl Captured {
    fn path(&self) -> &str {
        self.request_line.split(' ').nth(1).unwrap_or("")
    }
    fn header(&self, name: &str) -> Option<String> {
        let prefix = format!("{}:", name.to_lowercase());
        self.headers
            .iter()
            .find(|h| h.starts_with(&prefix))
            .map(|h| h[prefix.len()..].trim().to_string())
    }
}

struct Stub {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Captured>>>,
    responses: Arc<Mutex<VecDeque<(u16, String)>>>,
}

impl Stub {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
        let responses: Arc<Mutex<VecDeque<(u16, String)>>> = Arc::new(Mutex::new(VecDeque::new()));
        let (rq, rs) = (Arc::clone(&requests), Arc::clone(&responses));
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let (rq, rs) = (Arc::clone(&rq), Arc::clone(&rs));
                tokio::spawn(async move {
                    let (read_half, mut write_half) = stream.into_split();
                    let mut reader = BufReader::new(read_half);
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).await.unwrap();
                    let mut headers = Vec::new();
                    let mut len = 0usize;
                    loop {
                        let mut line = String::new();
                        let n = reader.read_line(&mut line).await.unwrap_or(0);
                        if n == 0 || line == "\r\n" {
                            break;
                        }
                        let lower = line.trim_end().to_lowercase();
                        if let Some(v) = lower.strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                        headers.push(lower);
                    }
                    let mut body = vec![0u8; len];
                    reader.read_exact(&mut body).await.ok();
                    rq.lock().unwrap().push(Captured {
                        request_line: request_line.trim_end().to_string(),
                        headers,
                        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                    });
                    let (status, resp_body) = rs
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or((500, "{\"error\":\"no response queued\"}".to_string()));
                    let reason = if status == 200 { "OK" } else { "ERR" };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = write_half.write_all(response.as_bytes()).await;
                });
            }
        });
        Stub {
            addr,
            requests,
            responses,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn queue(&self, status: u16, body: Value) {
        self.responses
            .lock()
            .unwrap()
            .push_back((status, body.to_string()));
    }

    fn take(&self) -> Vec<Captured> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}

fn text_response(text: &str) -> Value {
    json!({
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": text}],
        "usage": {"input_tokens": 10, "output_tokens": 5}
    })
}

fn entity_response() -> Value {
    json!({
        "stop_reason": "tool_use",
        "content": [{"type": "tool_use", "id": "t1", "name": "extract_entities",
            "input": {"entities": [{"name": "Alice", "entity_type": "Person", "summary": "A person"}]}}],
        "usage": {"input_tokens": 10, "output_tokens": 5}
    })
}

fn edge_response() -> Value {
    json!({
        "stop_reason": "tool_use",
        "content": [{"type": "tool_use", "id": "t2", "name": "extract_edges",
            "input": {"edges": [{"source_name": "Alice", "target_name": "Alice", "fact": "Alice is Alice", "relation_type": "IS"}]}}],
        "usage": {"input_tokens": 10, "output_tokens": 5}
    })
}

fn sink() -> Arc<dyn TelemetrySink> {
    Arc::new(NoopSink)
}

fn resolved(pairs: &[(&str, &str)], cli: AnthropicCliArgs) -> AnthropicEndpointConfig {
    let m: std::collections::HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    AnthropicEndpointConfig::resolve(&|k: &str| m.get(k).cloned(), &cli).unwrap()
}

fn opts(body: &str) -> ExtractOptions<'_> {
    ExtractOptions {
        episode_body: body,
        group_id: "g1",
        source_type: SourceType::Text,
        custom_instructions: None,
        reference_time: "2026-01-01T00:00:00Z",
        ontology: None,
        chunk_key: None,
    }
}

/// Drives all five Anthropic-backed call paths (six HTTP requests: extract makes two) against
/// `stub` and returns the captured requests, asserting each call succeeded.
async fn drive_all_paths(ex: &AnthropicExtractor, stub: &Stub) -> Vec<Captured> {
    // extract → entities request then edges request
    stub.queue(200, entity_response());
    stub.queue(200, edge_response());
    ex.extract(opts("Alice met Alice."))
        .await
        .expect("extract should succeed");

    stub.queue(200, text_response("[\"Person\"]"));
    let ents = [("Alice", "a person")];
    ex.classify_entities(&ents, None)
        .await
        .expect("classify_entities should succeed");

    stub.queue(200, text_response("[\"IS\"]"));
    let edges = [("Alice is Alice", "RELATES_TO")];
    let allowed = [("IS".to_string(), None)];
    ex.classify_relations(&edges, &allowed)
        .await
        .expect("classify_relations should succeed");

    stub.queue(200, text_response("{\"summary\": \"Alice is a person.\"}"));
    ex.consolidate_summary("Alice", "A person", "A human")
        .await
        .expect("consolidate_summary should succeed");

    stub.queue(
        200,
        text_response("{\"verdicts\": [{\"id\": 0, \"duplicate\": true}]}"),
    );
    let side = |n: &str| DedupSide {
        name: n.to_string(),
        entity_type: "Person".to_string(),
        summary: "s".to_string(),
    };
    let pairs = [DuplicatePair {
        existing: side("Alice"),
        incoming: side("Alicia"),
    }];
    ex.judge_duplicates(&pairs)
        .await
        .expect("judge_duplicates should succeed");

    let reqs = stub.take();
    assert_eq!(
        reqs.len(),
        6,
        "extract=2 + classify×2 + consolidate + judge"
    );
    reqs
}

// ── SC-003 / FR-003: default configuration is byte-for-byte unchanged ───────────

#[tokio::test]
async fn default_config_request_shape_is_unchanged_on_every_path() {
    let stub = Stub::start().await;
    let ex = AnthropicExtractor::with_url(
        "claude-haiku-4-5-20251001".into(),
        "test-key".into(),
        stub.url("/v1/messages"),
        sink(),
    );
    for r in drive_all_paths(&ex, &stub).await {
        assert_eq!(r.request_line, "POST /v1/messages HTTP/1.1");
        assert_eq!(r.header("x-api-key").as_deref(), Some("test-key"));
        assert_eq!(r.header("anthropic-version").as_deref(), Some("2023-06-01"));
        assert!(r.header("authorization").is_none());
        assert!(
            r.header("anthropic-beta").is_none(),
            "haiku: no beta header"
        );
        assert_eq!(r.body["model"], "claude-haiku-4-5-20251001");
        assert!(r.body.get("anthropic_version").is_none());
        assert!(r.body["system"].is_string(), "haiku: plain system string");
    }
}

#[tokio::test]
async fn default_config_sonnet_keeps_beta_header_and_cache_control() {
    let stub = Stub::start().await;
    let ex = AnthropicExtractor::with_url(
        "claude-sonnet-4-5".into(),
        "test-key".into(),
        stub.url("/v1/messages"),
        sink(),
    );
    for r in drive_all_paths(&ex, &stub).await {
        assert_eq!(
            r.header("anthropic-beta").as_deref(),
            Some("prompt-caching-2024-07-31")
        );
        assert_eq!(r.body["system"][0]["cache_control"]["type"], "ephemeral");
    }
}

// ── SC-001 / User Story 1: plain proxy via base URL ───────────────────────────

#[tokio::test]
async fn base_url_proxy_posts_to_base_plus_v1_messages_with_api_key() {
    let stub = Stub::start().await;
    let cfg = resolved(
        &[
            ("ANTHROPIC_BASE_URL", &format!("http://{}/", stub.addr)),
            ("ANTHROPIC_API_KEY", "proxy-key"),
        ],
        AnthropicCliArgs::default(),
    );
    let ex =
        AnthropicExtractor::with_model("claude-haiku-4-5-20251001".into(), String::new(), sink())
            .with_endpoint_config(cfg);
    for r in drive_all_paths(&ex, &stub).await {
        assert_eq!(r.path(), "/v1/messages");
        assert_eq!(r.header("x-api-key").as_deref(), Some("proxy-key"));
        assert_eq!(r.header("anthropic-version").as_deref(), Some("2023-06-01"));
        assert_eq!(r.body["model"], "claude-haiku-4-5-20251001");
    }
}

// ── User Story 2: Bearer auth ─────────────────────────────────────────────────

#[tokio::test]
async fn bearer_token_replaces_x_api_key() {
    let stub = Stub::start().await;
    let cfg = resolved(
        &[
            ("ANTHROPIC_BASE_URL", &format!("http://{}", stub.addr)),
            ("ANTHROPIC_AUTH_TOKEN", "tok-123"),
            ("ANTHROPIC_API_KEY", "should-be-ignored"),
        ],
        AnthropicCliArgs::default(),
    );
    let ex =
        AnthropicExtractor::with_model("claude-haiku-4-5-20251001".into(), String::new(), sink())
            .with_endpoint_config(cfg);
    for r in drive_all_paths(&ex, &stub).await {
        // The stub lower-cases whole header lines, so the scheme and token compare lower-case.
        assert_eq!(r.header("authorization").as_deref(), Some("bearer tok-123"));
        assert!(r.header("x-api-key").is_none(), "exactly one auth scheme");
    }
}

// ── SC-002 / User Story 3: Bedrock-style gateway ──────────────────────────────

#[tokio::test]
async fn bedrock_gateway_contract_holds_on_every_path() {
    let stub = Stub::start().await;
    let cfg = resolved(
        &[
            ("LCG_ANTHROPIC_ENDPOINT", &stub.url("/dep/v1/invoke")),
            ("ANTHROPIC_AUTH_TOKEN", "tok-bedrock"),
            ("LCG_ANTHROPIC_OMIT_MODEL", "true"),
            ("LCG_ANTHROPIC_VERSION", "bedrock-2023-05-31"),
            ("LCG_ANTHROPIC_VERSION_IN", "body"),
            ("LCG_ANTHROPIC_PROMPT_CACHING", "off"),
        ],
        AnthropicCliArgs::default(),
    );
    // A Sonnet model: model-specific behaviour (is_sonnet) must survive `model` omission, and
    // prompt-caching=off must then suppress both the beta header and cache_control.
    let ex = AnthropicExtractor::with_model("claude-sonnet-4-5".into(), String::new(), sink())
        .with_endpoint_config(cfg);
    for r in drive_all_paths(&ex, &stub).await {
        assert_eq!(r.request_line, "POST /dep/v1/invoke HTTP/1.1");
        assert_eq!(
            r.header("authorization").as_deref(),
            Some("bearer tok-bedrock")
        );
        assert!(r.header("x-api-key").is_none());
        assert!(
            r.header("anthropic-version").is_none(),
            "version is in the body, not a header"
        );
        assert!(r.header("anthropic-beta").is_none());
        assert!(r.body.get("model").is_none(), "model omitted: {}", r.body);
        assert_eq!(r.body["anthropic_version"], "bedrock-2023-05-31");
        assert!(
            r.body["system"].is_string(),
            "cache_control suppressed with prompt caching off"
        );
        assert!(r.body["max_tokens"].is_number());
        assert!(r.body["messages"].is_array());
    }
}

#[tokio::test]
async fn omit_model_alone_keeps_default_headers_and_other_fields() {
    let stub = Stub::start().await;
    let cfg = resolved(
        &[
            ("ANTHROPIC_BASE_URL", &format!("http://{}", stub.addr)),
            ("ANTHROPIC_API_KEY", "k"),
            ("LCG_ANTHROPIC_OMIT_MODEL", "1"),
        ],
        AnthropicCliArgs::default(),
    );
    let ex =
        AnthropicExtractor::with_model("claude-haiku-4-5-20251001".into(), String::new(), sink())
            .with_endpoint_config(cfg);
    for r in drive_all_paths(&ex, &stub).await {
        assert!(r.body.get("model").is_none());
        assert!(r.body.get("anthropic_version").is_none());
        assert_eq!(r.header("anthropic-version").as_deref(), Some("2023-06-01"));
    }
}

// ── FR-010: credentials / query secrets never reach error text ────────────────

#[tokio::test]
async fn error_text_never_contains_query_secret_or_credentials() {
    let stub = Stub::start().await;
    let cfg = resolved(
        &[
            (
                "LCG_ANTHROPIC_ENDPOINT",
                &stub.url("/dep/v1/invoke?code=URL-SECRET"),
            ),
            ("ANTHROPIC_AUTH_TOKEN", "TOKEN-SECRET"),
        ],
        AnthropicCliArgs::default(),
    );
    let ex =
        AnthropicExtractor::with_model("claude-haiku-4-5-20251001".into(), String::new(), sink())
            .with_endpoint_config(cfg);

    // 401 on each of the four `error_for_status()` paths.
    let mut errors: Vec<String> = Vec::new();
    stub.queue(401, json!({"error": "unauthorized"}));
    errors.push(
        ex.consolidate_summary("A", "b", "c")
            .await
            .unwrap_err()
            .to_string(),
    );
    stub.queue(401, json!({"error": "unauthorized"}));
    errors.push(
        ex.classify_entities(&[("A", "b")], None)
            .await
            .unwrap_err()
            .to_string(),
    );
    stub.queue(401, json!({"error": "unauthorized"}));
    errors.push(
        ex.classify_relations(&[("f", "t")], &[("T".to_string(), None)])
            .await
            .unwrap_err()
            .to_string(),
    );
    // Connection failure (nothing listening on the URL's port).
    let dead =
        AnthropicExtractor::with_model("claude-haiku-4-5-20251001".into(), String::new(), sink())
            .with_endpoint_config(resolved(
                &[
                    (
                        "LCG_ANTHROPIC_ENDPOINT",
                        "http://127.0.0.1:1/x?code=URL-SECRET",
                    ),
                    ("ANTHROPIC_AUTH_TOKEN", "TOKEN-SECRET"),
                ],
                AnthropicCliArgs::default(),
            ));
    errors.push(
        dead.consolidate_summary("A", "b", "c")
            .await
            .unwrap_err()
            .to_string(),
    );

    for e in &errors {
        assert!(!e.contains("URL-SECRET"), "query secret leaked: {e}");
        assert!(!e.contains("TOKEN-SECRET"), "token leaked: {e}");
    }
}
