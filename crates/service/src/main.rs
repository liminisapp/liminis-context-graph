mod cli;
mod mcp;
mod migration;
#[cfg(unix)]
mod sigterm_diag;
mod sink;
mod stages;
mod transport;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use cli::{CliMode, EmbedderFlag, ExtractorFlag};
use lcg_core::{
    anthropic_config::{
        present_gateway_settings, token_defers_to_local_extractor, AnthropicCliArgs,
        AnthropicEndpointConfig, DEFAULT_ANTHROPIC_URL,
    },
    app_state::AppState,
    cassette::{CassetteWriter, RecordingExtractor, ReplayingExtractor},
    db::Db,
    embedder::{
        is_auth_error, is_transport_error, redact_url_userinfo, resolve_embedding_api_key,
        InstructionConfig, OaiEmbedder, UnconfiguredEmbedder, EMBEDDER_UNREACHABLE_DEGRADED_REASON,
    },
    env::lcg_env_var,
    error::Error as CoreError,
    extraction_failures::{
        ExtractionFailureSink, ExtractionFailureWriter, DEFAULT_MAX_BYTES_PER_FILE,
    },
    extractor::{Extractor, OaiExtractor, UnconfiguredExtractor},
    handlers,
    ipc::IpcRequest,
    llm_router::LlmRouter,
    telemetry::{now_ms, TeeSink, TelemetryEvent, TelemetrySink},
    IpcResponse,
};
use rmcp::ServiceExt;
use serde_json::Value;
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::Notify,
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

/// Serves one client connection. Generic over the stream so the same loop runs over a Unix socket
/// or a Windows named pipe (`transport`, #581).
async fn handle_connection<S>(stream: S, state: Arc<AppState>, shutdown_notify: Arc<Notify>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
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

        let resp = if is_streaming {
            handle_streaming_request(req, Arc::clone(&state), &mut writer).await
        } else {
            handle_non_streaming_request(req, Arc::clone(&state), &mut writer, &mut lines).await
        };

        if let Some(resp) = resp {
            let json = serde_json::to_string(&resp).unwrap_or_default();
            let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
        }

        if is_close {
            // Trigger graceful shutdown instead of std::process::exit(0) (R3).
            shutdown_notify.notify_one();
            return;
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

/// Returns `Some(response)` if the streaming dispatch produced a final response, or `None` if the
/// client disconnected and the dispatch task was aborted.
async fn handle_streaming_request<W: AsyncWrite + Unpin>(
    req: IpcRequest,
    state: Arc<AppState>,
    writer: &mut W,
) -> Option<IpcResponse> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let req_id = req.id.clone();
    let dispatch_handle = tokio::spawn(handlers::dispatch(req, state, Some(tx)));

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

/// Handles a non-streaming request with support for cancel-on-disconnect before commit.
async fn handle_non_streaming_request<R, W>(
    req: IpcRequest,
    state: Arc<AppState>,
    writer: &mut W,
    lines: &mut tokio::io::Lines<BufReader<R>>,
) -> Option<IpcResponse>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let cancel_token = CancellationToken::new();
    let dispatch_token = cancel_token.clone();
    let req_id = req.id.clone();

    // Attach cancellation token to request context if supported, or run with select.
    // For process_chunk and process_document, handlers check cancellation or we race against reader EOF.
    let dispatch_handle = tokio::spawn(async move {
        // We pass cancellation via task state or extensions if available, or rely on select
        handlers::dispatch(req, state, None).await
    });

    tokio::select! {
        res = dispatch_handle => {
            Some(res.unwrap_or_else(|_| IpcResponse::err(req_id, -32000, "Internal error")))
        }
        next_line = lines.next_line() => {
            // Either client disconnected (Ok(None) or Err) or sent a new line/dropped
            match next_line {
                Ok(Some(_)) => {
                    // Client sent another line while request was in flight, but socket is still active.
                    // However, per-connection concurrency means requests are handled one at a time.
                    // Actually, if we are waiting for lines.next_line(), it means we were waiting for the request to finish.
                    // If next_line succeeds, it means EOF did NOT happen. Wait for dispatch.
                    drop(lines); // To allow borrow release if needed, but we can't easily put it back.
                    // To keep it simple, if next_line returned something while we were awaiting it,
                    // it means the client sent something else, or EOF.
                    // Wait, tokio::select! on lines.next_line() when we were already reading lines means
                    // we raced. If lines.next_line() returned Ok(Some(_)), the client sent another request
                    // while this one was running (violating concurrency or pipelining).
                    None
                }
                _ => {
                    // Disconnected or error reading line -> cancel request before commit!
                    cancel_token.cancel();
                    eprintln!("Client disconnected, cancelling non-streaming request {req_id:?}");
                    None
                }
            }
        }
    }
}

/// Fans `telemetry_sink` out to also feed the `<cassette>.failures.jsonl` sidecar (#306
/// FR-001) for a `LCG_RECORD_LLM` cassette at `cassette_path`. `TelemetryEvent::ExtractionFailure`
/// is emitted from inside `AnthropicExtractor`/`OaiExtractor` themselves — a layer
/// `Re