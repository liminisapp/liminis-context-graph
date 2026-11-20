//! Pure, unit-testable CLI argument parsing for the `liminis-context-graph` binary.
//!
//! Kept as a hand-rolled scan (matching the pre-existing `--embedder-uds`/`--embedder-http`
//! pattern in `main.rs`) rather than adopting `clap`: a handful of flags doesn't clear the bar
//! for a new dependency, and a pure `parse_args` function is just as testable.

use lcg_core::anthropic_config::AnthropicCliArgs;

use crate::mcp::scope::Scope;

#[derive(Debug, Clone, PartialEq)]
pub enum EmbedderFlag {
    Uds(String),
    Http(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExtractorFlag {
    Uds(String),
    Http(String),
}

/// Endpoint of an optional passage-search stage service (issue #796): the reranker
/// (`--reranker-*`) or the query-expansion LLM (`--expander-*`).
#[derive(Debug, Clone, PartialEq)]
pub enum StageEndpointFlag {
    Uds(String),
    Http(String),
}

/// The `--reranker-*` / `--expander-*` flags. Kept out of [`CliMode`] on purpose: they are only
/// consumed after the app state is bootstrapped, and every `CliMode` literal in the tests would
/// otherwise need a new field.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StageFlags {
    pub reranker: Option<StageEndpointFlag>,
    pub expander: Option<StageEndpointFlag>,
}

/// Extracts the passage-stage flags from argv (excluding the program name). Each pair is
/// mutually exclusive. Other arguments are ignored here — [`parse_args`] owns their validation.
pub fn parse_stage_flags(args: &[String]) -> Result<StageFlags, String> {
    let mut uds: [Option<String>; 2] = [None, None];
    let mut http: [Option<String>; 2] = [None, None];
    let mut i = 0;
    while i < args.len() {
        let (slot, kind, name) = match args[i].as_str() {
            "--reranker-uds" => (0, 0, "--reranker-uds"),
            "--reranker-http" => (0, 1, "--reranker-http"),
            "--expander-uds" => (1, 0, "--expander-uds"),
            "--expander-http" => (1, 1, "--expander-http"),
            _ => {
                i += 1;
                continue;
            }
        };
        i += 1;
        let value = args.get(i).cloned().ok_or(format!(
            "{name} requires a {} argument",
            if kind == 0 { "socket path" } else { "URL" }
        ))?;
        if kind == 0 {
            uds[slot] = Some(value);
        } else {
            http[slot] = Some(value);
        }
        i += 1;
    }
    let mut pick = |slot: usize, what: &str| -> Result<Option<StageEndpointFlag>, String> {
        match (uds[slot].take(), http[slot].take()) {
            (Some(_), Some(_)) => Err(format!(
                "--{what}-uds and --{what}-http are mutually exclusive; specify only one"
            )),
            (Some(u), None) => Ok(Some(StageEndpointFlag::Uds(u))),
            (None, Some(h)) => Ok(Some(StageEndpointFlag::Http(h))),
            (None, None) => Ok(None),
        }
    };
    Ok(StageFlags {
        reranker: pick(0, "reranker")?,
        expander: pick(1, "expander")?,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum CliMode {
    /// Default: run the Unix-socket JSON-RPC service (existing behavior).
    Socket {
        embedder: Option<EmbedderFlag>,
        extractor: Option<ExtractorFlag>,
        /// `--anthropic-base-url` / `--anthropic-endpoint` (#346). Validated lazily, only when
        /// the Anthropic provider is the one selected.
        anthropic: AnthropicCliArgs,
        /// Raw `--writable-groups` value (issue #742). Validated at startup, together with
        /// `LCG_WRITABLE_GROUPS`, by `lcg_core::write_policy::WritableGroups::resolve`.
        writable_groups: Option<String>,
    },
    /// `--mcp-stdio`: run an MCP server over stdin/stdout (FR-001).
    Mcp {
        embedder: Option<EmbedderFlag>,
        extractor: Option<ExtractorFlag>,
        /// `--anthropic-base-url` / `--anthropic-endpoint` (#346). Rejected with `--connect`
        /// (the daemon does the extracting), so non-empty only in standalone mode.
        anthropic: AnthropicCliArgs,
        /// `--connect <path>`: attached mode — forward calls to a running service instead
        /// of opening the DB directly (FR-006).
        connect: Option<String>,
        scopes: Vec<Scope>,
        /// `--allow-remote-close`: only meaningful in attached mode (FR-005).
        allow_remote_close: bool,
        /// `--connect-eager` (issue #575, FR-008): only meaningful in attached mode. Restores
        /// the pre-#575 behavior of dialling `connect` at startup and exiting on failure,
        /// instead of the new default of deferring the dial until the first `tools/call`.
        connect_eager: bool,
        /// Raw `--writable-groups` value (issue #742). Rejected with `--connect` (the daemon
        /// enforces it), so `Some` only ever appears in standalone mode.
        writable_groups: Option<String>,
    },
    /// `--help` / `-h`: caller should print `usage()` and exit successfully.
    Help,
    /// `--version` / `-V`: caller should print the version and exit successfully.
    Version,
}

/// Usage text for `--help` and for the error message on an unrecognized argument. Kept in one
/// place so the CLI is self-documenting — running the binary is otherwise the only way to learn
/// the flags exist (`--mcp-stdio`, `--scope`, `--connect`, …), and they were previously
/// undiscoverable without reading the README.
pub fn usage() -> &'static str {
    "liminis-context-graph — local-first context graph engine

USAGE:
    liminis-context-graph [OPTIONS]

By default, runs the Unix-socket JSON-RPC service (binds .lcg/service.sock in the current
workspace). Pass --mcp-stdio to run as an MCP server over stdin/stdout instead.

OPTIONS:
    --mcp-stdio                Run as an MCP (Model Context Protocol) server over stdin/stdout
                               instead of binding the Unix socket.
    --scope <LIST>             (MCP only) Comma-separated tool scopes to advertise in
                               tools/list: read, write, cypher, admin, all. Default: all.
    --connect <SOCKET_PATH>    (MCP only) Attached mode: forward each call over this socket to
                               an already-running service instead of opening the database.
    --allow-remote-close       (MCP attached mode only) Allow knowledge_close to shut down the
                               remote service. No effect in standalone mode.
    --connect-eager            (MCP attached mode only) Dial --connect's socket at startup and
                               exit if unreachable, restoring the pre-issue-575 behavior. By
                               default the dial is deferred until the first tools/call, so
                               initialize and tools/list succeed even before the daemon is up.
                               No effect without --connect.
    --writable-groups <LIST>   Comma-separated group ids clients may change (also settable via
                               LCG_WRITABLE_GROUPS; the flag wins). Writes to any other group
                               are refused with JSON-RPC error -32004, and knowledge_query_cypher
                               is disabled. Group-less writes go to the default group, which must
                               be listed to stay writable. Unset = every group writable. An empty
                               or malformed list fails startup. Not valid with --connect: set it
                               on the daemon.
    --embedder-uds <SOCKET>    Reach the embedding sidecar over this Unix socket.
    --embedder-http <URL>      Reach the embedding sidecar over this HTTP URL.
                               (--embedder-uds and --embedder-http are mutually exclusive.)
    --extractor-uds <SOCKET>  Reach a local OpenAI-compatible extraction endpoint over this
                               Unix socket (e.g. the CoreML sidecar's /v1/chat/completions
                               route), instead of Anthropic's hosted API.
    --extractor-http <URL>    Reach a local OpenAI-compatible extraction endpoint over this
                               HTTP URL.
                               (--extractor-uds and --extractor-http are mutually exclusive.)
    --anthropic-base-url <URL> Base URL of an Anthropic-compatible gateway (OpenRouter, LiteLLM,
                               a corporate proxy); `/v1/messages` is appended. Also settable via
                               ANTHROPIC_BASE_URL; the flag wins. Only used when the Anthropic
                               provider is selected (ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN).
    --anthropic-endpoint <URL> Full Anthropic endpoint URL, used verbatim as the POST target
                               (e.g. a Bedrock gateway's <base>/<deployment>/v1/invoke). Also
                               settable via LCG_ANTHROPIC_ENDPOINT; wins over the base URL.
                               Auth, body shape and the other gateway settings are environment
                               variables (ANTHROPIC_AUTH_TOKEN, LCG_ANTHROPIC_OMIT_MODEL,
                               LCG_ANTHROPIC_VERSION, LCG_ANTHROPIC_VERSION_IN,
                               LCG_ANTHROPIC_PROMPT_CACHING) — see docs/configuration.md.
                               Neither flag is valid with --connect: set them on the daemon.
    --reranker-uds <SOCKET>    Reach the cross-encoder reranker (POST /v1/rerank) over this Unix
                               socket. Default: the embedder's sidecar. Only used by
                               knowledge_search_passages and knowledge_graph_search when a
                               call (or LCG_RERANK=on) opts in.
    --reranker-http <URL>      Reach the reranker over this HTTP base URL.
                               (--reranker-uds and --reranker-http are mutually exclusive.)
    --expander-uds <SOCKET>    Reach the query-expansion LLM (an OpenAI-compatible
                               /v1/chat/completions route) over this Unix socket. Default: the
                               embedder's sidecar, or LCG_EXPANSION_URL if set. Only used when a
                               call passes expand: true.
    --expander-http <URL>      Reach the query-expansion LLM over this HTTP base URL.
                               (--expander-uds and --expander-http are mutually exclusive.)
                               None of the four stage flags is valid with --connect: set them
                               on the daemon.
    -h, --help                 Print this help and exit.
    -V, --version              Print version and exit.

Documentation: https://docs.liminis.app/liminis-context-graph/
"
}

/// Parses argv (excluding the program name at index 0).
pub fn parse_args(args: &[String]) -> Result<CliMode, String> {
    let mut mcp_stdio = false;
    let mut connect: Option<String> = None;
    let mut scope_arg: Option<String> = None;
    let mut writable_groups: Option<String> = None;
    let mut allow_remote_close = false;
    let mut connect_eager = false;
    let mut cli_uds: Option<String> = None;
    let mut cli_http: Option<String> = None;
    let mut extractor_cli_uds: Option<String> = None;
    let mut extractor_cli_http: Option<String> = None;
    let mut anthropic = AnthropicCliArgs::default();
    let mut want_help = false;
    let mut want_version = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => want_help = true,
            "--version" | "-V" => want_version = true,
            "--mcp-stdio" => mcp_stdio = true,
            "--allow-remote-close" => allow_remote_close = true,
            "--connect-eager" => connect_eager = true,
            "--connect" => {
                i += 1;
                connect = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--connect requires a socket path argument")?,
                );
            }
            "--embedder-uds" => {
                i += 1;
                cli_uds = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--embedder-uds requires a socket path argument")?,
                );
            }
            "--embedder-http" => {
                i += 1;
                cli_http = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--embedder-http requires a URL argument")?,
                );
            }
            "--extractor-uds" => {
                i += 1;
                extractor_cli_uds = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--extractor-uds requires a socket path argument")?,
                );
            }
            "--extractor-http" => {
                i += 1;
                extractor_cli_http = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--extractor-http requires a URL argument")?,
                );
            }
            // Passage-stage endpoints (issue #796): consumed by `parse_stage_flags`; validated
            // (value present, pairs exclusive) below.
            "--reranker-uds" | "--reranker-http" | "--expander-uds" | "--expander-http" => {
                i += 1;
            }
            "--anthropic-base-url" => {
                i += 1;
                anthropic.base_url = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--anthropic-base-url requires a URL argument")?,
                );
            }
            "--anthropic-endpoint" => {
                i += 1;
                anthropic.endpoint = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--anthropic-endpoint requires a URL argument")?,
                );
            }
            "--scope" => {
                i += 1;
                scope_arg = Some(args.get(i).cloned().ok_or("--scope requires a value")?);
            }
            "--writable-groups" => {
                i += 1;
                writable_groups = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--writable-groups requires a value")?,
                );
            }
            arg => {
                if let Some(v) = arg.strip_prefix("--scope=") {
                    scope_arg = Some(v.to_string());
                } else if let Some(v) = arg.strip_prefix("--writable-groups=") {
                    writable_groups = Some(v.to_string());
                } else {
                    // Previously unknown arguments were silently ignored, so a typo like
                    // `--scoep=read` ran with default scope with no warning. Reject them.
                    return Err(format!("unknown argument: '{arg}'\n\n{}", usage()));
                }
            }
        }
        i += 1;
    }

    // --help / --version short-circuit everything else (including the validation below), so
    // `--help` prints usage even alongside otherwise-invalid flag combinations. Help wins over
    // version when both are present.
    if want_help {
        return Ok(CliMode::Help);
    }
    if want_version {
        return Ok(CliMode::Version);
    }

    let stage_flags = parse_stage_flags(args)?;

    if cli_uds.is_some() && cli_http.is_some() {
        return Err(
            "--embedder-uds and --embedder-http are mutually exclusive; specify only one"
                .to_string(),
        );
    }
    let embedder = match (cli_uds, cli_http) {
        (Some(u), _) => Some(EmbedderFlag::Uds(u)),
        (_, Some(h)) => Some(EmbedderFlag::Http(h)),
        _ => None,
    };

    if extractor_cli_uds.is_some() && extractor_cli_http.is_some() {
        return Err(
            "--extractor-uds and --extractor-http are mutually exclusive; specify only one"
                .to_string(),
        );
    }
    let extractor = match (extractor_cli_uds, extractor_cli_http) {
        (Some(u), _) => Some(ExtractorFlag::Uds(u)),
        (_, Some(h)) => Some(ExtractorFlag::Http(h)),
        _ => None,
    };

    if !mcp_stdio {
        if connect.is_some() || scope_arg.is_some() {
            return Err("--connect and --scope require --mcp-stdio".to_string());
        }
        // --allow-remote-close only affects attached MCP mode; silently accepted elsewhere
        // per the spec's explicit "no effect" edge case rather than erroring.
        return Ok(CliMode::Socket {
            embedder,
            extractor,
            anthropic,
            writable_groups,
        });
    }

    if connect.is_some() && writable_groups.is_some() {
        return Err(
            "--writable-groups cannot be combined with --connect: the allow-list is \
             enforced by the daemon, so set --writable-groups (or LCG_WRITABLE_GROUPS) on the \
             daemon, not on the attached MCP process"
                .to_string(),
        );
    }

    if connect.is_some() && (stage_flags.reranker.is_some() || stage_flags.expander.is_some()) {
        return Err(
            "--reranker-uds/--reranker-http/--expander-uds/--expander-http cannot be combined \
             with --connect: the daemon runs the search, so set them on the daemon, not on the \
             attached MCP process"
                .to_string(),
        );
    }

    if connect.is_some() && !anthropic.is_empty() {
        return Err(
            "--anthropic-base-url/--anthropic-endpoint cannot be combined with --connect: the \
             daemon does the extracting, so set them (or ANTHROPIC_BASE_URL / \
             LCG_ANTHROPIC_ENDPOINT) on the daemon, not on the attached MCP process"
                .to_string(),
        );
    }

    if allow_remote_close && connect.is_none() {
        eprintln!(
            "liminis-context-graph: --allow-remote-close has no effect in standalone MCP mode \
             (no --connect); ignoring"
        );
    }
    if connect_eager && connect.is_none() {
        eprintln!(
            "liminis-context-graph: --connect-eager has no effect in standalone MCP mode \
             (no --connect); ignoring"
        );
    }

    let scopes = Scope::parse_list(scope_arg.as_deref().unwrap_or("all"))?;

    Ok(CliMode::Mcp {
        embedder,
        extractor,
        anthropic,
        connect,
        scopes,
        allow_remote_close,
        connect_eager,
        writable_groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_to_socket_mode() {
        assert_eq!(
            parse_args(&args(&[])).unwrap(),
            CliMode::Socket {
                embedder: None,
                extractor: None,
                anthropic: AnthropicCliArgs::default(),
                writable_groups: None
            }
        );
    }

    #[test]
    fn socket_mode_with_embedder_uds() {
        assert_eq!(
            parse_args(&args(&["--embedder-uds", "/tmp/x.sock"])).unwrap(),
            CliMode::Socket {
                embedder: Some(EmbedderFlag::Uds("/tmp/x.sock".to_string())),
                extractor: None,
                anthropic: AnthropicCliArgs::default(),
                writable_groups: None
            }
        );
    }

    #[test]
    fn embedder_uds_and_http_are_mutually_exclusive() {
        let err = parse_args(&args(&[
            "--embedder-uds",
            "/tmp/x.sock",
            "--embedder-http",
            "http://x",
        ]))
        .unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn socket_mode_with_extractor_uds() {
        assert_eq!(
            parse_args(&args(&["--extractor-uds", "/tmp/x.sock"])).unwrap(),
            CliMode::Socket {
                embedder: None,
                extractor: Some(ExtractorFlag::Uds("/tmp/x.sock".to_string())),
                anthropic: AnthropicCliArgs::default(),
                writable_groups: None
            }
        );
    }

    #[test]
    fn socket_mode_with_extractor_http() {
        assert_eq!(
            parse_args(&args(&["--extractor-http", "http://x"])).unwrap(),
            CliMode::Socket {
                embedder: None,
                extractor: Some(ExtractorFlag::Http("http://x".to_string())),
                anthropic: AnthropicCliArgs::default(),
                writable_groups: None
            }
        );
    }

    #[test]
    fn extractor_uds_and_http_are_mutually_exclusive() {
        let err = parse_args(&args(&[
            "--extractor-uds",
            "/tmp/x.sock",
            "--extractor-http",
            "http://x",
        ]))
        .unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn embedder_and_extractor_flags_are_independent() {
        assert_eq!(
            parse_args(&args(&[
                "--embedder-uds",
                "/tmp/e.sock",
                "--extractor-http",
                "http://x",
            ]))
            .unwrap(),
            CliMode::Socket {
                embedder: Some(EmbedderFlag::Uds("/tmp/e.sock".to_string())),
                extractor: Some(ExtractorFlag::Http("http://x".to_string())),
                anthropic: AnthropicCliArgs::default(),
                writable_groups: None
            }
        );
    }

    #[test]
    fn mcp_stdio_defaults_to_all_scope_standalone() {
        match parse_args(&args(&["--mcp-stdio"])).unwrap() {
            CliMode::Mcp {
                connect,
                scopes,
                allow_remote_close,
                connect_eager,
                embedder,
                extractor,
                anthropic,
                writable_groups: _,
            } => {
                assert_eq!(anthropic, AnthropicCliArgs::default());
                assert_eq!(connect, None);
                assert_eq!(scopes, Scope::ALL.to_vec());
                assert!(!allow_remote_close);
                assert!(!connect_eager);
                assert_eq!(embedder, None);
                assert_eq!(extractor, None);
            }
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn anthropic_flags_parse_in_socket_mode() {
        match parse_args(&args(&[
            "--anthropic-base-url",
            "https://gw.example.com",
            "--anthropic-endpoint",
            "https://gw.example.com/dep/v1/invoke",
        ]))
        .unwrap()
        {
            CliMode::Socket { anthropic, .. } => {
                assert_eq!(
                    anthropic.base_url.as_deref(),
                    Some("https://gw.example.com")
                );
                assert_eq!(
                    anthropic.endpoint.as_deref(),
                    Some("https://gw.example.com/dep/v1/invoke")
                );
            }
            other => panic!("expected Socket mode, got {other:?}"),
        }
    }

    #[test]
    fn anthropic_flags_parse_in_standalone_mcp_mode() {
        match parse_args(&args(&[
            "--mcp-stdio",
            "--anthropic-base-url",
            "https://gw",
        ]))
        .unwrap()
        {
            CliMode::Mcp { anthropic, .. } => {
                assert_eq!(anthropic.base_url.as_deref(), Some("https://gw"));
                assert_eq!(anthropic.endpoint, None);
            }
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn anthropic_flags_require_a_value() {
        let e = parse_args(&args(&["--anthropic-base-url"])).unwrap_err();
        assert!(e.contains("--anthropic-base-url requires a URL"), "{e}");
        let e = parse_args(&args(&["--anthropic-endpoint"])).unwrap_err();
        assert!(e.contains("--anthropic-endpoint requires a URL"), "{e}");
    }

    #[test]
    fn anthropic_flags_rejected_with_connect() {
        for flag in ["--anthropic-base-url", "--anthropic-endpoint"] {
            let e = parse_args(&args(&[
                "--mcp-stdio",
                "--connect",
                "/tmp/s.sock",
                flag,
                "https://gw",
            ]))
            .unwrap_err();
            assert!(e.contains("--connect"), "{e}");
        }
    }

    #[test]
    fn anthropic_flags_do_not_affect_extractor_flag() {
        match parse_args(&args(&[
            "--extractor-http",
            "http://localhost:1/v1",
            "--anthropic-base-url",
            "https://gw",
        ]))
        .unwrap()
        {
            CliMode::Socket {
                extractor,
                anthropic,
                ..
            } => {
                assert_eq!(
                    extractor,
                    Some(ExtractorFlag::Http("http://localhost:1/v1".to_string()))
                );
                assert!(anthropic.base_url.is_some());
            }
            other => panic!("expected Socket mode, got {other:?}"),
        }
    }

    #[test]
    fn usage_documents_anthropic_flags() {
        assert!(usage().contains("--anthropic-base-url"));
        assert!(usage().contains("--anthropic-endpoint"));
    }

    #[test]
    fn mcp_stdio_with_scope_flag_equals_form() {
        match parse_args(&args(&["--mcp-stdio", "--scope=read,admin"])).unwrap() {
            CliMode::Mcp { scopes, .. } => {
                assert_eq!(scopes, vec![Scope::Read, Scope::Admin]);
            }
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn mcp_stdio_with_scope_flag_space_form() {
        match parse_args(&args(&["--mcp-stdio", "--scope", "cypher"])).unwrap() {
            CliMode::Mcp { scopes, .. } => {
                assert_eq!(scopes, vec![Scope::Cypher]);
            }
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn mcp_stdio_attached_mode() {
        match parse_args(&args(&["--mcp-stdio", "--connect", ".lcg/service.sock"])).unwrap() {
            CliMode::Mcp { connect, .. } => {
                assert_eq!(connect, Some(".lcg/service.sock".to_string()));
            }
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn mcp_stdio_attached_mode_with_allow_remote_close() {
        match parse_args(&args(&[
            "--mcp-stdio",
            "--connect",
            ".lcg/service.sock",
            "--allow-remote-close",
        ]))
        .unwrap()
        {
            CliMode::Mcp {
                allow_remote_close, ..
            } => assert!(allow_remote_close),
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn allow_remote_close_without_connect_is_accepted_but_inert() {
        // Edge case from the spec: no error, just a no-op (stderr notice only).
        let result = parse_args(&args(&["--mcp-stdio", "--allow-remote-close"]));
        assert!(result.is_ok());
    }

    #[test]
    fn mcp_stdio_attached_mode_with_connect_eager() {
        match parse_args(&args(&[
            "--mcp-stdio",
            "--connect",
            ".lcg/service.sock",
            "--connect-eager",
        ]))
        .unwrap()
        {
            CliMode::Mcp { connect_eager, .. } => assert!(connect_eager),
            other => panic!("expected Mcp mode, got {other:?}"),
        }
    }

    #[test]
    fn connect_eager_without_connect_is_accepted_but_inert() {
        // Mirrors allow_remote_close_without_connect_is_accepted_but_inert: no error, just a
        // no-op (stderr notice only) — --connect-eager has nothing to be eager about without
        // --connect.
        let result = parse_args(&args(&["--mcp-stdio", "--connect-eager"]));
        assert!(result.is_ok());
    }

    #[test]
    fn connect_without_mcp_stdio_is_rejected() {
        let err = parse_args(&args(&["--connect", ".lcg/service.sock"])).unwrap_err();
        assert!(err.contains("--mcp-stdio"));
    }

    #[test]
    fn scope_without_mcp_stdio_is_rejected() {
        let err = parse_args(&args(&["--scope=read"])).unwrap_err();
        assert!(err.contains("--mcp-stdio"));
    }

    #[test]
    fn unknown_scope_is_rejected() {
        let err = parse_args(&args(&["--mcp-stdio", "--scope=bogus"])).unwrap_err();
        assert!(err.contains("bogus"));
    }

    #[test]
    fn help_flag_is_recognized() {
        assert_eq!(parse_args(&args(&["--help"])).unwrap(), CliMode::Help);
        assert_eq!(parse_args(&args(&["-h"])).unwrap(), CliMode::Help);
    }

    #[test]
    fn version_flag_is_recognized() {
        assert_eq!(parse_args(&args(&["--version"])).unwrap(), CliMode::Version);
        assert_eq!(parse_args(&args(&["-V"])).unwrap(), CliMode::Version);
    }

    #[test]
    fn help_wins_over_version_and_other_flags() {
        // Help short-circuits even alongside otherwise-invalid combinations.
        assert_eq!(
            parse_args(&args(&["--version", "--help"])).unwrap(),
            CliMode::Help
        );
        assert_eq!(
            parse_args(&args(&["--mcp-stdio", "--help"])).unwrap(),
            CliMode::Help
        );
    }

    #[test]
    fn unknown_flag_is_rejected_with_usage() {
        // Previously silently ignored; a typo must now surface as an error carrying usage text.
        let err = parse_args(&args(&["--scoep=read"])).unwrap_err();
        assert!(err.contains("unknown argument"));
        assert!(err.contains("--scoep=read"));
        assert!(err.contains("USAGE:"));
    }

    #[test]
    fn known_embedder_flag_still_accepted_after_unknown_rejection() {
        // Guards the app's invocation contract: it spawns the binary with `--embedder-uds`.
        assert_eq!(
            parse_args(&args(&["--embedder-uds", "/tmp/x.sock"])).unwrap(),
            CliMode::Socket {
                embedder: Some(EmbedderFlag::Uds("/tmp/x.sock".to_string())),
                extractor: None,
                anthropic: AnthropicCliArgs::default(),
                writable_groups: None
            }
        );
    }

    #[test]
    fn writable_groups_both_forms_in_socket_and_standalone_mcp() {
        for form in [
            args(&["--writable-groups", "a,b"]),
            args(&["--writable-groups=a,b"]),
        ] {
            match parse_args(&form).unwrap() {
                CliMode::Socket {
                    writable_groups, ..
                } => assert_eq!(writable_groups.as_deref(), Some("a,b")),
                other => panic!("expected Socket, got {other:?}"),
            }
        }
        match parse_args(&args(&["--mcp-stdio", "--writable-groups=a"])).unwrap() {
            CliMode::Mcp {
                writable_groups, ..
            } => assert_eq!(writable_groups.as_deref(), Some("a")),
            other => panic!("expected Mcp, got {other:?}"),
        }
    }

    #[test]
    fn writable_groups_defaults_to_none_and_keeps_raw_empty_value_for_startup_validation() {
        match parse_args(&args(&[])).unwrap() {
            CliMode::Socket {
                writable_groups, ..
            } => assert_eq!(writable_groups, None),
            other => panic!("{other:?}"),
        }
        // Validation (empty / malformed) is deliberately deferred to startup so flag and env
        // share one strict resolver; the parser must not swallow an empty value.
        match parse_args(&args(&["--writable-groups="])).unwrap() {
            CliMode::Socket {
                writable_groups, ..
            } => assert_eq!(writable_groups.as_deref(), Some("")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn writable_groups_requires_a_value() {
        let err = parse_args(&args(&["--writable-groups"])).unwrap_err();
        assert!(err.contains("--writable-groups requires a value"), "{err}");
    }

    #[test]
    fn stage_flags_are_rejected_with_connect() {
        for flag in [
            "--reranker-uds",
            "--reranker-http",
            "--expander-uds",
            "--expander-http",
        ] {
            let err = parse_args(&args(&[
                "--mcp-stdio",
                "--connect",
                ".lcg/service.sock",
                flag,
                "/x",
            ]))
            .unwrap_err();
            assert!(
                err.contains("--connect") && err.contains("daemon"),
                "{flag}: {err}"
            );
        }
    }

    #[test]
    fn writable_groups_is_rejected_with_connect() {
        let err = parse_args(&args(&[
            "--mcp-stdio",
            "--connect",
            ".lcg/service.sock",
            "--writable-groups=a",
        ]))
        .unwrap_err();
        assert!(err.contains("--connect") && err.contains("daemon"), "{err}");
    }

    #[test]
    fn usage_text_lists_the_key_flags() {
        let u = usage();
        for flag in [
            "--mcp-stdio",
            "--scope",
            "--connect",
            "--connect-eager",
            "--writable-groups",
            "--embedder-uds",
            "--extractor-uds",
            "--help",
        ] {
            assert!(u.contains(flag), "usage missing {flag}");
        }
    }

    #[test]
    fn stage_flags_default_to_none() {
        assert_eq!(
            parse_stage_flags(&args(&[])).unwrap(),
            StageFlags::default()
        );
        assert_eq!(
            parse_stage_flags(&args(&["--mcp-stdio", "--embedder-uds", "/x"])).unwrap(),
            StageFlags::default()
        );
    }

    #[test]
    fn reranker_and_expander_flags_parse_independently() {
        let f = parse_stage_flags(&args(&[
            "--reranker-uds",
            "/tmp/r.sock",
            "--expander-http",
            "http://llm:1",
        ]))
        .unwrap();
        assert_eq!(
            f.reranker,
            Some(StageEndpointFlag::Uds("/tmp/r.sock".into()))
        );
        assert_eq!(
            f.expander,
            Some(StageEndpointFlag::Http("http://llm:1".into()))
        );
        let f = parse_stage_flags(&args(&["--reranker-http", "http://r:2"])).unwrap();
        assert_eq!(
            f.reranker,
            Some(StageEndpointFlag::Http("http://r:2".into()))
        );
        assert_eq!(f.expander, None);
    }

    #[test]
    fn stage_flag_pairs_are_mutually_exclusive() {
        let e = parse_stage_flags(&args(&[
            "--reranker-uds",
            "/a",
            "--reranker-http",
            "http://b",
        ]))
        .unwrap_err();
        assert!(e.contains("mutually exclusive"), "{e}");
        let e = parse_stage_flags(&args(&[
            "--expander-uds",
            "/a",
            "--expander-http",
            "http://b",
        ]))
        .unwrap_err();
        assert!(e.contains("mutually exclusive"), "{e}");
        // parse_args surfaces the same error.
        assert!(parse_args(&args(&[
            "--reranker-uds",
            "/a",
            "--reranker-http",
            "http://b"
        ]))
        .is_err());
    }

    #[test]
    fn stage_flags_require_a_value() {
        for f in [
            "--reranker-uds",
            "--reranker-http",
            "--expander-uds",
            "--expander-http",
        ] {
            assert!(parse_stage_flags(&args(&[f])).is_err(), "{f}");
            assert!(parse_args(&args(&[f])).is_err(), "{f}");
        }
    }

    #[test]
    fn parse_args_accepts_stage_flags_without_changing_the_mode() {
        assert_eq!(
            parse_args(&args(&[
                "--reranker-uds",
                "/r",
                "--expander-http",
                "http://e"
            ]))
            .unwrap(),
            parse_args(&args(&[])).unwrap()
        );
    }
}
