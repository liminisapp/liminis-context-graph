# Feature Specification: Configurable endpoint and auth for the Anthropic extraction provider (Bedrock / OpenRouter / LiteLLM gateways)

**Feature Branch**: `fabrik/issue-346`
**Created**: 2026-11-15
**Status**: Specified
**Input**: User description: "## Problem

The Anthropic extraction provider is hardcoded to `https://api.anthropic.com/v1/messages`. There's no way to point it at an Anthropic-compatible **gateway** — e.g. an AWS Bedrock access gateway, OpenRouter, or LiteLLM — which is required in environments that (a) can't reach `api.anthropic.com` directly and must go through Bedrock, or (b) centralize LLM access/billing/routing through a proxy.

Verified against **v0.11.0** that none of the common env overrides are honored — the startup log still shows `extractor: provider=anthropic, transport=http, endpoint=https://api.anthropic.com/v1/messages`:

- `ANTHROPIC_BASE_URL` — ignored
- `ANTHROPIC_API_URL` — ignored
- `LCG_EXTRACTION_URL` — ignored

The only redirect available is `--extractor-http` / `--extractor-uds`, but that path is an **OpenAI `/v1/chat/completions`**-shaped, local, unauthenticated sidecar — not a drop-in for an authenticated remote **Anthropic Messages** gateway.

## Request

Let the Anthropic provider's base URL be configurable, while keeping the Anthropic Messages request/response shape and `ANTHROPIC_API_KEY` auth. Either/both:

1. **Honor `ANTHROPIC_BASE_URL`** (matches the official Anthropic SDKs), and
2. a `--anthropic-base-url <URL>` flag (parallels the existing `--embedder-http` / `--extractor-http` flags).

