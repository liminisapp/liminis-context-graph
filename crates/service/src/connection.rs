//! Per-connection request loop (issue #861, ADR-0861).
//!
//! One task per connection, strictly one request at a time. For the two *cooperative* ingest
//! methods ([`COOPERATIVE_CANCEL_METHODS`]) the loop additionally watches the socket while the
//! request runs, so a client that disconnects (a timeout followed by a retry on a new connection)
//! cancels the abandoned request at its next pre-commit cancellation point instead of leaving the
//! service to finish — and bill for — work nobody is waiting for.
//!
//! The loop is generic over the stream and over a dispatch closure so it can be driven in tests
//! with `tokio::io::duplex` and a fake dispatcher (the service crate has no lib target).

use std::collections::VecDeque;
use std::future::Future;

use lcg_core::{ipc::IpcRequest, IpcResponse};
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc::UnboundedReceiver, mpsc::UnboundedSender, Notify},
};
use tokio_util::sync::CancellationToken;

/// Methods whose in-flight request is cancelled when the client disconnects. Everything else
/// keeps the pre-#861 behaviour: run to completion with no socket watching.
pub const COOPERATIVE_CANCEL_METHODS: &[&str] =
    &["knowledge_process_chunk", "knowledge_process_document"];

/// Lines read off the socket while a cooperative request runs (pipelined requests). Once this
/// many are buffered the reader is no longer polled, which also pauses disconnect detection;
/// the cap bounds memory, and a well-behaved client sends one request at a time.
const MAX_PENDING_LINES: usize = 8;

/// How often a streaming cooperative request writes a `{"type":"progress","phase":"heartbeat"}`
/// frame while it is otherwise silent (e.g. the long LLM call between the `extract` and `commit`
/// frames). Clients that apply a per-line idle read timeout (the attached MCP backend, 30 s by
/// default) would otherwise drop the connection mid-extraction, which now cancels the ingest.
/// Only sent when the caller asked for progress; token-less requests never see a frame.
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Upper bound on one progress/heartbeat frame write while a cooperative request runs. The
/// dispatch future is polled inline, so a client that stops reading its socket would otherwise
/// stall the request (even a commit holding the group write lock) once the socket buffer fills.
/// A write that cannot finish in this time is treated as a disconnect.
const FRAME_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Writes one frame, returning `false` if the write failed or did not finish within `limit`
/// (both mean the client is gone or not reading). A timed-out write may leave a partial line on
/// the socket, so the caller must not write to it again.
async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    limit: std::time::Duration,
) -> bool {
    matches!(
        tokio::time::timeout(limit, writer.write_all(bytes)).await,
        Ok(Ok(()))
    )
}

/// Answers "has the peer fully closed its end (it can no longer receive)?" without consuming
/// any input or writing any byte. Consulted only after read EOF: EOF alone cannot tell a client
/// that hung up from one that merely half-closed its write side (`shutdown(SHUT_WR)`, `socat`,
/// `nc -N`) and is still waiting for the response.
pub type PeerGone = std::sync::Arc<dyn Fn() -> bool + Send + Sync>;

/// A connection stream that can report whether its peer has fully closed.
pub trait PeerProbe {
    /// Built before the stream is split; the returned probe is only used while the connection
    /// task still owns the stream, so a raw descriptor captured here stays valid.
    fn peer_gone_probe(&self) -> PeerGone;
}

/// How often a half-closed peer is re-checked for a later full close while a request runs.
const HALF_CLOSE_RECHECK: std::time::Duration = std::time::Duration::from_millis(100);

/// Unix sockets: a peer that closed fully raises `POLLHUP` on our end (Linux: both directions
/// shut down; macOS: `SS_CANTRCVMORE` and `SS_CANTSENDMORE`); a peer that only shut its write
/// side leaves us writable with no hang-up. `POLLOUT` is requested because macOS reports nothing
/// for an empty event mask.
#[cfg(unix)]
fn fd_fully_closed(fd: std::os::fd::RawFd) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLOUT,
        revents: 0,
    };
    // SAFETY: `pfd` is a valid, initialised pollfd and the count is 1.
    let ready = unsafe { libc::poll(&mut pfd, 1, 0) };
    ready > 0 && pfd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
}

