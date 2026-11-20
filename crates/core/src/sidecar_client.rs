//! Small pooled HTTP/UDS JSON client for sidecar routes other than `/v1/embeddings` (issue #796).
//!
//! The rerank stage (`/v1/rerank`) and the query-expansion stage (`/v1/chat/completions`) reach
//! the local-inference sidecar over the same two transports the embedder uses — a Unix-domain
//! socket or plain HTTP — with the same pool and retry boundary (ADR-0016 / ADR-0039 / ADR-0040):
//!
//! - UDS: a small lazily-populated pool of held HTTP/1.1 connections. Waiting for a busy slot is
//!   bounded by `request_timeout` (a slot is held for a whole request, which for rerank and
//!   expansion takes seconds), dialling by `connect_timeout`, and each send/read attempt by
//!   `request_timeout`.
//! - A *broken connection* (the sidecar restarted, idle-closed the socket) is re-dialled and the
//!   request retried exactly once. A *timeout* is never retried.
//!
//! This deliberately duplicates the embedder's pool code rather than generalising it: the
//! embedder is the shipped, always-on hot path and is left untouched by this feature. Folding the
//! two together is a follow-up.

use std::time::Duration;

use serde_json::Value;

use crate::error::Error;

/// Default whole-request bound for one send/read attempt.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Default bound on acquiring a usable connection (slot wait + dial).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

enum Transport {
    Http {
        client: reqwest::Client,
        /// Base URL without a trailing slash or a `/v1/...` route, e.g. `http://127.0.0.1:8765`.
        base: String,
        api_key: Option<String>,
    },
    #[cfg(unix)]
    Uds { path: String, pool: UdsPool },
}

/// A JSON-over-HTTP client for one sidecar endpoint.
pub struct SidecarClient {
    transport: Transport,
    // Read only by the Unix-socket send path; the HTTP transport bakes them into `reqwest`.
    #[cfg_attr(not(unix), allow(dead_code))]
    request_timeout: Duration,
    #[cfg_attr(not(unix), allow(dead_code))]
    connect_timeout: Duration,
}

impl std::fmt::Debug for SidecarClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, target) = self.describe();
        f.debug_struct("SidecarClient")
            .field("transport", &kind)
            .field("target", &target)
            .finish()
    }
}

/// Strips a trailing `/v1/...` route (and trailing slashes) so a caller may pass either a base
/// URL or a full route URL such as the embedder's `http://host:port/v1/embeddings`.
fn normalize_base(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    match trimmed.find("/v1/") {
        Some(i) => trimmed[..i].to_string(),
        None => match trimmed.strip_suffix("/v1") {
            Some(s) => s.to_string(),
            None => trimmed.to_string(),
        },
    }
}

impl SidecarClient {
    pub fn new_http(
        url: &str,
        api_key: Option<String>,
        request_timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .connect_timeout(connect_timeout)
            .build()
            .map_err(|e| Error::Config(format!("sidecar HTTP client: {e}")))?;
        Ok(Self {
            transport: Transport::Http {
                client,
                base: normalize_base(url),
                api_key,
            },
            request_timeout,
            connect_timeout,
        })
    }

    #[cfg(unix)]
    pub fn new_uds(
        path: impl Into<String>,
        request_timeout: Duration,
        connect_timeout: Duration,
    ) -> Self {
        Self {
            transport: Transport::Uds {
                path: path.into(),
                pool: UdsPool::new(),
            },
            request_timeout,
            connect_timeout,
        }
    }

    /// `("uds"|"http", path-or-url)` for logs.
    pub fn describe(&self) -> (&'static str, String) {
        match &self.transport {
            Transport::Http { base, .. } => ("http", base.clone()),
            #[cfg(unix)]
            Transport::Uds { path, .. } => ("uds", path.clone()),
        }
    }

