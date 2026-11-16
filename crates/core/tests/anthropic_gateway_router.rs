// `LlmRouter::from_config_with` shares one resolved Anthropic endpoint config between the
// primary and fallback leaves (#346). Lives in its own test binary (own process) because it sets
// `LCG_EXTRACTION_LLM`, which the router reads from the process environment.

use std::sync::{Arc, Mutex};

use lcg_core::anthropic_config::{AnthropicCliArgs, AnthropicEndpointConfig};
use lcg_core::extractor::Extractor;
use lcg_core::llm_router::LlmRouter;
use lcg_core::telemetry::NoopSink;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

type Seen = (String, Vec<String>, serde_json::Value);

#[tokio::test]
async fn primary_and_fallback_share_the_gateway_config() {
    std::env::set_var("LCG_EXTRACTION_LLM", "primary-model:fallback-model");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // (request line, lower-cased headers, body) per request.
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_task = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut n = 0;
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            n += 1;
            let first = n == 1;
            let seen = Arc::clone(&seen_task);
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut r = BufReader::new(r);
                let mut line = String::new();
                r.read_line(&mut line).await.unwrap();
                let mut headers = Vec::new();
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).await.unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    let h = h.trim_end().to_lowercase();
                    if let Some(v) = h.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    headers.push(h);
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).await.ok();
                seen.lock().unwrap().push((
                    line.trim_end().to_string(),
                    headers,
                    serde_json::from_slice(&body).unwrap_or_default(),
                ));
                // First request (primary) fails; the next (fallback) succeeds.
                let (status, resp) = if first {
                    ("500 ERR", "{}".to_string())
                } else {
                    (
                        "200 OK",
                        serde_json::json!({
                            "content": [{"type": "text", "text": "[\"Person\"]"}],
                            "usage": {"input_tokens": 1, "output_tokens": 1}
                        })
                        .to_string(),
                    )
                };
                let out = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                    resp.len()
                );
                let _ = w.write_all(out.as_bytes()).await;
            });
        }
    });

    let m: std::collections::HashMap<String, String> = [
        (
            "LCG_ANTHROPIC_ENDPOINT",
            format!("http://{addr}/dep/v1/invoke"),
        ),
        ("ANTHROPIC_AUTH_TOKEN", "tok-shared".to_string()),
        ("LCG_ANTHROPIC_OMIT_MODEL", "true".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let cfg = AnthropicEndpointConfig::resolve(
        &|k: &str| m.get(k).cloned(),
        &AnthropicCliArgs::default(),
    )
    .unwrap();

    let router = LlmRouter::from_config_with(Arc::new(NoopSink), cfg, |e, _| e);
    let out = router
        .classify_entities(&[("Alice", "a person")], None)
        .await
        .expect("fallback should answer after the primary fails");
    assert_eq!(out, vec!["Person".to_string()]);

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "primary then fallback");
    for (line, headers, body) in seen.iter() {
        assert_eq!(line, "POST /dep/v1/invoke HTTP/1.1");
        assert!(headers.contains(&"authorization: bearer tok-shared".to_string()));
        assert!(!headers.iter().any(|h| h.starts_with("x-api-key:")));
        assert!(body.get("model").is_none());
    }
}
