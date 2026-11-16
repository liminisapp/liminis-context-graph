//! Resolved endpoint / auth / request-shape configuration for the Anthropic Messages
//! extraction provider (#346, ADR-0346).
//!
//! The Anthropic provider historically posted to a hardcoded `https://api.anthropic.com/v1/messages`
//! with `x-api-key` auth. This module lets operators point it at an Anthropic-compatible gateway
//! (Bedrock access gateway, OpenRouter, LiteLLM, a corporate proxy) without changing the
//! Messages request/response handling.
//!
//! Everything is resolved **once** into an immutable [`AnthropicEndpointConfig`] by the pure
//! [`AnthropicEndpointConfig::resolve`] — the environment is injected as a closure so unit tests
//! never touch process env (which would race under `RUST_TEST_THREADS=4`).
//!
//! Credentials never appear in `Debug` output, startup summaries or error strings.

use std::fmt;

/// Stock Anthropic Messages endpoint.
pub const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
/// Stock `anthropic-version` value (sent as a header by default).
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// Env var: base URL (`/v1/messages` is appended), matching the official Anthropic SDKs.
pub const ENV_BASE_URL: &str = "ANTHROPIC_BASE_URL";
/// Env var: full endpoint URL, used verbatim as the POST target.
pub const ENV_ENDPOINT: &str = "LCG_ANTHROPIC_ENDPOINT";
/// Env var: Bearer credential (`Authorization: Bearer …`), per the SDK convention.
pub const ENV_AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
/// Env var: `x-api-key` credential.
pub const ENV_API_KEY: &str = "ANTHROPIC_API_KEY";
/// Env var: omit the top-level `model` field from request bodies.
pub const ENV_OMIT_MODEL: &str = "LCG_ANTHROPIC_OMIT_MODEL";
/// Env var: `anthropic_version` value.
pub const ENV_VERSION: &str = "LCG_ANTHROPIC_VERSION";
/// Env var: `anthropic_version` placement (`header` | `body`).
pub const ENV_VERSION_IN: &str = "LCG_ANTHROPIC_VERSION_IN";
/// Env var: prompt caching (`on` | `off`).
pub const ENV_PROMPT_CACHING: &str = "LCG_ANTHROPIC_PROMPT_CACHING";
/// CLI flag name for the base URL (used in error messages).
pub const FLAG_BASE_URL: &str = "--anthropic-base-url";
/// CLI flag name for the full endpoint (used in error messages).
pub const FLAG_ENDPOINT: &str = "--anthropic-endpoint";

/// Which credential scheme is sent on every request.
#[derive(Clone, PartialEq, Eq)]
pub enum AnthropicAuth {
    /// `x-api-key: <key>` (stock Anthropic).
    ApiKey(String),
    /// `Authorization: Bearer <token>` (gateways).
    Bearer(String),
}

impl AnthropicAuth {
    /// Scheme name for logs — never the credential value.
    pub fn scheme(&self) -> &'static str {
        match self {
            AnthropicAuth::ApiKey(_) => "x-api-key",
            AnthropicAuth::Bearer(_) => "bearer",
        }
    }
}

// Manual Debug: the credential must never be printed.
impl fmt::Debug for AnthropicAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AnthropicAuth::{}(<redacted>)", self.scheme())
    }
}

/// Where `anthropic_version` is carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPlacement {
    /// `anthropic-version` request header (stock Anthropic).
    Header,
    /// `anthropic_version` field in the JSON body (Bedrock).
    Body,
}

/// CLI-sourced overrides (flags win over env).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnthropicCliArgs {
    /// `--anthropic-base-url`
    pub base_url: Option<String>,
    /// `--anthropic-endpoint`
    pub endpoint: Option<String>,
}

impl AnthropicCliArgs {
    pub fn is_empty(&self) -> bool {
        self.base_url.is_none() && self.endpoint.is_none()
    }
}

/// Immutable resolved configuration shared by every Anthropic-backed call.
#[derive(Clone)]
pub struct AnthropicEndpointConfig {
    /// Final POST target.
    pub url: String,
    pub auth: AnthropicAuth,
    /// Omit the top-level `model` from request bodies.
    pub omit_model: bool,
    pub version: String,
    pub version_in: VersionPlacement,
    /// When false, suppresses both the `anthropic-beta` header and `cache_control` blocks.
    pub prompt_caching: bool,
    /// Human-readable notes produced during resolution (for the startup log).
    pub notes: Vec<String>,
}

