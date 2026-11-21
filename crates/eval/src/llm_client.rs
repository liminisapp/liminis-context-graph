//! One Anthropic-compatible Messages client for every model call `lcg-eval` makes itself: the
//! extraction and retrieval judges and the graph-qa planner and reader (#821, FR-019).
//!
//! The endpoint, credential scheme and request shape come from
//! [`lcg_core::anthropic_config::AnthropicEndpointConfig`] (ADR-0346), the same resolution the
//! service uses, so `ANTHROPIC_BASE_URL` / `LCG_ANTHROPIC_ENDPOINT` / `ANTHROPIC_AUTH_TOKEN` (and
//! the `--anthropic-base-url` / `--anthropic-endpoint` flags) point every eval model call at a
//! gateway or the local subscription bridge.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use lcg_core::anthropic_config::{
    AnthropicAuth, AnthropicCliArgs, AnthropicEndpointConfig, VersionPlacement, ENV_API_KEY,
    ENV_AUTH_TOKEN, ENV_BASE_URL, ENV_ENDPOINT,
};
use serde_json::{json, Value};
use tokio::time::sleep;

/// One model call: an optional system prompt, one user turn and an optional output schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRequest {
    pub model: String,
    pub system: Option<String>,
    pub user: String,
    /// JSON schema for `output_config.format`; sent closed (see [`close_schema`]).
    pub schema: Option<Value>,
    pub max_tokens: u32,
}

/// Something that answers a [`ModelRequest`] with the response text. Implemented by
/// [`MessagesClient`] and by test doubles (`graph_qa::mock::ScriptedModel`).
pub trait ModelClient: Send + Sync {
    fn complete<'a>(&'a self, req: &'a ModelRequest) -> BoxFuture<'a, Result<String, String>>;
}

/// How many model calls may fail back-to-back before the client stops making requests.
///
/// Failures are deliberately non-fatal per call for the judge, because one bad response should not
/// discard hours of scoring. That is right for transient faults and wrong for systemic ones: an
/// invalid model id, an exhausted credit balance, or a revoked key fails *every* call, and without
/// a breaker a ~6000-call scoring phase would grind through all of them.
pub const CONSECUTIVE_FAILURE_LIMIT: usize = 10;

/// The consecutive-failure breaker, split out from the HTTP path so its state transitions are
/// testable without a network round trip: closed below the limit, opens at it, any success resets.
#[derive(Debug)]
pub struct CircuitBreaker {
    consecutive: AtomicUsize,
    limit: usize,
}

impl CircuitBreaker {
    pub fn new(limit: usize) -> Self {
        Self {
            consecutive: AtomicUsize::new(0),
            limit,
        }
    }

    /// `Err` once the breaker is open — the caller must not issue the request.
    pub fn check(&self) -> Result<(), String> {
        let failures = self.consecutive.load(Ordering::Relaxed);
        if failures >= self.limit {
            return Err(format!(
                "model circuit breaker open after {failures} consecutive failures — \
                 not issuing further requests. Fix the underlying cause and re-run; \
                 responses already cached will be re-served for free."
            ));
        }
        Ok(())
    }

