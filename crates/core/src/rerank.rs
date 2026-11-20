//! Cross-encoder reranker client (issue #796).
//!
//! The local-inference sidecar serves a small cross-encoder (bge-reranker class, CoreML) at
//! `POST /v1/rerank`. The wire shape follows the de-facto Cohere/Jina convention:
//!
//! ```text
//! request : {"model": "...", "query": "...", "documents": ["...", ...], "top_n": N?}
//! response: {"results": [{"index": 0, "relevance_score": 1.23}, ...]}
//! ```
//!
//! `relevance_score` is the cross-encoder's **raw logit** (unbounded, higher = more relevant);
//! LCG squashes it with a sigmoid before blending it with belief and recency signals
//! ([`crate::passage_rerank`]). Truncation to the model's input limit happens sidecar-side.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Value};

use crate::{error::Error, sidecar_client::SidecarClient};

/// Route served by the sidecar.
pub const RERANK_ROUTE: &str = "/v1/rerank";

/// Scores `(query, document)` pairs. Returns one score per document, in input order.
pub trait Reranker: Send + Sync {
    fn rerank<'a>(
        &'a self,
        query: &'a str,
        documents: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<f64>, Error>>;
}

/// [`Reranker`] backed by the sidecar's `/v1/rerank` route.
pub struct SidecarReranker {
    client: Arc<SidecarClient>,
    model: String,
}

impl SidecarReranker {
    pub fn new(client: Arc<SidecarClient>, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

/// Parses a `/v1/rerank` response into scores aligned to the `n` input documents. Rejects a
/// response that is missing a result for any document, repeats an index, or carries a
/// non-finite score — a partially-scored head cannot be ordered meaningfully.
pub fn parse_rerank_response(resp: &Value, n: usize) -> Result<Vec<f64>, Error> {
    let results = resp["results"]
        .as_array()
        .ok_or_else(|| Error::Ipc("rerank response missing `results`".to_string()))?;
    let mut scores: Vec<Option<f64>> = vec![None; n];
    for r in results {
        let idx = r["index"]
            .as_u64()
            .map(|i| i as usize)
            .filter(|i| *i < n)
            .ok_or_else(|| Error::Ipc("rerank result has an invalid `index`".to_string()))?;
        let score = r["relevance_score"]
            .as_f64()
            .filter(|s| s.is_finite())
            .ok_or_else(|| {
                Error::Ipc("rerank result has a missing or non-finite `relevance_score`".into())
            })?;
        if scores[idx].replace(score).is_some() {
            return Err(Error::Ipc(format!("rerank result repeats index {idx}")));
        }
    }
    scores
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            s.ok_or_else(|| Error::Ipc(format!("rerank response missing a score for index {i}")))
        })
        .collect()
}

/// Per-document character cap on what is sent to the sidecar. The sidecar cuts every pair at the
/// model's token length (512) anyway, so text beyond this adds nothing but request size: a head of
/// 50 large notes would otherwise exceed the sidecar's request-body limit (Hummingbird's default
/// `maxUploadSize` is 2 MiB), and the resulting error would look like a reranker outage. 8,000
/// characters is far above 512 tokens in any language, so the cap never changes a score.
pub const MAX_RERANK_DOC_CHARS: usize = 8_000;

/// `doc` cut to at most `max` characters, on a character boundary.
fn cap_chars(doc: &str, max: usize) -> &str {
    match doc.char_indices().nth(max) {
        Some((i, _)) => &doc[..i],
        None => doc,
    }
}