// Manual Debug so the credential can never leak through `{:?}`.
impl fmt::Debug for AnthropicEndpointConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicEndpointConfig")
            .field("url", &redact_url_for_log(&self.url))
            .field("auth", &self.auth)
            .field("omit_model", &self.omit_model)
            .field("version", &self.version)
            .field("version_in", &self.version_in)
            .field("prompt_caching", &self.prompt_caching)
            .finish()
    }
}

impl AnthropicEndpointConfig {
    /// Stock configuration around `api_key` and the stock endpoint.
    pub fn default_with_key(api_key: String) -> Self {
        Self::with_url_and_key(DEFAULT_ANTHROPIC_URL.to_string(), api_key)
    }

    /// Stock request shape, custom URL and `x-api-key` (tests, `AnthropicExtractor::with_url`).
    pub fn with_url_and_key(url: String, api_key: String) -> Self {
        Self {
            url,
            auth: AnthropicAuth::ApiKey(api_key),
            omit_model: false,
            version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            version_in: VersionPlacement::Header,
            prompt_caching: true,
            notes: Vec::new(),
        }
    }

    /// True when nothing differs from the stock Anthropic endpoint/shape.
    pub fn is_default(&self) -> bool {
        self.url == DEFAULT_ANTHROPIC_URL
            && matches!(self.auth, AnthropicAuth::ApiKey(_))
            && !self.omit_model
            && self.version == DEFAULT_ANTHROPIC_VERSION
            && self.version_in == VersionPlacement::Header
            && self.prompt_caching
    }

    /// Endpoint for logs: userinfo stripped, query redacted, fragment dropped.
    pub fn endpoint_for_log(&self) -> String {
        redact_url_for_log(&self.url)
    }

    /// `x-api-key` or `bearer` — never the value.
    pub fn auth_scheme(&self) -> &'static str {
        self.auth.scheme()
    }

    /// Fields appended to the startup log line: `endpoint=…, auth=…` plus any non-default shape.
    pub fn startup_summary(&self) -> String {
        let mut s = format!(
            "endpoint={}, auth={}",
            self.endpoint_for_log(),
            self.auth_scheme()
        );
        if self.omit_model {
            s.push_str(", model_in_body=false");
        }
        if self.version_in == VersionPlacement::Body {
            s.push_str(", anthropic_version_in=body");
        }
        if self.version != DEFAULT_ANTHROPIC_VERSION {
            s.push_str(&format!(", anthropic_version={}", self.version));
        }
        if !self.prompt_caching {
            s.push_str(", prompt_caching=off");
        }
        s
    }

    /// Resolves the configuration from CLI overrides and the (injected) environment.
    ///
    /// Precedence for the URL, highest first: `--anthropic-endpoint`, `LCG_ANTHROPIC_ENDPOINT`
    /// (both verbatim), then `--anthropic-base-url`, `ANTHROPIC_BASE_URL` (both bases, to which
    /// `/v1/messages` is appended), then the stock default.
    ///
    /// Errors name the offending setting. Empty-but-set values are errors for every new
    /// variable; `ANTHROPIC_API_KEY` keeps its historical "set (even empty) selects Anthropic"
    /// semantics.
    pub fn resolve(
        env: &dyn Fn(&str) -> Option<String>,
        cli: &AnthropicCliArgs,
    ) -> Result<Self, String> {
        let mut notes: Vec<String> = Vec::new();

        // ── URL ──
        let endpoint = pick(cli.endpoint.as_deref(), FLAG_ENDPOINT, ENV_ENDPOINT, env)?;
        let base = pick(cli.base_url.as_deref(), FLAG_BASE_URL, ENV_BASE_URL, env)?;

        let url = match (endpoint, base) {
            (Some((name, v)), base) => {
                if let Some((bname, _)) = base {
                    notes.push(format!(
                        "{bname} ignored: {name} (full endpoint) takes precedence"
                    ));
                }
                validate_url(&name, &v)?;
                v
            }
            (None, Some((name, v))) => join_base(&name, &v)?,
            (None, None) => DEFAULT_ANTHROPIC_URL.to_string(),
        };

        // ── Auth ──
        let token = env_non_empty(env, ENV_AUTH_TOKEN)?;
        let api_key = env(ENV_API_KEY);
        let auth = match token {
            Some(t) => {
                if api_key.is_some() {
                    notes.push(format!(
                        "{ENV_API_KEY} ignored: {ENV_AUTH_TOKEN} (bearer) takes precedence"
                    ));
                }
                AnthropicAuth::Bearer(t)
            }
            None => AnthropicAuth::ApiKey(api_key.unwrap_or_default()),
        };

        // ── Request shape ──
        let omit_model = match env_non_empty(env, ENV_OMIT_MODEL)? {
            None => false,
            Some(v) => parse_bool(ENV_OMIT_MODEL, &v)?,
        };

        let version_explicit = env_non_empty(env, ENV_VERSION)?;
        let version_in = match env_non_empty(env, ENV_VERSION_IN)? {
            None => VersionPlacement::Header,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "header" => VersionPlacement::Header,
                "body" => VersionPlacement::Body,
                _ => {
                    return Err(format!(
                        "{ENV_VERSION_IN} must be `header` or `body`, got `{v}`"
                    ))
                }
            },
        };
        if version_in == VersionPlacement::Body && version_explicit.is_none() {
            return Err(format!(
                "{ENV_VERSION_IN}=body requires {ENV_VERSION} to be set explicitly \
                 (e.g. `bedrock-2023-05-31`)"
            ));
        }
        let version = version_explicit.unwrap_or_else(|| DEFAULT_ANTHROPIC_VERSION.to_string());

        let prompt_caching = match env_non_empty(env, ENV_PROMPT_CACHING)? {
            None => true,
            Some(v) => match v.to_ascii_lowercase().as_str() {
                "on" => true,
                "off" => false,
                _ => {
                    return Err(format!(
                        "{ENV_PROMPT_CACHING} must be `on` or `off`, got `{v}`"
                    ))
                }
            },
        };

        Ok(Self {
            url,
            auth,
            omit_model,
            version,
            version_in,
            prompt_caching,
            notes,
        })
    }
}