#[cfg(unix)]
impl PeerProbe for tokio::net::UnixStream {
    fn peer_gone_probe(&self) -> PeerGone {
        use std::os::fd::AsRawFd;
        let fd = self.as_raw_fd();
        std::sync::Arc::new(move || fd_fully_closed(fd))
    }
}

/// Windows named pipes have no half-close: read EOF means the client closed.
#[cfg(windows)]
impl PeerProbe for tokio::net::windows::named_pipe::NamedPipeServer {
    fn peer_gone_probe(&self) -> PeerGone {
        std::sync::Arc::new(|| true)
    }
}

/// How a cooperative request ended.
#[allow(clippy::large_enum_variant)]
enum Served {
    /// Ran to completion; write the response (and any frames still queued) to the client.
    /// `reader_failed` means a pipelined line was unreadable (not valid UTF-8): the request is
    /// still answered, then the connection closes, exactly as before #861.
    Done {
        resp: IpcResponse,
        reader_failed: bool,
    },
    /// The client went away. The request has already been cancelled and awaited; write nothing.
    Disconnected,
}

/// Serves one client connection. `parent` is the process-wide shutdown token: each cooperative
/// request gets a child of it, so shutdown still cascades while a disconnect cancels one request.
pub async fn handle_connection_with<S, F, Fut>(
    stream: S,
    parent: CancellationToken,
    dispatch: F,
    shutdown_notify: std::sync::Arc<Notify>,
) where
    S: AsyncRead + AsyncWrite + PeerProbe + Send + 'static,
    F: Fn(IpcRequest, CancellationToken, Option<UnboundedSender<Value>>) -> Fut,
    Fut: Future<Output = IpcResponse> + Send + 'static,
{
    let peer_gone = stream.peer_gone_probe();
    serve_connection(
        stream,
        parent,
        dispatch,
        shutdown_notify,
        HEARTBEAT_INTERVAL,
        FRAME_WRITE_TIMEOUT,
        peer_gone,
    )
    .await;
}

/// [`handle_connection_with`] with an explicit heartbeat period (tests use a short one).
async fn serve_connection<S, F, Fut>(
    stream: S,
    parent: CancellationToken,
    dispatch: F,
    shutdown_notify: std::sync::Arc<Notify>,
    heartbeat: std::time::Duration,
    write_timeout: std::time::Duration,
    peer_gone: PeerGone,
) where
    S: AsyncRead + AsyncWrite + Send + 'static,
    F: Fn(IpcRequest, CancellationToken, Option<UnboundedSender<Value>>) -> Fut,
    Fut: Future<Output = IpcResponse> + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();
    let mut pending: VecDeque<String> = VecDeque::new();

    loop {
        let line = match pending.pop_front() {
            Some(l) => l,
            None => match lines.next_line().await {
                Ok(Some(l)) => l,
                _ => return,
            },
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let req: IpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_parse_error(&mut writer, e).await;
                continue;
            }
        };

        let is_close = req.method == "knowledge_close";
        let is_streaming = req
            .params
            .get("_progress_token")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        let cooperative = COOPERATIVE_CANCEL_METHODS.contains(&req.method.as_str());

        let mut close_after = false;
        let resp = if cooperative {
            let method = req.method.clone();
            let cancel = parent.child_token();
            let (tx, rx) = if is_streaming {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
                (Some(tx), Some(rx))
            } else {
                (None, None)
            };
            let fut = CatchPanic {
                req_id: req.id.clone(),
                fut: Box::pin(dispatch(req, cancel.clone(), tx)),
            };
            match serve_cooperative(
                &method,
                fut,
                &cancel,
                rx,
                &mut lines,
                &mut pending,
                &mut writer,
                heartbeat,
                write_timeout,
                &peer_gone,
            )
            .await
            {
                Served::Done {
                    resp,
                    reader_failed,
                } => {
                    close_after = reader_failed;
                    Some(resp)
                }
                // Queued pipelined requests never start: the client is gone.
                Served::Disconnected => return,
            }
        } else if is_streaming {
            handle_streaming_request(req, &parent, &dispatch, &mut writer).await
        } else {
            Some(dispatch(req, parent.clone(), None).await)
        };

        if let Some(resp) = resp {
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
        }

        if close_after {
            return;
        }

        if is_close {
            // Trigger graceful shutdown instead of std::process::exit(0) (R3).
            shutdown_notify.notify_one();
            return;
        }
    }
}

