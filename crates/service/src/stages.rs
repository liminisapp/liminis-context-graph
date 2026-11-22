//! Startup wiring for the optional passage-search stages (issue #796): resolves the reranker and
//! query-expansion endpoints, builds their clients, and installs them on the app state.
//!
//! Resolution is deliberately forgiving about reachability. Both stages are opt-in per call, so an
//! endpoint that cannot be reached never fails startup — a call that opts in degrades and says
//! so. What *is* rejected loudly at startup is a malformed setting (a bad `LCG_RERANK` value,
//! say): `install` returns an error and the service exits rather than run with stages silently
//! unconfigured.
//!
//! Reranker endpoint: `--reranker-uds|http`, else the embedder's own sidecar (same flag, else the
//! default socket, else a *loopback* `LCG_EMBEDDING_URL`) — the sidecar serves `/v1/rerank` next
//! to `/v1/embeddings`. A non-loopback embedding URL is never reused, so note content cannot go to
//! a third-party embedding provider that was not named for rerank or expansion. Expansion endpoint: `--expander-uds|http`, else `LCG_EXPANSION_URL`, else the
//! sidecar's `/v1/chat/completions` (Apple Foundation Models, the model the sidecar hosts).

use std::sync::Arc;
use std::time::Duration;

use lcg_core::{
    env::lcg_env_var,
    query_expand::OaiQueryExpander,
    rerank::SidecarReranker,
    search_stages::{SearchStages, SearchStagesConfig},
    sidecar_client::SidecarClient,
    AppState,
};

use crate::cli::{EmbedderFlag, StageEndpointFlag, StageFlags};

pub const DEFAULT_SIDECAR_UDS: &str = "/tmp/liminis-inference.sock";
pub const DEFAULT_RERANK_MODEL: &str = "mixedbread-ai/mxbai-rerank-large-v1";
pub const DEFAULT_EXPANSION_MODEL: &str = "apple-foundation";
// The recorded Core ML p50 for the default model is ~1.9 s at 50 candidates (p95 not measured), so a
// 3 s bound would turn a modest tail into silent `unavailable` degradation.
const DEFAULT_RERANK_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_EXPANSION_TIMEOUT_MS: u64 = 3_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Where a stage service lives.
#[derive(Debug, Clone, PartialEq)]
pub enum Endpoint {
    Uds(String),
    Http(String),
}

impl From<StageEndpointFlag> for Endpoint {
    fn from(f: StageEndpointFlag) -> Self {
        match f {
            StageEndpointFlag::Uds(p) => Endpoint::Uds(p),
            StageEndpointFlag::Http(u) => Endpoint::Http(u),
        }
    }
}

