# ADR-0346: Configurable Endpoint and Auth for the Anthropic Extraction Provider

**Date**: 2026-11-15
**Status**: Accepted

## Context

The Anthropic extraction provider posted to a hardcoded `https://api.anthropic.com/v1/messages`
with `x-api-key` auth and an `anthropic-version` header. Deployments that cannot reach
`api.anthropic.com` (they reach Claude only through AWS Bedrock) or that centralize LLM access in
a proxy (OpenRouter, LiteLLM, a corporate gateway) could not run extraction at all. The existing
`--extractor-http`/`--extractor-uds` path is a different provider — OpenAI chat-completions
shaped, local, unauthenticated by default (ADR-0041) — and is not a drop-in. See issue #346.

The motivating target is a Bedrock gateway fronting Claude, authenticated by an OAuth2-minted
Bearer token, with a full `<base>/<deployment>/v1/invoke` URL (model chosen by path), no
top-level `model`, and `anthropic_version` carried in the **body**. The response is the standard
Messages shape.

Before this change the HTTP request was built in five places in `extractor.rs`
(`send_with_retry` and the four hand-rolled retry loops for summary consolidation, entity
classification, relation classification and dedup judgement), each duplicating the header block.

## Decision

### One resolved, immutable config; a pure resolver

`crates/core/src/anthropic_config.rs` defines `AnthropicEndpointConfig` (URL, auth scheme +
credential, omit-model flag, `anthropic_version` value and placement, prompt-caching flag).
`resolve(env, cli)` is pure — the environment is an injected closure — so unit tests never mutate
process env. The service resolves it once, and `LlmRouter::from_config_with` hands the same value
to the primary and fallback leaves (per-call endpoints are out of scope). `AnthropicExtractor`
keeps its public constructors and gains `with_endpoint_config` (the ADR-0497 builder precedent),
so existing call sites and `lcg-eval` are unchanged.

### One request builder for every call path

`AnthropicExtractor::post()` decides URL, auth header, version header and beta header;
`shape_body()` drops `model` and/or inserts `anthropic_version`. All five send sites use them, so
a gateway shape cannot apply to extraction but not to dedup judgement (FR-009). Each site keeps
its own retry and error handling.

### Setting names and precedence

| Setting | Env | Flag |
|---|---|---|
| Base URL (`/v1/messages` appended) | `ANTHROPIC_BASE_URL` | `--anthropic-base-url` |
| Full endpoint, verbatim | `LCG_ANTHROPIC_ENDPOINT` | `--anthropic-endpoint` |
| Bearer token | `ANTHROPIC_AUTH_TOKEN` | none |
| Omit `model` | `LCG_ANTHROPIC_OMIT_MODEL` | none |
| `anthropic_version` value / placement | `LCG_ANTHROPIC_VERSION` / `LCG_ANTHROPIC_VERSION_IN` | none |
| Prompt caching | `LCG_ANTHROPIC_PROMPT_CACHING` | none |

URL precedence: endpoint flag > endpoint env > base flag > base env > stock. A full endpoint always
beats a base. `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN` follow the official SDK conventions.
Only URL-bearing settings get flags (matching `--embedder-http`/`--extractor-http`); the token
has none because argv is visible in process listings. `LCG_EXTRACTION_URL` is never reinterpreted
— it belongs to the OpenAI-shaped provider (FR-016).

Base joining: trailing `/` trimmed; a base ending `/v1` is not doubled; a path prefix is kept; a
base ending `/v1/messages`, or carrying a query/fragment, is an error pointing at the full-endpoint
setting. A full endpoint may carry a query (some gateways need `?api-version=`).

### Auth

`ANTHROPIC_AUTH_TOKEN` is sent as `Authorization: Bearer` with no `x-api-key`, and wins over
`ANTHROPIC_API_KEY` when a gateway URL is configured; on the stock endpoint with an API key set the
key is kept (FR-003: unchanged behaviour) and the token is ignored with a startup note, because the
token is often exported for other Anthropic tooling. Exactly one scheme is sent. Either selects the Anthropic provider
(FR-006) — `ANTHROPIC_BASE_URL` alone does not. `ANTHROPIC_API_KEY` keeps its historical
"set, even if empty, selects Anthropic" semantics.