/// Polls a dispatch future inline but turns a panic into the same `Internal error` response the
/// spawned streaming path produced (a `JoinError`), so a panicking request answers the client
/// instead of killing the connection task. Never polled again after it returns `Ready`.
struct CatchPanic<Fut> {
    req_id: Value,
    fut: std::pin::Pin<Box<Fut>>,
}

impl<Fut: Future<Output = IpcResponse>> Future for CatchPanic<Fut> {
    type Output = IpcResponse;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<IpcResponse> {
        use std::task::Poll;
        let this = &mut *self;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.fut.as_mut().poll(cx)))
        {
            Ok(poll) => poll,
            Err(_) => {
                eprintln!(
                    "liminis-context-graph: request handler panicked; returning Internal error"
                );
                Poll::Ready(IpcResponse::err(
                    this.req_id.clone(),
                    -32000,
                    "Internal error",
                ))
            }
        }
    }
}

/// Runs a cooperative request while watching the socket.
///
/// The dispatch future is polled inline (never spawned), so aborting the connection task at
/// shutdown still drops it exactly as before. A disconnect cancels `cancel` and then keeps
/// awaiting the future to completion: if Phase C has already started it must finish (never a
/// torn write), and if it hasn't, the next pre-commit cancellation point ends it.
///
/// A disconnect is a socket read error, a failed or stalled progress write, or read EOF *with the
/// peer fully closed* (`peer_gone`). A client that only half-closes its write side after sending
/// its request (`socat`, `nc -N`, `shutdown(SHUT_WR)`) can still receive, so its request keeps
/// running and is answered; if that peer later closes fully, a periodic re-check cancels the
/// request then. An unreadable, non-UTF-8 pipelined line is not a disconnect either: the request
/// still completes.
#[allow(clippy::too_many_arguments)]
async fn serve_cooperative<Fut, R, W>(
    method: &str,
    fut: Fut,
    cancel: &CancellationToken,
    mut rx: Option<UnboundedReceiver<Value>>,
    lines: &mut tokio::io::Lines<BufReader<R>>,
    pending: &mut VecDeque<String>,
    writer: &mut W,
    heartbeat: std::time::Duration,
    write_timeout: std::time::Duration,
    peer_gone: &PeerGone,
) -> Served
where
    Fut: Future<Output = IpcResponse>,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    tokio::pin!(fut);
    let mut disconnected = false;
    let mut reader_done = false;
    let mut reader_failed = false;
    let mut half_closed = false;
    let mut recheck = tokio::time::interval(HALF_CLOSE_RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Keep-alive for streaming callers only (see `HEARTBEAT_INTERVAL`).
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let resp = loop {
        tokio::select! {
            resp = &mut fut => break Some(resp),
            frame = async {
                match rx.as_mut() {
                    Some(r) => r.recv().await,
                    None => std::future::pending().await,
                }
            } => match frame {
                Some(val) => {
                    let json = serde_json::to_string(&val).unwrap_or_default();
                    if !write_frame(writer, format!("{json}\n").as_bytes(), write_timeout).await {
                        disconnected = true;
                    }
                }
                // Every sender dropped: no more frames, the future is about to finish.
                None => rx = None,
            },
            _ = beat.tick(), if rx.is_some() => {
                let frame = "{\"type\":\"progress\",\"phase\":\"heartbeat\"}\n";
                if !write_frame(writer, frame.as_bytes(), write_timeout).await {
                    disconnected = true;
                }
            }
            // `next_line` is cancel-safe: a line that arrives while the request runs is a
            // pipelined request and is kept for the loop to serve afterwards.
            line = lines.next_line(), if !reader_done && pending.len() < MAX_PENDING_LINES => {
                match line {
                    Ok(Some(l)) => pending.push_back(l),
                    // A socket error is a disconnect, and so is EOF once the peer has fully
                    // closed; EOF from a peer that merely half-closed is not (it is still
                    // waiting for the response). A non-UTF-8 pipelined line is a malformed
                    // *request*, not a hang-up: finish the in-flight request, answer it, then
                    // close (the pre-#861 behaviour for an unreadable line).
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                        reader_done = true;
                        reader_failed = true;
                    }
                    Ok(None) => {
                        reader_done = true;
                        if peer_gone() {
                            disconnected = true;
                        } else {
                            half_closed = true;
                        }
                    }
                    Err(_) => {
                        reader_done = true;
                        disconnected = true;
                    }
                }
            }
            // The peer half-closed earlier; notice if it later closes fully.
            _ = recheck.tick(), if half_closed => {
                if peer_gone() {
                    disconnected = true;
                }
            }
        }
        if disconnected {
            break None;
        }
    };

    match resp {
        Some(resp) => {
            // Frames queued between the last poll and completion go out before the response.
            if let Some(r) = rx.as_mut() {
                while let Ok(val) = r.try_recv() {
                    let json = serde_json::to_string(&val).unwrap_or_default();
                    if !write_frame(writer, format!("{json}\n").as_bytes(), write_timeout).await {
                        // The client stopped reading (or is gone). A timed-out write may have
                        // left half a line on the socket, so nothing more may be written to it:
                        // the request already ran to completion, so just end the connection.
                        eprintln!(
                            "liminis-context-graph: frame write failed after {method} finished; \
                             closing connection without a response"
                        );
                        return Served::Disconnected;
                    }
                }
            }
            Served::Done {
                resp,
                reader_failed,
            }
        }
        None => {
            eprintln!(
                "liminis-context-graph: client disconnected; cancelling in-flight {method} \
                 (cause=client_disconnect)"
            );
            cancel.cancel();
            drop(rx);
            // Finish whatever has already started (an in-progress commit completes); the
            // response is discarded and nothing is written to the dead socket.
            let _ = fut.await;
            Served::Disconnected
        }
    }
}