    /// POSTs `body` as JSON to `route` (e.g. `/v1/rerank`) and returns the parsed JSON response.
    /// Any non-2xx status is an error (the caller degrades; it never fails the search).
    pub async fn post_json(&self, route: &str, body: &Value) -> Result<Value, Error> {
        match &self.transport {
            Transport::Http {
                client,
                base,
                api_key,
            } => {
                let mut req = client.post(format!("{base}{route}")).json(body);
                if let Some(key) = api_key {
                    req = req.bearer_auth(key);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| Error::Ipc(format!("sidecar HTTP request: {e}")))?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(Error::Ipc(format!("sidecar returned status {status}")));
                }
                resp.json::<Value>()
                    .await
                    .map_err(|e| Error::Ipc(format!("parse sidecar response: {e}")))
            }
            #[cfg(unix)]
            Transport::Uds { path, pool } => {
                let bytes = hyper::body::Bytes::from(
                    serde_json::to_vec(body)
                        .map_err(|e| Error::Ipc(format!("serialize sidecar request: {e}")))?,
                );
                uds_post(
                    path,
                    pool,
                    route,
                    bytes,
                    self.request_timeout,
                    self.connect_timeout,
                )
                .await
            }
        }
    }
}

// ── UDS pool (mirrors embedder.rs; see module docs) ───────────────────────────

#[cfg(unix)]
type UdsSender = hyper::client::conn::http1::SendRequest<http_body_util::Full<hyper::body::Bytes>>;

#[cfg(unix)]
const UDS_POOL_SIZE: usize = 4;

#[cfg(unix)]
struct UdsPool {
    slots: Vec<tokio::sync::Mutex<Option<UdsSender>>>,
    cursor: std::sync::atomic::AtomicUsize,
}

#[cfg(unix)]
impl UdsPool {
    fn new() -> Self {
        Self {
            slots: (0..UDS_POOL_SIZE)
                .map(|_| tokio::sync::Mutex::new(None))
                .collect(),
            cursor: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[cfg(unix)]
async fn dial_uds(path: &str) -> Result<UdsSender, Error> {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(path)
        .await
        .map_err(|e| Error::Ipc(format!("UDS connect to {path}: {e}")))?;
    let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| Error::Ipc(format!("UDS HTTP/1.1 handshake: {e}")))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

#[cfg(unix)]
enum AttemptError {
    ConnectionBroken(Error),
    Other(Error),
}

#[cfg(unix)]
async fn send_and_read(
    sender: &mut UdsSender,
    route: &str,
    body_bytes: hyper::body::Bytes,
) -> Result<Value, AttemptError> {
    use http_body_util::{BodyExt, Full};
    use hyper::Request;

    let req = Request::builder()
        .method("POST")
        .uri(route)
        .header("content-type", "application/json")
        .header("host", "localhost")
        .body(Full::new(body_bytes))
        .map_err(|e| AttemptError::Other(Error::Ipc(format!("build UDS request: {e}"))))?;

    let resp = sender.send_request(req).await.map_err(|e| {
        AttemptError::ConnectionBroken(Error::Ipc(format!("UDS send request: {e}")))
    })?;

    if !resp.status().is_success() {
        let status = resp.status();
        // Drain so the pooled keep-alive connection's framing stays in sync.
        let _ = resp.into_body().collect().await;
        return Err(AttemptError::Other(Error::Ipc(format!(
            "sidecar returned status {status}"
        ))));
    }

    let bytes = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| AttemptError::Other(Error::Ipc(format!("UDS read response body: {e}"))))?
        .to_bytes();
    serde_json::from_slice(&bytes)
        .map_err(|e| AttemptError::Other(Error::Ipc(format!("parse sidecar response: {e}"))))
}

/// Clears the slot if the send/read span is dropped mid-flight (cancellation or timeout), so the
/// next use re-dials instead of reusing a connection in an indeterminate state.
#[cfg(unix)]
struct PoisonGuard<'a> {
    slot: &'a mut Option<UdsSender>,
    armed: bool,
}

#[cfg(unix)]
impl Drop for PoisonGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            *self.slot = None;
        }
    }
}