impl Reranker for SidecarReranker {
    fn rerank<'a>(
        &'a self,
        query: &'a str,
        documents: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<f64>, Error>> {
        Box::pin(async move {
            if documents.is_empty() {
                return Ok(Vec::new());
            }
            let capped: Vec<&str> = documents
                .iter()
                .map(|d| cap_chars(d, MAX_RERANK_DOC_CHARS))
                .collect();
            let body = json!({
                "model": self.model,
                "query": query,
                "documents": capped,
            });
            let resp = self.client.post_json(RERANK_ROUTE, &body).await?;
            parse_rerank_response(&resp, documents.len())
        })
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn cap_chars_cuts_on_a_character_boundary() {
        assert_eq!(cap_chars("short", 10), "short");
        assert_eq!(cap_chars("abcdef", 3), "abc");
        // Multi-byte characters are never split.
        assert_eq!(cap_chars("héllo wörld", 5), "héllo");
        assert_eq!(cap_chars("日本語のテキスト", 3), "日本語");
        assert_eq!(cap_chars("", 3), "");
    }
}

type ScoreFn = Box<dyn Fn(&str, &str) -> f64 + Send + Sync>;

/// Test double: scores each pair with a caller-supplied function, or fails on demand.
pub struct MockReranker {
    score: ScoreFn,
    fail: bool,
    calls: std::sync::atomic::AtomicUsize,
    last_docs: std::sync::Mutex<Vec<String>>,
}

impl MockReranker {
    pub fn new(score: impl Fn(&str, &str) -> f64 + Send + Sync + 'static) -> Self {
        Self {
            score: Box::new(score),
            fail: false,
            calls: Default::default(),
            last_docs: Default::default(),
        }
    }

    /// A reranker whose every call fails (simulates an unreachable sidecar).
    pub fn failing() -> Self {
        let mut m = Self::new(|_, _| 0.0);
        m.fail = true;
        m
    }

    /// Number of `rerank` calls made.
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The documents passed to the most recent call.
    pub fn last_documents(&self) -> Vec<String> {
        self.last_docs.lock().unwrap().clone()
    }
}

impl Reranker for MockReranker {
    fn rerank<'a>(
        &'a self,
        query: &'a str,
        documents: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<f64>, Error>> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.last_docs.lock().unwrap() = documents.to_vec();
            if self.fail {
                return Err(Error::Ipc("mock reranker unavailable".to_string()));
            }
            Ok(documents.iter().map(|d| (self.score)(query, d)).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_orders_by_index_not_by_position() {
        let resp = json!({"results": [
            {"index": 1, "relevance_score": 2.0},
            {"index": 0, "relevance_score": -1.5},
        ]});
        assert_eq!(parse_rerank_response(&resp, 2).unwrap(), vec![-1.5, 2.0]);
    }

    #[test]
    fn parse_rejects_malformed_responses() {
        for resp in [
            json!({}),
            json!({"results": [{"index": 0, "relevance_score": 1.0}]}), // missing index 1
            json!({"results": [
                {"index": 0, "relevance_score": 1.0},
                {"index": 0, "relevance_score": 2.0},
            ]}), // duplicate
            json!({"results": [{"index": 5, "relevance_score": 1.0}]}), // out of range
            json!({"results": [{"index": 0}, {"index": 1, "relevance_score": 1.0}]}),
        ] {
            assert!(parse_rerank_response(&resp, 2).is_err(), "{resp}");
        }
    }

    #[tokio::test]
    async fn mock_reranker_scores_and_counts() {
        let m = MockReranker::new(|q, d| if d.contains(q) { 1.0 } else { 0.0 });
        let docs = vec!["alpha beta".to_string(), "gamma".to_string()];
        assert_eq!(m.rerank("beta", &docs).await.unwrap(), vec![1.0, 0.0]);
        assert_eq!(m.calls(), 1);
        assert_eq!(m.last_documents(), docs);
        assert!(MockReranker::failing().rerank("q", &docs).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sidecar_reranker_round_trips_over_uds() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = std::env::temp_dir()
            .join(format!("lcg-rr-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let mut total = 0;
                    // One read is enough for these tiny requests.
                    let n = s.read(&mut buf[total..]).await.unwrap_or(0);
                    total += n;
                    let req = String::from_utf8_lossy(&buf[..total]).to_string();
                    assert!(req.starts_with("POST /v1/rerank"), "{req}");
                    assert!(req.contains("\"documents\""));
                    let body = r#"{"results":[{"index":1,"relevance_score":3.0},{"index":0,"relevance_score":-2.0}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        let client = Arc::new(SidecarClient::new_uds(
            path.clone(),
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(2),
        ));
        let r = SidecarReranker::new(client, "bge-reranker-base");
        let docs = vec!["a".to_string(), "b".to_string()];
        assert_eq!(r.rerank("q", &docs).await.unwrap(), vec![-2.0, 3.0]);
        assert!(r.rerank("q", &[]).await.unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