async fn write_parse_error<W: AsyncWrite + Unpin>(writer: &mut W, e: serde_json::Error) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {"code": -32700, "message": format!("Parse error: {e}")}
    });
    let json = serde_json::to_string(&response).unwrap_or_default();
    let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
}

/// Streaming dispatch for the non-cooperative methods (rebuild, reprocess, ...): returns
/// `Some(response)` if the dispatch produced a final response, or `None` if the client
/// disconnected and the dispatch task was aborted. Unchanged from before #861.
async fn handle_streaming_request<W, F, Fut>(
    req: IpcRequest,
    parent: &CancellationToken,
    dispatch: &F,
    writer: &mut W,
) -> Option<IpcResponse>
where
    W: AsyncWrite + Unpin,
    F: Fn(IpcRequest, CancellationToken, Option<UnboundedSender<Value>>) -> Fut,
    Fut: Future<Output = IpcResponse> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let req_id = req.id.clone();
    let dispatch_handle = tokio::spawn(dispatch(req, parent.clone(), Some(tx)));

    let mut client_ok = true;
    while let Some(val) = rx.recv().await {
        let json = serde_json::to_string(&val).unwrap_or_default();
        if writer
            .write_all(format!("{json}\n").as_bytes())
            .await
            .is_err()
        {
            client_ok = false;
            break;
        }
    }

    if client_ok {
        Some(
            dispatch_handle
                .await
                .unwrap_or_else(|_| IpcResponse::err(req_id, -32000, "Internal error")),
        )
    } else {
        drop(rx);
        dispatch_handle.abort();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, DuplexStream};

    /// Duplex streams cannot tell a half-close from a full close, so over them read EOF always
    /// means gone (the pre-half-close behaviour). Real-socket tests below use the real probe.
    fn always_gone() -> PeerGone {
        Arc::new(|| true)
    }

    impl PeerProbe for DuplexStream {
        fn peer_gone_probe(&self) -> PeerGone {
            always_gone()
        }
    }

    /// What the fake dispatcher observed for one request.
    #[derive(Default)]
    struct Probe {
        started: AtomicUsize,
        finished: AtomicUsize,
        saw_cancel: AtomicUsize,
    }

    /// Fake dispatcher: `work_ms` of "extraction" that a cancel cuts short, then a `commit`
    /// stage of `commit_ms` that ignores cancellation (as Phase C does). Echoes the method.
    type BoxedFut = std::pin::Pin<Box<dyn Future<Output = IpcResponse> + Send>>;

    fn fake(
        probe: Arc<Probe>,
        work_ms: u64,
        commit_ms: u64,
    ) -> impl Fn(IpcRequest, CancellationToken, Option<UnboundedSender<Value>>) -> BoxedFut {
        move |req, cancel, tx| {
            let probe = Arc::clone(&probe);
            Box::pin(async move {
                probe.started.fetch_add(1, Ordering::SeqCst);
                if let Some(tx) = &tx {
                    let _ = tx.send(serde_json::json!({"type":"progress","phase":"extract"}));
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(work_ms)) => {}
                    _ = cancel.cancelled() => {
                        probe.saw_cancel.fetch_add(1, Ordering::SeqCst);
                        return IpcResponse::err(req.id, -32000, "cancelled");
                    }
                }
                if let Some(tx) = &tx {
                    let _ = tx.send(serde_json::json!({"type":"progress","phase":"commit"}));
                }
                tokio::time::sleep(Duration::from_millis(commit_ms)).await;
                probe.finished.fetch_add(1, Ordering::SeqCst);
                IpcResponse::ok(req.id, serde_json::json!({"method": req.method}))
            })
        }
    }

    fn spawn_conn(
        server: DuplexStream,
        parent: CancellationToken,
        probe: Arc<Probe>,
        work_ms: u64,
        commit_ms: u64,
    ) -> (tokio::task::JoinHandle<()>, Arc<Notify>) {
        let notify = Arc::new(Notify::new());
        let n = Arc::clone(&notify);
        let h = tokio::spawn(async move {
            handle_connection_with(server, parent, fake(probe, work_ms, commit_ms), n).await;
        });
        (h, notify)
    }

    fn line(id: u64, method: &str, token: bool) -> String {
        let params = if token {
            serde_json::json!({"_progress_token": "t"})
        } else {
            serde_json::json!({})
        };
        format!(
            "{}\n",
            serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        )
    }

    async fn read_lines(client: &mut DuplexStream, n: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while out.len() < n {
            let got = tokio::time::timeout(Duration::from_secs(5), client.read(&mut byte))
                .await
                .expect("timed out waiting for server output")
                .unwrap();
            if got == 0 {
                break;
            }
            if byte[0] == b'\n' {
                out.push(serde_json::from_slice(&buf).unwrap());
                buf.clear();
            } else {
                buf.push(byte[0]);
            }
        }
        out
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    #[tokio::test]
    async fn streaming_cooperative_request_sends_heartbeats_while_silent() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let notify = Arc::new(Notify::new());
        let h = tokio::spawn(serve_connection(
            server,
            CancellationToken::new(),
            // 600 ms of silent work, no commit stage.
            fake(Arc::clone(&probe), 600, 0),
            notify,
            Duration::from_millis(100),
            FRAME_WRITE_TIMEOUT,
            always_gone(),
        ));
        client
            .write_all(line(1, "knowledge_process_chunk", true).as_bytes())
            .await
            .unwrap();
        let out = read_lines(&mut client, 5).await;
        let phases: Vec<&str> = out.iter().filter_map(|v| v["phase"].as_str()).collect();
        assert_eq!(phases[0], "extract");
        assert!(
            phases.iter().filter(|p| **p == "heartbeat").count() >= 3,
            "silence must be broken by heartbeats: {phases:?}"
        );
        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
    }

    // ---- Half-close vs full close over a real socket (the real `PeerProbe`). These run on
    // macOS and Linux CI alike: hang-up reporting is platform-specific, so they pin it.

    #[cfg(unix)]
    async fn real_conn(
        probe: Arc<Probe>,
        work_ms: u64,
        commit_ms: u64,
    ) -> (tokio::net::UnixStream, tokio::task::JoinHandle<()>) {
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let notify = Arc::new(Notify::new());
        let h = tokio::spawn(async move {
            handle_connection_with(
                server,
                CancellationToken::new(),
                fake(probe, work_ms, commit_ms),
                notify,
            )
            .await;
        });
        (client, h)
    }

    #[cfg(unix)]
    async fn read_json_lines(client: &mut tokio::net::UnixStream, n: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        while out.len() < n {
            let got = tokio::time::timeout(Duration::from_secs(5), client.read(&mut byte))
                .await
                .expect("timed out waiting for server output")
                .unwrap();
            if got == 0 {
                break;
            }
            if byte[0] == b'\n' {
                out.push(serde_json::from_slice(&buf).unwrap());
                buf.clear();
            } else {
                buf.push(byte[0]);
            }
        }
        out
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn half_closed_client_still_gets_the_full_response() {
        for token in [false, true] {
            let probe = Arc::new(Probe::default());
            let (mut client, h) = real_conn(Arc::clone(&probe), 300, 100).await;
            client
                .write_all(line(1, "knowledge_process_chunk", token).as_bytes())
                .await
                .unwrap();
            // `socat` / `nc -N` / `shutdown(SHUT_WR)`: done writing, still reading.
            client.shutdown().await.unwrap();
            let out = read_json_lines(&mut client, if token { 3 } else { 1 }).await;
            let last = out.last().expect("a response");
            assert_eq!(last["result"]["method"], "knowledge_process_chunk");
            assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 0, "token={token}");
            assert_eq!(probe.finished.load(Ordering::SeqCst), 1, "token={token}");
            tokio::time::timeout(Duration::from_secs(5), h)
                .await
                .expect("connection closes after answering")
                .unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn half_closed_client_that_then_drops_cancels_the_request() {
        let probe = Arc::new(Probe::default());
        let (mut client, h) = real_conn(Arc::clone(&probe), 10_000, 0).await;
        client
            .write_all(line(1, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        client.shutdown().await.unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // The half-close alone must not cancel anything.
        settle().await;
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 0);
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("a full close after the half-close cancels the request")
            .unwrap();
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 1);
        assert_eq!(probe.finished.load(Ordering::SeqCst), 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fully_closed_client_cancels_over_a_real_socket() {
        let probe = Arc::new(Probe::default());
        let (mut client, h) = real_conn(Arc::clone(&probe), 10_000, 0).await;
        client
            .write_all(line(1, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("a full close cancels the request")
            .unwrap();
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn client_that_stops_reading_is_treated_as_disconnected() {
        // A 16-byte socket buffer fills on the first progress frame and the client never reads.
        let (mut client, server) = tokio::io::duplex(16);
        let probe = Arc::new(Probe::default());
        let notify = Arc::new(Notify::new());
        let h = tokio::spawn(serve_connection(
            server,
            CancellationToken::new(),
            fake(Arc::clone(&probe), 10_000, 0),
            notify,
            Duration::from_secs(60),
            Duration::from_millis(100),
            always_gone(),
        ));
        client
            .write_all(line(1, "knowledge_process_chunk", true).as_bytes())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("a stalled frame write must not hang the connection")
            .unwrap();
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 1);
        assert_eq!(probe.finished.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_drain_write_closes_without_appending_response_to_partial_line() {
        // The request finishes instantly with several frames queued; a 16-byte buffer and a
        // client that never reads make a frame write stall. The connection must end instead of
        // blocking on the response write or gluing the response onto a partial frame line.
        let (mut client, server) = tokio::io::duplex(16);
        let notify = Arc::new(Notify::new());
        let h = tokio::spawn(serve_connection(
            server,
            CancellationToken::new(),
            |req: IpcRequest, _cancel: CancellationToken, tx: Option<UnboundedSender<Value>>| {
                Box::pin(async move {
                    if let Some(tx) = &tx {
                        for _ in 0..4 {
                            let _ = tx.send(
                                serde_json::json!({"type":"progress","phase":"extract","pad":"xxxxxxxxxxxxxxxx"}),
                            );
                        }
                    }
                    IpcResponse::ok(req.id, serde_json::json!({"done": true}))
                }) as BoxedFut
            },
            notify,
            Duration::from_secs(60),
            Duration::from_millis(100),
            always_gone(),
        ));
        client
            .write_all(line(1, "knowledge_process_chunk", true).as_bytes())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("a stalled drain write must not hang the connection")
            .unwrap();
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest)).await;
        assert!(
            !String::from_utf8_lossy(&rest).contains("\"result\""),
            "no response may follow a partial frame"
        );
    }

    #[tokio::test]
    async fn non_utf8_pipelined_line_does_not_cancel_the_inflight_request() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (h, _n) = spawn_conn(server, CancellationToken::new(), Arc::clone(&probe), 200, 0);
        client
            .write_all(line(1, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Not valid UTF-8, while the client is still connected and waiting.
        client.write_all(&[0xff, 0xfe, b'\n']).await.unwrap();
        let out = read_lines(&mut client, 1).await;
        assert_eq!(out[0]["result"]["method"], "knowledge_process_chunk");
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 0, "not cancelled");
        assert_eq!(probe.finished.load(Ordering::SeqCst), 1);
        // Then the connection closes, as it did before #861.
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("connection closes after answering")
            .unwrap();
    }

    #[tokio::test]
    async fn token_less_cooperative_request_gets_no_heartbeats() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let notify = Arc::new(Notify::new());
        let h = tokio::spawn(serve_connection(
            server,
            CancellationToken::new(),
            fake(Arc::clone(&probe), 400, 0),
            notify,
            Duration::from_millis(50),
            FRAME_WRITE_TIMEOUT,
            always_gone(),
        ));
        client
            .write_all(line(1, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        // The first and only line is the terminal response.
        let out = read_lines(&mut client, 1).await;
        assert_eq!(out[0]["result"]["method"], "knowledge_process_chunk");
        drop(client);
        let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
    }

    #[tokio::test]
    async fn panicking_cooperative_request_answers_internal_error() {
        let (mut client, server) = tokio::io::duplex(4096);
        let notify = Arc::new(Notify::new());
        let h = tokio::spawn(handle_connection_with(
            server,
            CancellationToken::new(),
            |_req, _cancel, _tx| async { panic!("boom") },
            notify,
        ));
        client
            .write_all(line(7, "knowledge_process_document", true).as_bytes())
            .await
            .unwrap();
        let out = read_lines(&mut client, 1).await;
        assert_eq!(out[0]["error"]["message"], "Internal error");
        assert_eq!(out[0]["id"], 7);
        // The connection survives and serves the next request.
        client
            .write_all(line(8, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        let out = read_lines(&mut client, 1).await;
        assert_eq!(out[0]["error"]["message"], "Internal error");
        assert_eq!(out[0]["id"], 8);
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn disconnect_cancels_cooperative_request_and_writes_nothing() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (h, _n) = spawn_conn(
            server,
            CancellationToken::new(),
            Arc::clone(&probe),
            60_000,
            0,
        );
        client
            .write_all(line(1, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        // Let the request start, then drop the client (EOF + closed pipe).
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .expect("connection task must return after a disconnect")
            .unwrap();
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 1);
        assert_eq!(probe.finished.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disconnect_during_commit_lets_dispatch_run_to_completion() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        // No extraction time; a 300 ms commit stage that ignores cancellation.
        let (h, _n) = spawn_conn(server, CancellationToken::new(), Arc::clone(&probe), 0, 300);
        client
            .write_all(line(1, "knowledge_process_chunk", false).as_bytes())
            .await
            .unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(60)).await; // now inside the commit stage
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            probe.finished.load(Ordering::SeqCst),
            1,
            "the dispatch future must be awaited to completion, not dropped"
        );
    }

    #[tokio::test]
    async fn queued_pipelined_requests_never_start_after_disconnect() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (h, _n) = spawn_conn(
            server,
            CancellationToken::new(),
            Arc::clone(&probe),
            60_000,
            0,
        );
        let mut both = line(1, "knowledge_process_chunk", false);
        both.push_str(&line(2, "knowledge_process_chunk", false));
        client.write_all(both.as_bytes()).await.unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            probe.started.load(Ordering::SeqCst),
            1,
            "request 2 never starts"
        );
    }

    #[tokio::test]
    async fn progress_frames_precede_the_result() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (_h, _n) = spawn_conn(server, CancellationToken::new(), probe, 20, 20);
        client
            .write_all(line(7, "knowledge_process_chunk", true).as_bytes())
            .await
            .unwrap();
        let out = read_lines(&mut client, 3).await;
        assert_eq!(out[0]["phase"], "extract");
        assert_eq!(out[1]["phase"], "commit");
        assert_eq!(out[2]["id"], 7);
        assert_eq!(out[2]["result"]["method"], "knowledge_process_chunk");
    }

    #[tokio::test]
    async fn pipelined_request_on_open_connection_is_buffered_and_served() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (_h, _n) = spawn_conn(server, CancellationToken::new(), probe, 100, 0);
        let mut both = line(1, "knowledge_process_chunk", false);
        both.push_str(&line(2, "health_check", false));
        client.write_all(both.as_bytes()).await.unwrap();
        let out = read_lines(&mut client, 2).await;
        assert_eq!(out[0]["id"], 1);
        assert_eq!(out[1]["id"], 2, "no pipelined bytes lost");
        assert_eq!(out[1]["result"]["method"], "health_check");
    }

    #[tokio::test]
    async fn disconnect_during_non_cooperative_method_does_not_cancel() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (h, _n) = spawn_conn(server, CancellationToken::new(), Arc::clone(&probe), 200, 0);
        client
            .write_all(line(1, "knowledge_search", false).as_bytes())
            .await
            .unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(client);
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 0);
        assert_eq!(probe.finished.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn parent_shutdown_cancel_reaches_the_request_token() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let parent = CancellationToken::new();
        let (_h, _n) = spawn_conn(server, parent.clone(), Arc::clone(&probe), 60_000, 0);
        client
            .write_all(line(1, "knowledge_process_document", false).as_bytes())
            .await
            .unwrap();
        while probe.started.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        parent.cancel();
        let out = read_lines(&mut client, 1).await;
        assert_eq!(out[0]["error"]["message"], "cancelled");
        assert_eq!(probe.saw_cancel.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn knowledge_close_triggers_shutdown() {
        let (mut client, server) = tokio::io::duplex(4096);
        let probe = Arc::new(Probe::default());
        let (h, notify) = spawn_conn(server, CancellationToken::new(), probe, 0, 0);
        client
            .write_all(line(1, "knowledge_close", false).as_bytes())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), notify.notified())
            .await
            .expect("shutdown notified");
        tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap();
        settle().await;
    }
}