/// True when `url`'s host is a loopback address (`localhost`, `127.0.0.0/8`, `::1`).
fn is_loopback_http(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The inference sidecar's endpoint as the embedder would resolve it: its flag, else the default
/// socket if present, else `LCG_EMBEDDING_URL`.
///
/// An HTTP embedder URL is only treated as "the sidecar" when it is loopback. The embedding URL
/// may name a third-party provider chosen for embeddings alone; defaulting rerank (passage
/// bodies) or expansion (the query) to it would send note content off the machine to a host the
/// operator never named for that purpose. A non-loopback host needs an explicit
/// `--reranker-http` / `--expander-http` / `LCG_EXPANSION_URL`.
pub fn sidecar_endpoint(
    embedder: Option<&EmbedderFlag>,
    default_socket_exists: bool,
    embedding_url: Option<String>,
) -> Option<Endpoint> {
    let local_http = |u: String| is_loopback_http(&u).then_some(Endpoint::Http(u));
    match embedder {
        Some(EmbedderFlag::Uds(p)) => Some(Endpoint::Uds(p.clone())),
        Some(EmbedderFlag::Http(u)) => local_http(u.clone()),
        // `--embedder test` (#851) has no sidecar, and must never fall through to one found in
        // the environment: the reranker and expander stay unconfigured.
        Some(EmbedderFlag::Test) => None,
        None if default_socket_exists => Some(Endpoint::Uds(DEFAULT_SIDECAR_UDS.to_string())),
        None => embedding_url.and_then(local_http),
    }
}

/// An environment value with surrounding whitespace removed; empty or whitespace-only counts as
/// unset (a variable set to `""` by docker-compose or a `.env` template must not become an endpoint
/// of `""`, or a bearer token of nothing).
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_non_empty(name: &str) -> Option<String> {
    non_empty(std::env::var(name).ok())
}

/// `LCG_EXPANSION_API_KEY` is sent only when the endpoint is the one `LCG_EXPANSION_URL` names; an
/// endpoint chosen by `--expander-*` or defaulted to the sidecar never receives it.
fn expansion_key(api_key: Option<String>, endpoint_from_url: bool) -> Option<String> {
    api_key.filter(|_| endpoint_from_url)
}

/// Reranker endpoint: explicit flag, else the sidecar.
pub fn reranker_endpoint(flags: &StageFlags, sidecar: Option<Endpoint>) -> Option<Endpoint> {
    flags.reranker.clone().map(Endpoint::from).or(sidecar)
}

/// Expansion endpoint: explicit flag, else `LCG_EXPANSION_URL`, else the sidecar.
pub fn expander_endpoint(
    flags: &StageFlags,
    expansion_url: Option<String>,
    sidecar: Option<Endpoint>,
) -> Option<Endpoint> {
    flags
        .expander
        .clone()
        .map(Endpoint::from)
        .or_else(|| expansion_url.map(Endpoint::Http))
        .or(sidecar)
}

fn timeout_from_env(name: &str, default_ms: u64) -> Result<Duration, String> {
    // Empty or whitespace-only counts as unset, like every other stage variable.
    match env_non_empty(name) {
        None => Ok(Duration::from_millis(default_ms)),
        Some(v) => v
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(Duration::from_millis)
            .ok_or_else(|| {
                format!("{name}: expected a positive integer (milliseconds), got {v:?}")
            }),
    }
}

fn build_client(
    ep: &Endpoint,
    api_key: Option<String>,
    request_timeout: Duration,
) -> Result<SidecarClient, String> {
    match ep {
        Endpoint::Http(url) => {
            SidecarClient::new_http(url, api_key, request_timeout, CONNECT_TIMEOUT)
                .map_err(|e| e.to_string())
        }
        #[cfg(unix)]
        Endpoint::Uds(path) => Ok(SidecarClient::new_uds(
            path.clone(),
            request_timeout,
            CONNECT_TIMEOUT,
        )),
        #[cfg(not(unix))]
        Endpoint::Uds(_) => Err("Unix-socket endpoints are only supported on Unix".to_string()),
    }
}

/// The rerank and expansion request timeouts from the environment. Called before any endpoint is
/// resolved so a malformed value fails startup even when no stage ends up configured.
fn stage_timeouts() -> Result<(Duration, Duration), String> {
    Ok((
        timeout_from_env("LCG_RERANK_TIMEOUT_MS", DEFAULT_RERANK_TIMEOUT_MS)?,
        timeout_from_env("LCG_EXPANSION_TIMEOUT_MS", DEFAULT_EXPANSION_TIMEOUT_MS)?,
    ))
}

/// Validates every stage setting that needs neither an `AppState` nor a network call: the
/// `LCG_RERANK*` values and the two timeouts. Called before bootstrap so a typo fails startup
/// immediately instead of after the DB open and the embedder retry window.
pub fn validate_settings() -> Result<(), String> {
    SearchStagesConfig::from_env().map_err(|e| e.to_string())?;
    stage_timeouts()?;
    Ok(())
}

/// Builds the stage handles from flags and environment and installs them on `state`.
pub fn install(
    state: &AppState,
    flags: &StageFlags,
    embedder: Option<&EmbedderFlag>,
) -> Result<(), String> {
    let config = SearchStagesConfig::from_env().map_err(|e| e.to_string())?;
    // Validate both timeouts up front, whether or not an endpoint resolves below: a malformed
    // value must stop startup, not be silently ignored because no stage is configured.
    let (rerank_timeout, expansion_timeout) = stage_timeouts()?;
    let sidecar = sidecar_endpoint(
        embedder,
        std::path::Path::new(DEFAULT_SIDECAR_UDS).exists(),
        lcg_env_var("LCG_EMBEDDING_URL", "GRAPHITI_EMBEDDING_URL").ok(),
    );

    let reranker = match reranker_endpoint(flags, sidecar.clone()) {
        None => {
            eprintln!(
                "liminis-context-graph: rerank: no endpoint resolved; calls with rerank:true \
                 will report not_configured. Set --reranker-uds/--reranker-http (a non-loopback \
                 embedder URL is deliberately not reused)."
            );
            None
        }
        Some(ep) => {
            let client = build_client(&ep, None, rerank_timeout)?;
            let model =
                env_non_empty("LCG_RERANK_MODEL").unwrap_or_else(|| DEFAULT_RERANK_MODEL.into());
            eprintln!(
                "liminis-context-graph: rerank: endpoint={ep:?}, model={model}, default={}, candidates={}",
                if config.rerank_default { "on" } else { "off" },
                config.rerank_candidates
            );
            Some(Arc::new(SidecarReranker::new(Arc::new(client), model))
                as Arc<dyn lcg_core::rerank::Reranker>)
        }
    };

    let expansion_url = env_non_empty("LCG_EXPANSION_URL");
    let expansion_url_set = expansion_url.is_some();
    let expander = match expander_endpoint(flags, expansion_url, sidecar) {
        None => {
            eprintln!(
                "liminis-context-graph: query expansion: no endpoint resolved; calls with \
                 expand:true will report not_configured. Set --expander-uds/--expander-http or \
                 LCG_EXPANSION_URL (a non-loopback embedder URL is deliberately not reused)."
            );
            None
        }
        Some(ep) => {
            // The key is meant for the `LCG_EXPANSION_URL` provider only: an endpoint picked by
            // `--expander-*` or defaulted to the sidecar must not receive it.
            let from_url = flags.expander.is_none() && expansion_url_set;
            let api_key = env_non_empty("LCG_EXPANSION_API_KEY");
            if api_key.is_some() && !from_url {
                eprintln!(
                    "liminis-context-graph: query expansion: LCG_EXPANSION_API_KEY is only sent to \
                     the LCG_EXPANSION_URL endpoint; ignoring it for {ep:?}"
                );
            }
            let client = build_client(&ep, expansion_key(api_key, from_url), expansion_timeout)?;
            let model = env_non_empty("LCG_EXPANSION_MODEL")
                .unwrap_or_else(|| DEFAULT_EXPANSION_MODEL.into());
            eprintln!("liminis-context-graph: query expansion: endpoint={ep:?}, model={model}");
            Some(Arc::new(OaiQueryExpander::new(Arc::new(client), model))
                as Arc<dyn lcg_core::query_expand::QueryExpander>)
        }
    };

    state.set_search_stages(SearchStages::new(config, reranker, expander));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uds(p: &str) -> Endpoint {
        Endpoint::Uds(p.into())
    }
    fn http(u: &str) -> Endpoint {
        Endpoint::Http(u.into())
    }

    #[test]
    fn test_embedder_has_no_sidecar_even_when_the_environment_offers_one() {
        // #851: `--embedder test` must never fall through to a default socket or env URL.
        assert_eq!(
            sidecar_endpoint(
                Some(&EmbedderFlag::Test),
                true,
                Some("http://localhost:8080/v1".into())
            ),
            None
        );
    }

    #[test]
    fn sidecar_follows_the_embedder_resolution_order() {
        assert_eq!(
            sidecar_endpoint(
                Some(&EmbedderFlag::Uds("/e".into())),
                true,
                Some("u".into())
            ),
            Some(uds("/e"))
        );
        assert_eq!(
            sidecar_endpoint(
                Some(&EmbedderFlag::Http("http://127.0.0.1:9".into())),
                true,
                None
            ),
            Some(http("http://127.0.0.1:9"))
        );
        assert_eq!(
            sidecar_endpoint(None, true, Some("http://env".into())),
            Some(uds(DEFAULT_SIDECAR_UDS))
        );
        assert_eq!(
            sidecar_endpoint(None, false, Some("http://localhost:8080/v1".into())),
            Some(http("http://localhost:8080/v1"))
        );
        assert_eq!(sidecar_endpoint(None, false, None), None);
    }

    #[test]
    fn non_loopback_embedder_url_is_never_the_sidecar() {
        // A third-party embedding provider must not silently receive passages or queries.
        assert_eq!(
            sidecar_endpoint(None, false, Some("https://api.example.com/v1".into())),
            None
        );
        assert_eq!(
            sidecar_endpoint(
                Some(&EmbedderFlag::Http("https://api.example.com".into())),
                true,
                None
            ),
            None
        );
        // A trick host that merely starts with a loopback name is not loopback.
        assert_eq!(
            sidecar_endpoint(None, false, Some("http://localhost.evil.example/".into())),
            None
        );
        assert_eq!(
            sidecar_endpoint(None, false, Some("http://127.0.0.1@evil.example/".into())),
            None
        );
    }

    #[test]
    fn loopback_detection() {
        for u in [
            "http://localhost",
            "http://LOCALHOST:1/x",
            "http://127.0.0.1:8080",
            "http://127.1.2.3/",
            "http://[::1]:9/v1",
            "http://user:pw@localhost/",
        ] {
            assert!(is_loopback_http(u), "{u}");
        }
        for u in [
            "https://api.openai.com",
            "http://10.0.0.5:1",
            "http://localhost.evil.example",
            "http://[2001:db8::1]/",
            "",
        ] {
            assert!(!is_loopback_http(u), "{u}");
        }
    }

    #[test]
    fn empty_timeout_variables_use_the_defaults() {
        let _g = env_guard();
        std::env::set_var("LCG_RERANK_TIMEOUT_MS", "");
        std::env::set_var("LCG_EXPANSION_TIMEOUT_MS", "  ");
        let r = stage_timeouts();
        std::env::remove_var("LCG_RERANK_TIMEOUT_MS");
        std::env::remove_var("LCG_EXPANSION_TIMEOUT_MS");
        assert_eq!(
            r.unwrap(),
            (
                Duration::from_millis(DEFAULT_RERANK_TIMEOUT_MS),
                Duration::from_millis(DEFAULT_EXPANSION_TIMEOUT_MS)
            )
        );
    }

    #[test]
    fn empty_or_blank_env_values_count_as_unset() {
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("  \t".into())), None);
        assert_eq!(non_empty(None), None);
        assert_eq!(
            non_empty(Some(" http://x ".into())),
            Some("http://x".into())
        );
        // An empty `LCG_EXPANSION_URL` therefore falls through to the sidecar default.
        assert_eq!(
            expander_endpoint(
                &StageFlags::default(),
                non_empty(Some(String::new())),
                Some(uds("/s"))
            ),
            Some(uds("/s"))
        );
    }

    #[test]
    fn expansion_api_key_goes_only_to_the_url_endpoint() {
        assert_eq!(expansion_key(Some("k".into()), true), Some("k".into()));
        assert_eq!(expansion_key(Some("k".into()), false), None);
        assert_eq!(expansion_key(None, true), None);
    }

    #[test]
    fn reranker_flag_beats_the_sidecar() {
        let flags = StageFlags {
            reranker: Some(StageEndpointFlag::Http("http://r".into())),
            expander: None,
        };
        assert_eq!(
            reranker_endpoint(&flags, Some(uds("/s"))),
            Some(http("http://r"))
        );
        assert_eq!(
            reranker_endpoint(&StageFlags::default(), Some(uds("/s"))),
            Some(uds("/s"))
        );
        assert_eq!(reranker_endpoint(&StageFlags::default(), None), None);
    }

    #[test]
    fn expander_precedence_is_flag_then_env_then_sidecar() {
        let flags = StageFlags {
            reranker: None,
            expander: Some(StageEndpointFlag::Uds("/x".into())),
        };
        assert_eq!(
            expander_endpoint(&flags, Some("http://env".into()), Some(uds("/s"))),
            Some(uds("/x"))
        );
        assert_eq!(
            expander_endpoint(
                &StageFlags::default(),
                Some("http://env".into()),
                Some(uds("/s"))
            ),
            Some(http("http://env"))
        );
        assert_eq!(
            expander_endpoint(&StageFlags::default(), None, Some(uds("/s"))),
            Some(uds("/s"))
        );
        assert_eq!(expander_endpoint(&StageFlags::default(), None, None), None);
    }

    /// Serialises the tests that mutate the process environment (they run in parallel threads of
    /// one process, so an unrelated variable set by one would fail another's `is_ok` check).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn malformed_timeouts_fail_even_with_no_endpoint_configured() {
        let _g = env_guard();
        // `install` validates these before resolving endpoints, so this holds with no sidecar.
        std::env::set_var("LCG_RERANK_TIMEOUT_MS", "abc");
        let err = stage_timeouts().unwrap_err();
        std::env::remove_var("LCG_RERANK_TIMEOUT_MS");
        assert!(err.contains("LCG_RERANK_TIMEOUT_MS"), "{err}");

        std::env::set_var("LCG_EXPANSION_TIMEOUT_MS", "0");
        let err = stage_timeouts().unwrap_err();
        std::env::remove_var("LCG_EXPANSION_TIMEOUT_MS");
        assert!(err.contains("LCG_EXPANSION_TIMEOUT_MS"), "{err}");

        assert!(stage_timeouts().is_ok());
    }

    #[test]
    fn validate_settings_rejects_malformed_values_without_an_app_state() {
        let _g = env_guard();
        std::env::set_var("LCG_RERANK", "maybe");
        let err = validate_settings().unwrap_err();
        std::env::remove_var("LCG_RERANK");
        // The error must name `LCG_RERANK` itself, not a `LCG_RERANK_*` timeout variable.
        assert!(
            err.contains("LCG_RERANK") && !err.contains("TIMEOUT"),
            "{err}"
        );
    }
}