#[cfg(unix)]
async fn attempt_send(
    slot: &mut Option<UdsSender>,
    route: &str,
    body_bytes: hyper::body::Bytes,
    request_timeout: Duration,
) -> Result<Value, AttemptError> {
    let attempt = async {
        let mut guard = PoisonGuard { slot, armed: true };
        let result = send_and_read(
            guard
                .slot
                .as_mut()
                .expect("PoisonGuard requires a populated slot"),
            route,
            body_bytes,
        )
        .await;
        guard.armed = false;
        result
    };
    match tokio::time::timeout(request_timeout, attempt).await {
        Ok(r) => r,
        // A timeout is `Other`, never `ConnectionBroken`: it is never retried.
        Err(_) => Err(AttemptError::Other(Error::Ipc(format!(
            "UDS send request: timed out after {}ms",
            request_timeout.as_millis()
        )))),
    }
}

#[cfg(unix)]
/// Takes a pool slot. Waiting for a busy slot is bounded by `wait_timeout` (the request timeout),
/// and only the dial by `connect_timeout`. The slot's guard is held for a whole request, so for a
/// stage whose requests take seconds (rerank, expansion) a short connect bound on the *wait* would
/// fail a call queued behind a healthy in-flight one, long before its own request timeout.
async fn acquire_slot<'a>(
    path: &str,
    slot_mutex: &'a tokio::sync::Mutex<Option<UdsSender>>,
    wait_timeout: Duration,
    connect_timeout: Duration,
) -> Result<tokio::sync::MutexGuard<'a, Option<UdsSender>>, Error> {
    let mut slot = tokio::time::timeout(wait_timeout, slot_mutex.lock())
        .await
        .map_err(|_| {
            Error::Ipc(format!(
                "UDS connect: pool slot acquisition timed out after {}ms",
                wait_timeout.as_millis()
            ))
        })?;
    if slot.is_none() {
        let sender = tokio::time::timeout(connect_timeout, dial_uds(path))
            .await
            .map_err(|_| {
                Error::Ipc(format!(
                    "UDS connect: dial timed out after {}ms",
                    connect_timeout.as_millis()
                ))
            })??;
        *slot = Some(sender);
    }
    Ok(slot)
}