/// Picks the CLI value (labelled with the flag) or else the env value (labelled with the var).
/// Empty-but-set is an error.
fn pick(
    cli: Option<&str>,
    flag: &str,
    var: &str,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<(String, String)>, String> {
    if let Some(v) = cli {
        if v.trim().is_empty() {
            return Err(format!("{flag} must not be empty"));
        }
        return Ok(Some((flag.to_string(), v.trim().to_string())));
    }
    Ok(env_non_empty(env, var)?.map(|v| (var.to_string(), v)))
}

/// `Ok(None)` when unset; error when set but empty/whitespace.
fn env_non_empty(
    env: &dyn Fn(&str) -> Option<String>,
    var: &str,
) -> Result<Option<String>, String> {
    match env(var) {
        None => Ok(None),
        Some(v) if v.trim().is_empty() => Err(format!("{var} is set but empty")),
        Some(v) => Ok(Some(v.trim().to_string())),
    }
}

fn parse_bool(var: &str, v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("{var} must be one of true/false/1/0, got `{v}`")),
    }
}

/// Parses `value` and checks the scheme. The error names the setting and the *redacted* value.
fn validate_url(setting: &str, value: &str) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(value).map_err(|_| {
        format!(
            "{setting}: not a valid URL: `{}`",
            redact_url_for_log(value)
        )
    })?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "{setting}: unsupported URL scheme `{other}` (expected http or https)"
            ))
        }
    }
    if parsed.host_str().is_none() {
        return Err(format!(
            "{setting}: URL has no host: `{}`",
            redact_url_for_log(value)
        ));
    }
    Ok(parsed)
}

/// Joins `/v1/messages` onto a base URL.
///
/// - trailing `/` trimmed; `…/v1` → `…/v1/messages`; otherwise `…/v1/messages` appended
/// - a path prefix (`https://gw/anthropic`) is preserved
/// - a base already ending in `/v1/messages` is an error pointing at the full-endpoint setting
/// - a query or fragment on a base is an error (use the full-endpoint setting)
fn join_base(setting: &str, value: &str) -> Result<String, String> {
    let mut parsed = validate_url(setting, value)?;
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "{setting}: a base URL must not carry a query or fragment; \
             use {ENV_ENDPOINT} / {FLAG_ENDPOINT} for a full URL"
        ));
    }
    let path = parsed.path().trim_end_matches('/').to_string();
    if path.ends_with("/v1/messages") {
        return Err(format!(
            "{setting}: `{}` already ends in /v1/messages; it is a full endpoint — \
             use {ENV_ENDPOINT} / {FLAG_ENDPOINT} instead",
            redact_url_for_log(value)
        ));
    }
    let new_path = if path.ends_with("/v1") {
        format!("{path}/messages")
    } else {
        format!("{path}/v1/messages")
    };
    parsed.set_path(&new_path);
    Ok(parsed.to_string())
}