### Strict, but lazy, validation

Every new variable is strictly parsed: a set-but-empty value, unparseable URL, non-`http(s)`
scheme or unsupported placement value fails startup with an error naming the setting. But per
ADR-0331 validation runs only when the Anthropic provider is actually selected: a stale
`ANTHROPIC_BASE_URL` inherited from a shell must not break an explicit `--extractor-http` run. In
that case one log line names the ignored settings (never their values). `LCG_ANTHROPIC_VERSION_IN=body`
**requires** an explicit `LCG_ANTHROPIC_VERSION`: silently sending `2023-06-01` to a Bedrock
gateway would fail confusingly at runtime. The new flags are rejected with `--connect`, mirroring
`--writable-groups` (the daemon does the extracting).

### Prompt caching: one switch disables both halves

`LCG_ANTHROPIC_PROMPT_CACHING=off` suppresses the `anthropic-beta` header **and** the `cache_control`
system-prompt blocks. Suppressing only the header would leave a payload strict gateways may still
reject. `is_sonnet()` still reads the configured model, so model-specific behaviour survives
`LCG_ANTHROPIC_OMIT_MODEL`.

### Credentials never leak

- `AnthropicAuth` and `AnthropicEndpointConfig` implement `Debug` by hand; the credential is never
  printed.
- The startup log reports the endpoint with userinfo stripped, any query shown as `?<redacted>` and
  the fragment dropped, plus the auth *scheme* (`x-api-key`/`bearer`). This is deliberately a
  separate helper from `embedder::redact_url_userinfo`, whose ADR-0497 scope is Basic-auth userinfo
  only.
- `reqwest::Error`'s `Display` embeds the request URL, which may carry query secrets; all five
  send sites convert errors with `.without_url()`.
- Invalid-URL errors name the setting and the *redacted* value.
- `ANTHROPIC_AUTH_TOKEN` alone does not displace a configured `LCG_EXTRACTION_URL`: with no
  `ANTHROPIC_API_KEY` and no gateway URL setting, the local extractor stays selected and a warning
  is logged. The token is often exported for unrelated Anthropic tooling and MCP stdio servers
  inherit it, so honouring it would silently send it to `api.anthropic.com` after an upgrade.
  (`ANTHROPIC_API_KEY` keeps its pre-existing precedence over `LCG_EXTRACTION_URL`.)
- A non-default endpoint (URL other than the stock one) logs a notice that the credential is sent
  to an operator-chosen URL. This
  reverses ADR-0497's premise that `ANTHROPIC_API_KEY` "only ever talks to a fixed Anthropic
  endpoint": it no longer does, so the same visible-notice reasoning now applies.

## Consequences

- Anthropic-compatible gateways (OpenRouter, LiteLLM) and a Bedrock gateway with Bearer auth,
  path-encoded model and body-placed version are configuration-only; no translating sidecar.
- With no new setting present, URL, headers and body are byte-for-byte unchanged (pinned by
  `crates/core/tests/anthropic_gateway.rs`). Replay cassettes are unaffected: keys hash request
  content, not URL/auth, and replay bypasses provider resolution.
- **Known limitation — token expiry.** lcg reads the Bearer token once. An expired short-lived
  token surfaces as auth failures; with a `primary:fallback` model pair `LlmRouter` treats the first
  as a primary failure and permanently fails over to the fallback, which uses the same expired
  token. Documented, not fixed; supported patterns are long-lived tokens or an external refresher
  that restarts the process.
- A built-in OAuth2 client-credentials fetcher is a follow-up; `ANTHROPIC_AUTH_TOKEN` is the seam it
  plugs into. Native Bedrock (SigV4, streaming) and per-call endpoints remain out of scope. The
  `lcg-eval` judge keeps its own fixed endpoint.
- An `ANTHROPIC_BASE_URL` exported for other Anthropic tooling now redirects lcg too (standard SDK
  semantics); the startup log names the effective endpoint so this is visible.