The provided URL should be used verbatim for `POST .../v1/messages` (or configurable path), sending the existing `x-api-key` / `anthropic-version` headers, so an Anthropic-compatible gateway (Bedrock access gateway, OpenRouter's Anthropic-compatible endpoint, LiteLLM in `anthropic` mode) is a straight swap.

### Nice-to-have
- Allow an override header/token env for gateways that authenticate differently from `x-api-key` (e.g. `Authorization: Bearer …`).

## Context / use case

Deploying the orac/tarial ingestion container (which drives lcg as the write-side extractor) to AWS ECS in an environment that reaches Anthropic **only via Bedrock**. Without a base-URL override, extraction can't run there. Happy to help test a build."

## Background

The Anthropic extraction provider always posts to `https://api.anthropic.com/v1/messages` with `x-api-key` auth and an `anthropic-version` header. Deployments that cannot reach `api.anthropic.com` directly — because they must go through AWS Bedrock, or because LLM access, billing and routing are centralized in a proxy (OpenRouter, LiteLLM, a corporate gateway) — therefore cannot run extraction at all. The motivating case is the orac/tarial ingestion container, deployed on AWS ECS, which reaches Claude only through a Bedrock gateway.

Today none of the conventional overrides (`ANTHROPIC_BASE_URL`, `ANTHROPIC_API_URL`, `LCG_EXTRACTION_URL`) change the endpoint of the Anthropic provider. The existing `--extractor-uds` / `--extractor-http` (and `LCG_EXTRACTION_URL` / `LCG_EXTRACTION_API_KEY`) path is a different provider: it speaks the OpenAI chat-completions shape to a local OpenAI-compatible server, so it is not a drop-in for a remote Anthropic-Messages gateway.

A follow-up comment from the requester describes the concrete target — a **Bedrock gateway fronting Claude, authenticated via OAuth2** — which needs more than a base-URL swap:

1. **Bearer auth.** The gateway wants `Authorization: Bearer <token>` (the token coming from an OAuth2 client-credentials exchange), not `x-api-key`.
2. **Full invoke URL with the model in the path.** The endpoint looks like `<base>/<deployment>/v1/invoke`; the model is selected by the URL path, not by a body field, so the full URL must be configurable and the top-level `model` must be omittable.
3. **Bedrock-flavored body.** Same Messages schema, but `"anthropic_version": "bedrock-2023-05-31"` is sent in the **body** (not as the `anthropic-version` header), with no top-level `model`.
4. **Standard response.** The response is the standard Anthropic Messages shape (`content[0].text`) — no change needed.

The requester's minimal design: a configurable full endpoint URL, an injectable `Authorization` header, optional `model`, and a configurable `anthropic_version` (value and header-vs-body placement); an optional built-in OAuth2 client-credentials fetcher is "nicest" but not required. An interim workaround exists (a local OpenAI-compatible sidecar that translates to the gateway), which native support is meant to replace.

Prior art: the official Anthropic SDKs honor `ANTHROPIC_BASE_URL` (a base to which `/v1/messages` is appended) and `ANTHROPIC_AUTH_TOKEN` (sent as `Authorization: Bearer`); LiteLLM and OpenRouter expose Anthropic-compatible endpoints under the same convention; Bedrock's Anthropic invoke API places `anthropic_version` in the body and the model in the URL.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Point the Anthropic provider at a gateway base URL (Priority: P1)

An operator running lcg where `api.anthropic.com` is unreachable, or where traffic must be routed through a proxy (OpenRouter, LiteLLM in Anthropic mode, a corporate gateway), sets a base URL via the standard `ANTHROPIC_BASE_URL` environment variable or a `--anthropic-base-url` flag. Extraction then sends Anthropic Messages requests to that gateway, with the existing `x-api-key` auth and headers, and nothing else about extraction changes.

**Why this priority**: This is the core ask and the minimum viable slice; it unblocks every gateway that is a straight Anthropic-compatible proxy.

**Independent Test**: Start a local stub that speaks the Anthropic Messages shape, set `ANTHROPIC_BASE_URL` (or the flag) to it with `ANTHROPIC_API_KEY` set, run an extraction, and confirm the stub received `POST <base>/v1/messages` with the `x-api-key` and `anthropic-version` headers and that extraction results are produced from its response.

**Acceptance Scenarios**:

1. **Given** `ANTHROPIC_API_KEY` is set and `ANTHROPIC_BASE_URL=https://gw.example.com`, **When** an episode is extracted, **Then** the request goes to `https://gw.example.com/v1/messages` with `x-api-key` and `anthropic-version` headers, and never to `api.anthropic.com`.
2. **Given** both `--anthropic-base-url` and `ANTHROPIC_BASE_URL` are set to different values, **When** the service starts, **Then** the flag value is used.
3. **Given** neither is set, **When** the service starts, **Then** behavior is identical to today (`https://api.anthropic.com/v1/messages`).
4. **Given** a base URL with a trailing slash (`https://gw.example.com/`), **When** a request is built, **Then** the resulting URL has no doubled slash.

---

### User Story 2 - Authenticate to a gateway with a Bearer credential (Priority: P1)

An operator whose gateway authenticates with `Authorization: Bearer <token>` (not `x-api-key`) supplies the credential through configuration. lcg sends the `Authorization` header instead of `x-api-key` on every Anthropic request. The token is supplied by the operator (for example, minted by an external OAuth2 client-credentials step and injected into the environment); lcg does not mint it.

**Why this priority**: Without it, Bedrock gateways and many proxies cannot be used even with a custom URL; it is the "nice-to-have" in the original request and requirement 1 of the follow-up.

**Independent Test**: Against a stub that rejects requests lacking `Authorization: Bearer <token>`, configure the bearer credential with no `ANTHROPIC_API_KEY`, run an extraction, and confirm it succeeds and that no `x-api-key` header was sent.

**Acceptance Scenarios**:

1. **Given** a bearer credential is configured, **When** a request is sent, **Then** it carries `Authorization: Bearer <credential>` and does not carry `x-api-key`.
2. **Given** only a bearer credential is configured (no `ANTHROPIC_API_KEY`), **When** the service starts, **Then** the Anthropic provider is selected (not "no extraction provider configured") and extraction works.
3. **Given** both a bearer credential and `ANTHROPIC_API_KEY` are set, **When** a request is sent, **Then** the bearer credential takes precedence and exactly one auth scheme is sent.
4. **Given** the credential value, **When** the service logs its startup configuration or any error, **Then** the value never appears in any log or telemetry output.

---

### User Story 3 - Use a full invoke URL with the model in the path and a Bedrock-style body (Priority: P2)

An operator whose gateway exposes a per-deployment invoke URL (`<base>/<deployment>/v1/invoke`) configures that full URL, which lcg uses verbatim. They also configure the request to omit the top-level `model` field and to carry `anthropic_version` in the body (e.g. `bedrock-2023-05-31`) instead of as a header. Extraction works against the Bedrock gateway end to end, with the standard Anthropic response shape parsed as today.

**Why this priority**: Required for the real Bedrock-with-OAuth2 target, but builds on Stories 1–2 and is only needed for gateways that diverge from the plain Messages request shape.

**Independent Test**: Against a stub that requires `POST /dep/v1/invoke`, no `model` in the body, and `anthropic_version: "bedrock-2023-05-31"` in the body, run an extraction and confirm the stub's expectations are all met and results are produced.

**Acceptance Scenarios**:

1. **Given** a full endpoint URL is configured, **When** a request is sent, **Then** it is posted to exactly that URL with no path appended.
2. **Given** model omission is configured, **When** a request body is built, **Then** it contains no top-level `model` key; otherwise it contains `model` as today.
3. **Given** `anthropic_version` placement is set to "body" with a value, **When** a request is built, **Then** the body carries `anthropic_version: <value>` and no `anthropic-version` header is sent; **Given** "header" placement (the default), **Then** behavior matches today (`anthropic-version: 2023-06-01`).
4. **Given** all these options, **When** any Anthropic-backed call is made (extraction, entity/relation reclassification, summary consolidation, LLM dedup judgement), **Then** each uses the same configured URL, auth and body shape — none falls back to the hardcoded endpoint.

---

### User Story 4 - Operators can see and validate what is configured (Priority: P2)

An operator can confirm from the startup log which endpoint and auth scheme the Anthropic provider is using, and gets a clear startup error for invalid configuration rather than a confusing runtime failure.

**Why this priority**: Misconfigured gateways are the main failure mode of this feature; clear diagnostics are what makes it usable in ECS-style environments.

**Independent Test**: Start with a valid gateway config and check the log line; start with an invalid URL and check for a startup error naming the offending setting.

**Acceptance Scenarios**:

1. **Given** a gateway URL is configured, **When** the service starts, **Then** the existing `extractor: provider=anthropic, transport=http, endpoint=…` log line shows the effective endpoint (with any credentials/userinfo/query secrets redacted) and states which auth scheme is in use.
2. **Given** an unparseable URL or non-`http(s)` scheme, **When** the service starts, **Then** it fails with an error naming the setting and the value rather than silently falling back to the default endpoint.
3. **Given** a bearer credential or base URL is set but the setting that selects the extraction provider conflicts (e.g. an explicit `--extractor-uds`/`--extractor-http` is also passed), **When** the service starts, **Then** the existing precedence rules apply and the log makes clear the Anthropic gateway settings were not used.

---

### Edge Cases

- Base URL that already ends in `/v1` or `/v1/messages`: documented behavior is deterministic (see Assumptions) and never silently produces a doubled path.
- Empty-string values for any new variable are treated as a configuration error or as unset consistently with the project's other strictly-parsed variables, never as a valid value.
- Gateway returns 429/529: existing retry/backoff behavior is unchanged against a gateway.
- Gateway returns a non-2xx with a non-Anthropic error body: the raw body is still captured in extraction-failure telemetry as today.
- Short-lived bearer tokens expire during a long run: since lcg does not refresh tokens, expiry surfaces as an auth failure; the token is re-read on each request so an external refresher that updates the process environment/file source is honored if the supported source allows it (see Assumptions).
- `prompt-caching` beta header (`anthropic-beta`) is sent today for Sonnet models; against a gateway that rejects unknown headers this must be suppressible or documented.
- `LCG_EXTRACTION_URL` is a different provider's URL (OpenAI-shaped) and must not be reinterpreted as the Anthropic endpoint.
- Record/replay cassettes (`LCG_RECORD_LLM` / `LCG_REPLAY_LLM`) keep working: recording against a gateway works, replay needs no gateway or credentials.
- `LCG_EXTRACTION_LLM` model name is still used for non-omitted `model`, and for model-specific behavior (e.g. Sonnet detection) even when `model` is omitted from the body.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The Anthropic extraction provider MUST honor the `ANTHROPIC_BASE_URL` environment variable as its base URL, appending the Messages path (`/v1/messages`) to it, matching the official Anthropic SDK convention.
- **FR-002**: The service MUST accept an `--anthropic-base-url <URL>` command-line flag with the same meaning as `ANTHROPIC_BASE_URL`; when both are set the flag wins.
- **FR-003**: When no base URL, full endpoint URL, or related setting is configured, the provider's behavior (URL, headers, body) MUST be byte-for-byte equivalent to current behavior.
- **FR-004**: The provider MUST support a full endpoint URL setting (env var and flag) that is used verbatim as the POST target with no path appended, taking precedence over the base URL when both are set.
- **FR-005**: The provider MUST support a Bearer credential setting (env var) that, when set, causes requests to carry `Authorization: Bearer <credential>` and omit `x-api-key`; it takes precedence over `ANTHROPIC_API_KEY`.
- **FR-006**: The Anthropic provider MUST be selected for extraction when either `ANTHROPIC_API_KEY` or the Bearer credential is set (subject to the existing explicit-flag precedence), so a gateway deployment needs no dummy `ANTHROPIC_API_KEY`.
- **FR-007**: The provider MUST support omitting the top-level `model` field from the request body via a configuration setting; when not set, `model` is sent as today.
- **FR-008**: The provider MUST support configuring the `anthropic_version` value and whether it is sent as the `anthropic-version` header (default, `2023-06-01`) or as an `anthropic_version` field in the request body; exactly one placement is used per request.
- **FR-009**: Every Anthropic-backed call path (entity/edge extraction, entity and relation reclassification, summary consolidation, LLM dedup judgement) MUST use the same configured URL, auth and request-shape settings.
- **FR-010**: Credential values (API key, Bearer token) MUST never be written to logs, error messages or telemetry; URLs logged at startup MUST have any userinfo and query-string secrets redacted.
- **FR-011**: The startup log MUST report the effective Anthropic endpoint and the auth scheme in use (`x-api-key` or `bearer`, never the value).
- **FR-012**: Invalid configuration (unparseable URL, non-`http`/`https` scheme, empty-but-set value, unsupported `anthropic_version` placement value) MUST fail startup with an error naming the setting; it MUST NOT silently fall back to the default endpoint.
- **FR-013**: The existing request/response handling — Messages response parsing, retry/backoff on 429/529, failure telemetry, timeouts, cassette record/replay — MUST work unchanged against a configured gateway.
- **FR-014**: The `anthropic-beta` prompt-caching header MUST be suppressible by configuration, or the documentation MUST state precisely when it is sent, so gateways that reject it can be used.
- **FR-015**: The new flags and environment variables MUST be documented in the configuration reference and CLI help, including a worked example for a Bedrock-style gateway (full URL, Bearer auth, no `model`, `anthropic_version` in body) and one for a plain proxy (OpenRouter/LiteLLM).
- **FR-016**: A flag or env var for the Anthropic gateway MUST NOT alter the behavior of the separate OpenAI-compatible provider (`--extractor-http`, `--extractor-uds`, `LCG_EXTRACTION_URL`, `LCG_EXTRACTION_API_KEY`).

### Key Entities

- **Anthropic endpoint configuration**: The resolved set of target URL (base or full), auth scheme and credential, model-in-body on/off, and `anthropic_version` value and placement, shared by every Anthropic-backed call.
- **Bearer credential**: An operator-supplied, opaque token sent as `Authorization: Bearer`; lcg neither mints nor refreshes it.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: With only `ANTHROPIC_BASE_URL` and `ANTHROPIC_API_KEY` set against an Anthropic-compatible stub, an extraction completes with zero requests to `api.anthropic.com`.
- **SC-002**: With full-URL, Bearer credential, model-omission and body-placed `anthropic_version` configured against a stub modelling the Bedrock gateway contract, an extraction completes end to end and the stub observes exactly the expected URL, headers and body.
- **SC-003**: With no new settings, the recorded request (URL, headers, body) is identical to that of the previous release, verified by an automated test.
- **SC-004**: A secret-scanning check over startup logs, error output and telemetry from runs using a Bearer credential and an API key finds neither value.
- **SC-005**: Each documented invalid configuration produces a startup error naming the offending setting in 100% of tested cases.
- **SC-006**: The requester (who has a working Bedrock/OAuth2 reference client) can run extraction against their Bedrock gateway using only lcg configuration, with no translating sidecar.

## Assumptions

- `ANTHROPIC_BASE_URL` is a *base* (the Messages path is appended), as in the Anthropic SDKs; the issue's "verbatim" requirement is met by the separate full-endpoint setting. A base that already ends in `/v1` is handled without doubling the segment; one ending in `/v1/messages` is treated as a configuration error pointing the operator to the full-endpoint setting.
- Exact names for the new settings beyond `ANTHROPIC_BASE_URL` and `--anthropic-base-url` (full endpoint, Bearer credential, model omission, `anthropic_version` value/placement, beta-header suppression) are chosen in Plan, following the project's `LCG_*` naming and the SDK-conventional `ANTHROPIC_AUTH_TOKEN` for the Bearer credential where it fits.
- The Bearer credential is read from the environment (as other credentials are); it is not re-read mid-run unless Plan finds a cheap, safe way to support external refresh, and long-lived tokens or an external refresher that restarts the process are the supported pattern.
- Existing `--extractor-uds`/`--extractor-http` precedence over the Anthropic provider is unchanged.
- The model name configured via `LCG_EXTRACTION_LLM` continues to drive model-specific behavior even when the body omits `model`.
- Gateways targeted respond in standard Anthropic Messages shape (`content[...]` blocks including `tool_use`), as the requester confirmed for Bedrock.
- The `lcg-eval` judge's separate hardcoded Anthropic call is a developer tool, not part of the service's extraction path.

## Out of Scope *(optional)*

- A built-in OAuth2 client-credentials fetcher (token URL, client id/secret, scope, caching and refresh). Tracked as a follow-up; the Bearer-credential injection here is designed to be the seam it plugs into.
- Native AWS Bedrock support (SigV4 signing, Bedrock Runtime streaming/`invoke-with-response-stream`, AWS credential chain).
- Translating between non-Anthropic request/response shapes (OpenAI-shaped gateways remain the job of `--extractor-http`).
- Per-call or per-group differing endpoints or credentials.
- Changing the OpenAI-compatible extractor or the embedder transports.
- Changing the hardcoded endpoint in the `lcg-eval` judge.

## Source References *(optional)*

- Issue #346 and the 2026-08-04 follow-up comment describing the Bedrock/OAuth2 gateway contract.
- `docs/configuration.md` — existing `ANTHROPIC_API_KEY`, `LCG_EXTRACTION_URL`, `LCG_EXTRACTION_API_KEY`, `LCG_EXTRACTION_LLM` entries.
- `specs/212-local-openai-compatible-extraction/spec.md` and ADR-0041 — the existing local OpenAI-compatible extraction adapter this feature must not disturb.
- Anthropic SDK conventions: `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`.