/// Redacts a URL for logs: strips userinfo, replaces any query with `?<redacted>` and drops the
/// fragment. An unparseable value is replaced wholesale (it could be anything, secrets included).
pub fn redact_url_for_log(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return "<unparseable-url>".to_string();
    };
    let had_query = parsed.query().is_some();
    let _ = parsed.set_username("");
    let _ = parsed.set_password(None);
    parsed.set_query(None);
    parsed.set_fragment(None);
    let mut s = parsed.to_string();
    if had_query {
        s.push_str("?<redacted>");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| m.get(k).cloned()
    }

    fn resolve(
        pairs: &[(&str, &str)],
        cli: AnthropicCliArgs,
    ) -> Result<AnthropicEndpointConfig, String> {
        let e = env_of(pairs);
        AnthropicEndpointConfig::resolve(&e, &cli)
    }

    #[test]
    fn default_config_is_stock() {
        let c = resolve(&[("ANTHROPIC_API_KEY", "k")], AnthropicCliArgs::default()).unwrap();
        assert_eq!(c.url, DEFAULT_ANTHROPIC_URL);
        assert!(c.is_default());
        assert_eq!(c.auth_scheme(), "x-api-key");
        assert_eq!(c.version, "2023-06-01");
        assert_eq!(c.version_in, VersionPlacement::Header);
        assert!(c.prompt_caching && !c.omit_model);
        assert!(c.notes.is_empty());
    }

    #[test]
    fn base_url_appends_messages_path() {
        let c = resolve(
            &[
                ("ANTHROPIC_BASE_URL", "https://gw.example.com"),
                ("ANTHROPIC_API_KEY", "k"),
            ],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.url, "https://gw.example.com/v1/messages");
        assert!(!c.is_default());
    }

    #[test]
    fn base_url_trailing_slash_not_doubled() {
        let c = resolve(
            &[("ANTHROPIC_BASE_URL", "https://gw.example.com/")],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.url, "https://gw.example.com/v1/messages");
        let c = resolve(
            &[("ANTHROPIC_BASE_URL", "https://gw.example.com/anthropic/")],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.url, "https://gw.example.com/anthropic/v1/messages");
    }

    #[test]
    fn base_url_ending_v1_not_doubled() {
        for b in ["https://gw/v1", "https://gw/v1/", "https://gw/api/v1"] {
            let c = resolve(&[("ANTHROPIC_BASE_URL", b)], AnthropicCliArgs::default()).unwrap();
            assert!(c.url.ends_with("/v1/messages"), "{}", c.url);
            assert!(!c.url.contains("/v1/v1"), "{}", c.url);
        }
    }

    #[test]
    fn base_url_ending_v1_messages_is_error_pointing_at_endpoint() {
        let e = resolve(
            &[("ANTHROPIC_BASE_URL", "https://gw/v1/messages")],
            AnthropicCliArgs::default(),
        )
        .unwrap_err();
        assert!(e.contains("ANTHROPIC_BASE_URL"), "{e}");
        assert!(e.contains("LCG_ANTHROPIC_ENDPOINT"), "{e}");
    }

    #[test]
    fn base_with_query_or_fragment_is_error() {
        let e = resolve(
            &[("ANTHROPIC_BASE_URL", "https://gw/?k=secret")],
            AnthropicCliArgs::default(),
        )
        .unwrap_err();
        assert!(e.contains("ANTHROPIC_BASE_URL"), "{e}");
        assert!(!e.contains("secret"), "{e}");
        assert!(resolve(
            &[("ANTHROPIC_BASE_URL", "https://gw/#frag")],
            AnthropicCliArgs::default()
        )
        .is_err());
    }

    #[test]
    fn flag_beats_env_for_base() {
        let cli = AnthropicCliArgs {
            base_url: Some("https://flag.example".into()),
            endpoint: None,
        };
        let c = resolve(&[("ANTHROPIC_BASE_URL", "https://env.example")], cli).unwrap();
        assert_eq!(c.url, "https://flag.example/v1/messages");
    }

    #[test]
    fn endpoint_is_verbatim_and_beats_base() {
        let c = resolve(
            &[
                ("LCG_ANTHROPIC_ENDPOINT", "https://gw/dep/v1/invoke"),
                ("ANTHROPIC_BASE_URL", "https://other"),
            ],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.url, "https://gw/dep/v1/invoke");
        assert!(c
            .notes
            .iter()
            .any(|n| n.contains("ANTHROPIC_BASE_URL ignored")));
    }

    #[test]
    fn endpoint_flag_beats_endpoint_env_and_base_flag() {
        let cli = AnthropicCliArgs {
            base_url: Some("https://b".into()),
            endpoint: Some("https://flag/e".into()),
        };
        let c = resolve(&[("LCG_ANTHROPIC_ENDPOINT", "https://env/e")], cli).unwrap();
        assert_eq!(c.url, "https://flag/e");
    }

    #[test]
    fn endpoint_may_carry_query() {
        let c = resolve(
            &[("LCG_ANTHROPIC_ENDPOINT", "https://gw/x?api-version=1")],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.url, "https://gw/x?api-version=1");
    }

    #[test]
    fn invalid_urls_name_the_setting() {
        let e = resolve(
            &[("ANTHROPIC_BASE_URL", "not a url")],
            AnthropicCliArgs::default(),
        )
        .unwrap_err();
        assert!(e.contains("ANTHROPIC_BASE_URL"), "{e}");
        let e = resolve(
            &[("LCG_ANTHROPIC_ENDPOINT", "ftp://gw/x")],
            AnthropicCliArgs::default(),
        )
        .unwrap_err();
        assert!(
            e.contains("LCG_ANTHROPIC_ENDPOINT") && e.contains("ftp"),
            "{e}"
        );
        let cli = AnthropicCliArgs {
            base_url: Some("file:///etc".into()),
            endpoint: None,
        };
        let e = resolve(&[], cli).unwrap_err();
        assert!(e.contains("--anthropic-base-url"), "{e}");
    }

    #[test]
    fn empty_but_set_values_are_errors() {
        for var in [
            "ANTHROPIC_BASE_URL",
            "LCG_ANTHROPIC_ENDPOINT",
            "ANTHROPIC_AUTH_TOKEN",
            "LCG_ANTHROPIC_OMIT_MODEL",
            "LCG_ANTHROPIC_VERSION",
            "LCG_ANTHROPIC_VERSION_IN",
            "LCG_ANTHROPIC_PROMPT_CACHING",
        ] {
            let e = resolve(&[(var, "  ")], AnthropicCliArgs::default()).unwrap_err();
            assert!(e.contains(var), "{var}: {e}");
        }
        let cli = AnthropicCliArgs {
            base_url: Some(String::new()),
            endpoint: None,
        };
        assert!(resolve(&[], cli)
            .unwrap_err()
            .contains("--anthropic-base-url"));
        let cli = AnthropicCliArgs {
            base_url: None,
            endpoint: Some(String::new()),
        };
        assert!(resolve(&[], cli)
            .unwrap_err()
            .contains("--anthropic-endpoint"));
    }

    #[test]
    fn empty_api_key_keeps_historical_semantics() {
        let c = resolve(&[("ANTHROPIC_API_KEY", "")], AnthropicCliArgs::default()).unwrap();
        assert_eq!(c.auth, AnthropicAuth::ApiKey(String::new()));
    }

    #[test]
    fn bearer_beats_api_key() {
        let c = resolve(
            &[
                ("ANTHROPIC_AUTH_TOKEN", "tok"),
                ("ANTHROPIC_API_KEY", "key"),
            ],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.auth, AnthropicAuth::Bearer("tok".into()));
        assert_eq!(c.auth_scheme(), "bearer");
        assert!(c
            .notes
            .iter()
            .any(|n| n.contains("ANTHROPIC_API_KEY ignored")));
    }

    #[test]
    fn bearer_alone_resolves() {
        let c = resolve(
            &[("ANTHROPIC_AUTH_TOKEN", "tok")],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.auth, AnthropicAuth::Bearer("tok".into()));
        assert!(c.notes.is_empty());
    }

    #[test]
    fn bedrock_shape() {
        let c = resolve(
            &[
                ("LCG_ANTHROPIC_ENDPOINT", "https://gw/dep/v1/invoke"),
                ("ANTHROPIC_AUTH_TOKEN", "tok"),
                ("LCG_ANTHROPIC_OMIT_MODEL", "true"),
                ("LCG_ANTHROPIC_VERSION", "bedrock-2023-05-31"),
                ("LCG_ANTHROPIC_VERSION_IN", "body"),
                ("LCG_ANTHROPIC_PROMPT_CACHING", "off"),
            ],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert!(c.omit_model);
        assert_eq!(c.version, "bedrock-2023-05-31");
        assert_eq!(c.version_in, VersionPlacement::Body);
        assert!(!c.prompt_caching);
        let s = c.startup_summary();
        assert!(s.contains("endpoint=https://gw/dep/v1/invoke"), "{s}");
        assert!(s.contains("auth=bearer"), "{s}");
        assert!(s.contains("model_in_body=false"), "{s}");
        assert!(s.contains("anthropic_version_in=body"), "{s}");
        assert!(s.contains("prompt_caching=off"), "{s}");
    }

    #[test]
    fn body_placement_requires_explicit_version() {
        let e = resolve(
            &[("LCG_ANTHROPIC_VERSION_IN", "body")],
            AnthropicCliArgs::default(),
        )
        .unwrap_err();
        assert!(e.contains("LCG_ANTHROPIC_VERSION"), "{e}");
    }

    #[test]
    fn unsupported_enum_values_are_errors() {
        assert!(resolve(
            &[("LCG_ANTHROPIC_VERSION_IN", "query")],
            AnthropicCliArgs::default()
        )
        .unwrap_err()
        .contains("LCG_ANTHROPIC_VERSION_IN"));
        assert!(resolve(
            &[("LCG_ANTHROPIC_OMIT_MODEL", "maybe")],
            AnthropicCliArgs::default()
        )
        .unwrap_err()
        .contains("LCG_ANTHROPIC_OMIT_MODEL"));
        assert!(resolve(
            &[("LCG_ANTHROPIC_PROMPT_CACHING", "sometimes")],
            AnthropicCliArgs::default()
        )
        .unwrap_err()
        .contains("LCG_ANTHROPIC_PROMPT_CACHING"));
    }

    #[test]
    fn version_header_override_allowed() {
        let c = resolve(
            &[("LCG_ANTHROPIC_VERSION", "2024-01-01")],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        assert_eq!(c.version, "2024-01-01");
        assert_eq!(c.version_in, VersionPlacement::Header);
    }

    #[test]
    fn debug_never_prints_credentials() {
        let c = resolve(
            &[
                ("ANTHROPIC_AUTH_TOKEN", "SECRET-TOKEN"),
                (
                    "LCG_ANTHROPIC_ENDPOINT",
                    "https://u:SECRET-PW@gw/x?key=SECRET-Q",
                ),
            ],
            AnthropicCliArgs::default(),
        )
        .unwrap();
        let d = format!("{c:?}");
        assert!(!d.contains("SECRET"), "{d}");
        let k = AnthropicEndpointConfig::default_with_key("SECRET-KEY".into());
        assert!(!format!("{k:?}").contains("SECRET"));
        assert!(!k.startup_summary().contains("SECRET"));
    }

    #[test]
    fn redact_strips_userinfo_query_fragment() {
        let r = redact_url_for_log("https://user:pw@gw.example.com:8443/a/b?token=abc#frag");
        assert_eq!(r, "https://gw.example.com:8443/a/b?<redacted>");
        assert_eq!(redact_url_for_log("https://gw/x"), "https://gw/x");
        assert_eq!(
            redact_url_for_log("garbage?key=SECRET"),
            "<unparseable-url>"
        );
    }

    #[test]
    fn invalid_url_error_does_not_leak_query() {
        let e = resolve(
            &[("LCG_ANTHROPIC_ENDPOINT", "ftp://gw/x?key=SECRET")],
            AnthropicCliArgs::default(),
        )
        .unwrap_err();
        assert!(!e.contains("SECRET"), "{e}");
    }
}