    /// Resets on success rather than decaying: an isolated transient fault in a
    /// multi-thousand-call run must not accumulate toward the limit over hours.
    pub fn record<T, E>(&self, result: &Result<T, E>) {
        match result {
            Ok(_) => self.consecutive.store(0, Ordering::Relaxed),
            Err(_) => {
                self.consecutive.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn consecutive_failures(&self) -> usize {
        self.consecutive.load(Ordering::Relaxed)
    }
}

/// A stalled call would otherwise block the whole run indefinitely; bound it generously since a
/// large prompt can legitimately take a while.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Cap on the raw-body fallback when the API's error JSON cannot be parsed. Bytes, not chars.
const ERROR_BODY_FALLBACK_BYTES: usize = 500;

/// True when a provider is configured at all: a credential (`ANTHROPIC_API_KEY`,
/// `ANTHROPIC_AUTH_TOKEN`) **or** a gateway URL (flag or env). A credential-less local bridge is a
/// configured provider, so it must not need a dummy API key to be used.
pub fn provider_configured(env: &dyn Fn(&str) -> Option<String>, cli: &AnthropicCliArgs) -> bool {
    let set = |k: &str| env(k).is_some_and(|v| !v.trim().is_empty());
    env(ENV_API_KEY).is_some()
        || set(ENV_AUTH_TOKEN)
        || set(ENV_BASE_URL)
        || set(ENV_ENDPOINT)
        || !cli.is_empty()
}

/// A `reqwest` error for an error message. Its `Display` includes the request URL, and the endpoint
/// can carry userinfo or a token query (`http://user:token@host/…`, `…?key=…`), so the URL is
/// dropped: a credential must never reach stderr, a log or a report.
pub fn request_error(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

/// The process environment, as [`AnthropicEndpointConfig::resolve`] wants it.
pub fn process_env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// Returns `schema` with `"additionalProperties": false` on every object schema that does not
/// already say otherwise. The Messages API's structured-output mode requires closed objects; the
/// E13 probe's planner and reader schemas (sent through `claude -p --json-schema`) are open.
pub fn close_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m {
                out.insert(k.clone(), close_schema(v));
            }
            if out.get("type").and_then(Value::as_str) == Some("object")
                && !out.contains_key("additionalProperties")
            {
                out.insert("additionalProperties".into(), json!(false));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(close_schema).collect()),
        other => other.clone(),
    }
}

/// Direct Messages API client (no `Extractor` involved), built from a resolved endpoint config.
pub struct MessagesClient {
    cfg: AnthropicEndpointConfig,
    client: reqwest::Client,
    /// Consecutive failures, reset by any success. See [`CONSECUTIVE_FAILURE_LIMIT`].
    pub(crate) breaker: CircuitBreaker,
}

impl MessagesClient {
    pub fn new(cfg: AnthropicEndpointConfig) -> Self {
        Self {
            cfg,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client with a fixed timeout is infallible to build"),
            breaker: CircuitBreaker::new(CONSECUTIVE_FAILURE_LIMIT),
        }
    }

    /// Resolves the endpoint from `cli` and the process environment.
    pub fn from_env(cli: &AnthropicCliArgs) -> Result<Self, String> {
        Ok(Self::new(AnthropicEndpointConfig::resolve(
            &process_env,
            cli,
        )?))
    }

    /// Endpoint and auth scheme for logs and reports — never the credential: the endpoint URL is
    /// shown without userinfo or query.
    pub fn describe(&self) -> String {
        self.cfg.startup_summary().replace(
            &self.cfg.url,
            &crate::retrieval::workspace::redact_endpoint(&self.cfg.url),
        )
    }

    pub fn breaker(&self) -> &CircuitBreaker {
        &self.breaker
    }

    /// The JSON body for `req`; `schema: false` drops `output_config` (the grammar fallback).
    pub fn body(&self, req: &ModelRequest, with_schema: bool) -> Value {
        let mut body = json!({
            "max_tokens": req.max_tokens,
            "messages": [{"role": "user", "content": req.user}],
            // Explicit, not inherited from the model's default: a measuring instrument must not
            // switch thinking on silently when the model changes (see the judge's history in
            // docs/history/extraction-eval-2026-04.md).
            "thinking": {"type": "disabled"}
        });
        if !self.cfg.omit_model {
            body["model"] = json!(req.model);
        }
        if let Some(s) = &req.system {
            body["system"] = json!(s);
        }
        if self.cfg.version_in == VersionPlacement::Body {
            body["anthropic_version"] = json!(self.cfg.version);
        }
        if with_schema {
            if let Some(schema) = &req.schema {
                body["output_config"] =
                    json!({"format": {"type": "json_schema", "schema": close_schema(schema)}});
            }
        }
        body
    }

    fn send<'a>(
        &'a self,
        req: &'a ModelRequest,
        with_schema: bool,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let body = self.body(req, with_schema);
            // Retry 429/529 and transport errors with exponential backoff, mirroring
            // `AnthropicExtractor`: over thousands of calls a transient fault is close to certain.
            let mut attempt = 0u32;
            let resp: Value = loop {
                let mut rb = self
                    .client
                    .post(&self.cfg.url)
                    .header("content-type", "application/json");
                rb = match &self.cfg.auth {
                    AnthropicAuth::ApiKey(k) => rb.header("x-api-key", k),
                    AnthropicAuth::Bearer(t) => rb.header("authorization", format!("Bearer {t}")),
                };
                if self.cfg.version_in == VersionPlacement::Header {
                    rb = rb.header("anthropic-version", &self.cfg.version);
                }
                let http_resp = match rb.json(&body).send().await {
                    Ok(r) => r,
                    Err(_) if attempt < 3 => {
                        sleep(Duration::from_secs(1u64 << attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    Err(e) => {
                        return Err(format!(
                            "model request failed after retries: {}",
                            request_error(e)
                        ))
                    }
                };
                let status = http_resp.status();
                if (status.as_u16() == 429 || status.as_u16() == 529) && attempt < 3 {
                    sleep(Duration::from_secs(1u64 << attempt)).await;
                    attempt += 1;
                    continue;
                }
                // The API puts the *reason* in the body; `error_for_status()` would discard it.
                if !status.is_success() {
                    let text = http_resp.text().await.unwrap_or_default();
                    // The API failed to compile the constrained-output grammar: retrying the same
                    // constrained request would time out again, so drop the constraint and let the
                    // caller parse JSON out of the unconstrained text.
                    if text.contains("Grammar compilation") && with_schema && req.schema.is_some() {
                        eprintln!(
                            "lcg-eval: grammar compilation timed out — retrying this call \
                             without the schema constraint"
                        );
                        return self.send(req, false).await;
                    }
                    let detail = serde_json::from_str::<Value>(&text)
                        .ok()
                        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                        .unwrap_or_else(|| {
                            let mut end = text.len().min(ERROR_BODY_FALLBACK_BYTES);
                            while end > 0 && !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            text[..end].to_owned()
                        });
                    return Err(format!("model request failed ({status}): {detail}"));
                }
                break http_resp
                    .json()
                    .await
                    .map_err(|e| format!("model response not JSON: {}", request_error(e)))?;
            };
            response_text(&resp)
        })
    }
}

/// The response's text: every `text` content block, joined. A bridge may split one answer over
/// several blocks.
pub fn response_text(resp: &Value) -> Result<String, String> {
    let blocks = resp["content"]
        .as_array()
        .ok_or_else(|| "model response missing content".to_string())?;
    let text: Vec<&str> = blocks
        .iter()
        .filter(|b| b["type"].as_str().is_none_or(|t| t == "text"))
        .filter_map(|b| b["text"].as_str())
        .collect();
    if text.is_empty() {
        return Err("model response has no text content".into());
    }
    Ok(text.concat())
}

impl ModelClient for MessagesClient {
    fn complete<'a>(&'a self, req: &'a ModelRequest) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            self.breaker.check()?;
            let result = self.send(req, true).await;
            self.breaker.record(&result);
            result
        })
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A one-route HTTP/1.1 responder for tests: records every request body and headers, and
    //! answers with a fixed Messages response.
    use std::sync::{Arc, Mutex};

    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Debug, Clone)]
    pub struct Seen {
        pub path: String,
        pub headers: Vec<(String, String)>,
        pub body: Value,
    }

    pub struct StubServer {
        pub url: String,
        pub seen: Arc<Mutex<Vec<Seen>>>,
    }

    /// Serves `reply_text` as the single text block of every response.
    pub async fn serve(reply_text: &str) -> StubServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = seen.clone();
        let reply =
            serde_json::json!({"content": [{"type": "text", "text": reply_text}]}).to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let log = log.clone();
                let reply = reply.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let (head_end, len) = loop {
                        let n = s.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (p + 4, len);
                        }
                    };
                    while buf.len() < head_end + len {
                        let n = s.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let mut lines = head.lines();
                    let path = lines
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("")
                        .to_string();
                    let headers = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
                        .collect();
                    let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                    log.lock().unwrap().push(Seen {
                        path,
                        headers,
                        body,
                    });
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        StubServer { url, seen }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn req(schema: Option<Value>) -> ModelRequest {
        ModelRequest {
            model: "m-1".into(),
            system: Some("be brief".into()),
            user: "Question: hi".into(),
            schema,
            max_tokens: 256,
        }
    }

    #[test]
    fn close_schema_closes_every_object_and_keeps_explicit_settings() {
        let s = json!({"type": "object", "properties": {
            "hops": {"type": "array", "items": {"type": "object", "properties": {"t": {"type": "string"}}}},
            "open": {"type": "object", "additionalProperties": true}
        }});
        let c = close_schema(&s);
        assert_eq!(c["additionalProperties"], json!(false));
        assert_eq!(
            c["properties"]["hops"]["items"]["additionalProperties"],
            json!(false)
        );
        assert_eq!(c["properties"]["open"]["additionalProperties"], json!(true));
        assert!(c["properties"]["hops"]
            .get("additionalProperties")
            .is_none());
        // Idempotent.
        assert_eq!(close_schema(&c), c);
    }

    #[test]
    fn provider_is_configured_by_a_credential_or_a_url() {
        let none = AnthropicCliArgs::default();
        assert!(!provider_configured(&env_of(&[]), &none));
        assert!(provider_configured(&env_of(&[(ENV_API_KEY, "k")]), &none));
        assert!(provider_configured(
            &env_of(&[(ENV_AUTH_TOKEN, "t")]),
            &none
        ));
        assert!(!provider_configured(
            &env_of(&[(ENV_AUTH_TOKEN, " ")]),
            &none
        ));
        assert!(provider_configured(
            &env_of(&[(ENV_BASE_URL, "http://127.0.0.1:1")]),
            &none
        ));
        let flag = AnthropicCliArgs {
            base_url: Some("http://127.0.0.1:1".into()),
            endpoint: None,
        };
        assert!(provider_configured(&env_of(&[]), &flag));
    }

    #[tokio::test]
    async fn requests_reach_the_configured_base_url_with_system_schema_and_bearer() {
        let srv = testing::serve("{\"answers\": [\"x\"]}").await;
        let cfg = AnthropicEndpointConfig::resolve(
            &env_of(&[(ENV_AUTH_TOKEN, "tok")]),
            &AnthropicCliArgs {
                base_url: Some(srv.url.clone()),
                endpoint: None,
            },
        )
        .unwrap();
        let c = MessagesClient::new(cfg);
        let schema = json!({"type": "object", "properties": {"answers": {"type": "array", "items": {"type": "string"}}}, "required": ["answers"]});
        let text = c.complete(&req(Some(schema))).await.unwrap();
        assert_eq!(text, "{\"answers\": [\"x\"]}");
        let seen = srv.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].path, "/v1/messages");
        let h = |k: &str| {
            seen[0]
                .headers
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(h("authorization").as_deref(), Some("Bearer tok"));
        assert!(h("x-api-key").is_none());
        assert_eq!(h("anthropic-version").as_deref(), Some("2023-06-01"));
        let b = &seen[0].body;
        assert_eq!(b["model"], "m-1");
        assert_eq!(b["system"], "be brief");
        assert_eq!(b["messages"][0]["content"], "Question: hi");
        assert_eq!(b["thinking"]["type"], "disabled");
        assert_eq!(
            b["output_config"]["format"]["schema"]["additionalProperties"],
            json!(false)
        );
    }

    #[tokio::test]
    async fn no_schema_no_system_sends_neither() {
        let srv = testing::serve("plain").await;
        let c = MessagesClient::new(AnthropicEndpointConfig::with_url_and_key(
            format!("{}/v1/messages", srv.url),
            "k".into(),
        ));
        let mut r = req(None);
        r.system = None;
        assert_eq!(c.complete(&r).await.unwrap(), "plain");
        let seen = srv.seen.lock().unwrap().clone();
        assert!(seen[0].body.get("system").is_none());
        assert!(seen[0].body.get("output_config").is_none());
        assert!(seen[0]
            .headers
            .iter()
            .any(|(k, v)| k == "x-api-key" && v == "k"));
    }

    #[test]
    fn response_text_joins_text_blocks() {
        let r = json!({"content": [{"type": "text", "text": "a"}, {"type": "tool_use"}, {"type": "text", "text": "b"}]});
        assert_eq!(response_text(&r).unwrap(), "ab");
        assert!(response_text(&json!({"content": []})).is_err());
        assert!(response_text(&json!({})).is_err());
    }

    #[tokio::test]
    async fn the_breaker_stops_requests_before_any_network_call() {
        let c = MessagesClient::new(AnthropicEndpointConfig::with_url_and_key(
            "http://127.0.0.1:9/v1/messages".into(),
            "k".into(),
        ));
        for _ in 0..CONSECUTIVE_FAILURE_LIMIT {
            c.breaker.record::<(), ()>(&Err(()));
        }
        let e = c.complete(&req(None)).await.unwrap_err();
        assert!(e.contains("circuit breaker open"), "{e}");
    }

    #[tokio::test]
    async fn request_errors_never_carry_the_endpoint_credential() {
        let e = reqwest::Client::new()
            .get("http://user:s3cret@127.0.0.1:1/v1/messages?key=t0ken")
            .send()
            .await
            .unwrap_err();
        // reqwest already drops userinfo from its Display, but not a token in the query.
        assert!(
            e.to_string().contains("t0ken"),
            "reqwest prints the URL: {e}"
        );
        let msg = request_error(e);
        assert!(!msg.contains("s3cret") && !msg.contains("t0ken"), "{msg}");
    }

    #[test]
    fn describe_shows_the_endpoint_without_its_credential() {
        let env = |k: &str| match k {
            "ANTHROPIC_API_KEY" => Some("k".to_string()),
            "LCG_ANTHROPIC_ENDPOINT" => {
                Some("http://user:s3cret@127.0.0.1:9/v1/messages?key=t0ken".to_string())
            }
            _ => None,
        };
        let cfg = AnthropicEndpointConfig::resolve(&env, &AnthropicCliArgs::default()).unwrap();
        let d = MessagesClient::new(cfg).describe();
        assert!(d.contains("127.0.0.1:9/v1/messages"), "{d}");
        assert!(!d.contains("s3cret") && !d.contains("t0ken"), "{d}");
    }
}