#[cfg(unix)]
async fn uds_post(
    path: &str,
    pool: &UdsPool,
    route: &str,
    body_bytes: hyper::body::Bytes,
    request_timeout: Duration,
    connect_timeout: Duration,
) -> Result<Value, Error> {
    let idx = pool
        .cursor
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        % pool.slots.len();
    // Prefer an idle slot: scan from the round-robin position so a call is not queued behind a
    // multi-second request on its own slot while others are free. Only when every slot is busy
    // does it wait (bounded by the request timeout) on the round-robin one.
    let idle = (0..pool.slots.len())
        .map(|off| (idx + off) % pool.slots.len())
        .find_map(|i| pool.slots[i].try_lock().ok());
    let mut slot = match idle {
        Some(mut g) => {
            if g.is_none() {
                let sender = tokio::time::timeout(connect_timeout, dial_uds(path))
                    .await
                    .map_err(|_| {
                        Error::Ipc(format!(
                            "UDS connect: dial timed out after {}ms",
                            connect_timeout.as_millis()
                        ))
                    })??;
                *g = Some(sender);
            }
            g
        }
        None => acquire_slot(path, &pool.slots[idx], request_timeout, connect_timeout).await?,
    };

    match attempt_send(&mut slot, route, body_bytes.clone(), request_timeout).await {
        Ok(v) => Ok(v),
        Err(AttemptError::Other(e)) => Err(e),
        Err(AttemptError::ConnectionBroken(_)) => {
            *slot = None;
            *slot = Some(
                tokio::time::timeout(connect_timeout, dial_uds(path))
                    .await
                    .unwrap_or_else(|_| {
                        Err(Error::Ipc(format!(
                            "UDS connect: redial timed out after {}ms",
                            connect_timeout.as_millis()
                        )))
                    })?,
            );
            match attempt_send(&mut slot, route, body_bytes, request_timeout).await {
                Ok(v) => Ok(v),
                Err(AttemptError::Other(e)) => Err(e),
                Err(AttemptError::ConnectionBroken(e)) => {
                    *slot = None;
                    Err(e)
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Reads one HTTP/1.1 request (headers + content-length body); returns `(request_line, body)`.
    async fn read_request<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Option<(String, Vec<u8>)> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        let header_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            let n = s.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let len = head
            .lines()
            .find_map(|l| {
                let l = l.to_ascii_lowercase();
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = buf[header_end + 4..].to_vec();
        while body.len() < len {
            let n = s.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        Some((head.lines().next().unwrap_or("").to_string(), body))
    }

    fn ok_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn sock_path(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("lcg-sc-{}-{tag}.sock", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p.to_string_lossy().to_string()
    }

    #[test]
    fn normalize_base_strips_route() {
        assert_eq!(normalize_base("http://h:1/v1/embeddings"), "http://h:1");
        assert_eq!(normalize_base("http://h:1/v1/"), "http://h:1");
        assert_eq!(normalize_base("http://h:1/"), "http://h:1");
        assert_eq!(normalize_base("http://h:1"), "http://h:1");
    }

    /// Serves keep-alive connections, counting accepts and requests.
    async fn spawn_server(
        path: &str,
        accepts: Arc<AtomicUsize>,
        requests: Arc<AtomicUsize>,
        close_after_each: bool,
    ) {
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                accepts.fetch_add(1, Ordering::SeqCst);
                let requests = Arc::clone(&requests);
                tokio::spawn(async move {
                    while let Some((line, body)) = read_request(&mut s).await {
                        requests.fetch_add(1, Ordering::SeqCst);
                        let echo = serde_json::json!({
                            "line": line,
                            "body": serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
                        });
                        if s.write_all(ok_response(&echo.to_string()).as_bytes())
                            .await
                            .is_err()
                        {
                            return;
                        }
                        if close_after_each {
                            return;
                        }
                    }
                });
            }
        });
    }

    #[tokio::test]
    async fn uds_posts_json_to_route_and_reuses_connection() {
        let path = sock_path("reuse");
        let accepts = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        spawn_server(&path, accepts.clone(), requests.clone(), false).await;
        let c =
            SidecarClient::new_uds(path.clone(), Duration::from_secs(5), Duration::from_secs(2));
        // Four calls round-robin across four slots; a second lap reuses the held connections.
        for i in 0..8 {
            let v = c
                .post_json("/v1/rerank", &serde_json::json!({"n": i}))
                .await
                .unwrap();
            assert_eq!(v["line"], "POST /v1/rerank HTTP/1.1");
            assert_eq!(v["body"]["n"], i);
        }
        assert_eq!(requests.load(Ordering::SeqCst), 8);
        assert_eq!(accepts.load(Ordering::SeqCst), UDS_POOL_SIZE);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn uds_broken_connection_is_retried_once() {
        let path = sock_path("broken");
        let accepts = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(AtomicUsize::new(0));
        // The server closes each connection after one response, so every pooled reuse hits a
        // dead connection and must be re-dialled.
        spawn_server(&path, accepts.clone(), requests.clone(), true).await;
        let c =
            SidecarClient::new_uds(path.clone(), Duration::from_secs(5), Duration::from_secs(2));
        for i in 0..2 * UDS_POOL_SIZE {
            let v = c
                .post_json("/v1/rerank", &serde_json::json!({"n": i}))
                .await
                .unwrap();
            assert_eq!(v["body"]["n"], i);
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2 * UDS_POOL_SIZE);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn uds_timeout_is_not_retried() {
        let path = sock_path("timeout");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let r2 = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let r3 = Arc::clone(&r2);
                tokio::spawn(async move {
                    // Read the request, then never answer.
                    if read_request(&mut s).await.is_some() {
                        r3.fetch_add(1, Ordering::SeqCst);
                    }
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });
        let c = SidecarClient::new_uds(
            path.clone(),
            Duration::from_millis(150),
            Duration::from_secs(2),
        );
        let err = c
            .post_json("/v1/rerank", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            requests.load(Ordering::SeqCst),
            1,
            "a timeout must not retry"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// More concurrent calls than pool slots, each taking longer than `connect_timeout`: the extra
    /// call must queue behind a healthy in-flight one (bounded by the request timeout) rather than
    /// fail with a slot-acquisition timeout.
    #[tokio::test]
    async fn uds_queued_call_waits_for_a_busy_slot_beyond_the_connect_timeout() {
        let path = sock_path("queue");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    while read_request(&mut s).await.is_some() {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let _ = s.write_all(ok_response("{}").as_bytes()).await;
                    }
                });
            }
        });
        let c = Arc::new(SidecarClient::new_uds(
            path.clone(),
            Duration::from_secs(5),
            Duration::from_millis(50),
        ));
        let calls = (0..UDS_POOL_SIZE + 1).map(|_| {
            let c = Arc::clone(&c);
            tokio::spawn(async move { c.post_json("/v1/rerank", &serde_json::json!({})).await })
        });
        for h in calls.collect::<Vec<_>>() {
            h.await
                .unwrap()
                .expect("a queued call must not time out on the slot wait");
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A call must use an idle slot rather than queue behind a slow request on its round-robin slot.
    #[tokio::test]
    async fn uds_call_prefers_an_idle_slot_over_waiting_behind_a_slow_one() {
        let path = sock_path("idle");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let first = Arc::new(AtomicUsize::new(0));
        let f2 = Arc::clone(&first);
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let f3 = Arc::clone(&f2);
                tokio::spawn(async move {
                    while read_request(&mut s).await.is_some() {
                        // The very first request is slow; every other one is instant.
                        if f3.fetch_add(1, Ordering::SeqCst) == 0 {
                            tokio::time::sleep(Duration::from_millis(800)).await;
                        }
                        let _ = s.write_all(ok_response("{}").as_bytes()).await;
                    }
                });
            }
        });
        let c = Arc::new(SidecarClient::new_uds(
            path.clone(),
            Duration::from_secs(5),
            Duration::from_secs(2),
        ));
        let slow = {
            let c = Arc::clone(&c);
            tokio::spawn(async move { c.post_json("/v1/rerank", &serde_json::json!({})).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Four more calls round-robin over all four slots, including the busy one.
        let t0 = std::time::Instant::now();
        for _ in 0..UDS_POOL_SIZE {
            c.post_json("/v1/rerank", &serde_json::json!({}))
                .await
                .unwrap();
        }
        assert!(
            t0.elapsed() < Duration::from_millis(600),
            "calls queued behind the slow slot: {:?}",
            t0.elapsed()
        );
        slow.await.unwrap().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn uds_non_success_status_is_an_error() {
        let path = sock_path("status");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    while read_request(&mut s).await.is_some() {
                        let _ = s
                            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                            .await;
                    }
                });
            }
        });
        let c =
            SidecarClient::new_uds(path.clone(), Duration::from_secs(2), Duration::from_secs(2));
        let err = c
            .post_json("/v1/rerank", &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn uds_unreachable_socket_is_an_error() {
        let c = SidecarClient::new_uds(
            "/nonexistent/lcg-sidecar.sock",
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        assert!(c
            .post_json("/v1/rerank", &serde_json::json!({}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn http_posts_json_to_route() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Some((line, body)) = read_request(&mut s).await {
                        let echo = serde_json::json!({
                            "line": line,
                            "body": serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
                        });
                        let _ = s.write_all(ok_response(&echo.to_string()).as_bytes()).await;
                    }
                });
            }
        });
        let c = SidecarClient::new_http(
            &format!("http://{addr}/v1/embeddings"),
            None,
            Duration::from_secs(5),
            Duration::from_secs(2),
        )
        .unwrap();
        let v = c
            .post_json("/v1/rerank", &serde_json::json!({"q": "x"}))
            .await
            .unwrap();
        assert_eq!(v["line"], "POST /v1/rerank HTTP/1.1");
        assert_eq!(v["body"]["q"], "x");
    }
}
