---
layout: default
title: Configuration
---

# Configuration

## CLI flags

| Flag | Description |
|------|-------------|
| `--embedder-uds <path>` | Unix domain socket for the embedder sidecar (default on macOS: `/tmp/liminis-inference.sock`, auto-detected). |
| `--embedder-http <url>` | HTTP URL for an OpenAI-compatible embedding endpoint. Mutually exclusive with `--embedder-uds`. |
| `--extractor-uds <path>` | Unix domain socket for a local OpenAI-compatible extraction endpoint. |
| `--extractor-http <url>` | HTTP URL for a local OpenAI-compatible extraction endpoint. Mutually exclusive with `--extractor-uds`. |
| `--reranker-uds <path>` / `--reranker-http <url>` | Where `knowledge_search_passages` reaches the cross-encoder reranker (`POST /v1/rerank`). Mutually exclusive. Default: the embedder's sidecar. Only used when a call (or `LCG_RERANK=on`) opts in. Not valid with `--connect`: the daemon runs the search, so set it on the daemon. See [Passage rerank and query expansion](#passage-rerank-and-query-expansion). |
| `--expander-uds <path>` / `--expander-http <url>` | Where `knowledge_search_passages` reaches the query-expansion LLM (an OpenAI-compatible `/v1/chat/completions` route). Mutually exclusive. Default: `LCG_EXPANSION_URL`, else the embedder's sidecar. Only used when a call passes `expand: true`. Not valid with `--connect` (set it on the daemon). |
| `--anthropic-base-url <url>` | Base URL of an Anthropic-compatible gateway (OpenRouter, LiteLLM, a corporate proxy); `/v1/messages` is appended. Same meaning as `ANTHROPIC_BASE_URL`; the flag wins. Not valid with `--connect`. See [Anthropic-compatible gateways](#anthropic-compatible-gateways-bedrock-openrouter-litellm). |
| `--anthropic-endpoint <url>` | Full Anthropic endpoint URL, used verbatim as the POST target (e.g. a Bedrock gateway's `<base>/<deployment>/v1/invoke`). Same meaning as `LCG_ANTHROPIC_ENDPOINT`; wins over the base URL. Not valid with `--connect`. |
| `--mcp-stdio` | Starts a native [Model Context Protocol](https://modelcontextprotocol.io) server over stdin/stdout instead of binding the Unix socket. See [MCP-over-stdio transport](ipc-mcp-reference.md#mcp-over-stdio-transport). |
| `--scope=<list>` | MCP-stdio only. Comma-separated list of scopes to advertise in `tools/list` (default `all`): `read`, `write`, `cypher`, `admin`. |
| `--connect <path>` | MCP-stdio only. Attached mode: forward every `tools/call` as JSON-RPC over the given Unix socket to an already-running service, instead of opening the database directly. |
| `--allow-remote-close` | MCP-stdio attached mode only. Advertise and allow `knowledge_close`, forwarding the shutdown to the remote service. No effect in standalone mode. |
| `--writable-groups=<g1,g2,…>` | Daemon-wide **write allow-list**: only the listed groups may be changed by any client, over any connection path (IPC, standalone MCP, attached MCP). Writes to any other group are refused with JSON-RPC error `-32004` and write nothing. Exact, case-sensitive group ids; unset = every group writable. An empty or malformed list fails startup. A call with no `group_id` targets the default group (`liminis`), which must be listed to stay writable. `knowledge_query_cypher` is refused entirely while the list is set. Not accepted with `--connect` (set it on the daemon). See [Per-group write allow-list](ipc-mcp-reference.md#per-group-write-allow-list). Env equivalent: `LCG_WRITABLE_GROUPS`. |
| `--help` | Print usage and exit. |
| `--version` | Print the binary's version and exit. |

`--embedder-uds`/`--embedder-http` are for the embedding sidecar; `--extractor-uds`/`--extractor-http` are for the extraction LLM. See [Embedder sidecar](#embedder-sidecar) and [Extractor: local or hosted](#extractor-local-or-hosted) below.

## Environment variables

| Variable | Required | Description |
|----------|----------|-------------|
| `LCG_SOCKET_PATH` | No | Unix socket path the IPC daemon listens on (default `.lcg/service.sock`). On Windows it names the workspace endpoint instead: the service serves a named pipe derived from this path and writes its name to `.lcg/service.endpoint` — see [IPC & MCP Reference: Windows](ipc-mcp-reference.md#windows-named-pipe). A `\\.\pipe\…` value is used as the pipe name directly. |
| `LCG_DB_PATH` | No | Path to the LadybugDB database file (default `.lcg/db/liminis.db`) |
| `LCG_EMBEDDING_URL` | No | Fallback HTTP URL used when neither `--embedder-uds` nor `--embedder-http` is passed and the default UDS socket (`/tmp/liminis-inference.sock`) is absent. On Unix, if this var is also unset, the binary exits with an error. On non-Unix, defaults to `http://127.0.0.1:8765/v1/embeddings`. |
| `LCG_EMBEDDING_MODEL` | No | Embedding model name sent in requests (default `bge-base-en-v1.5`) |
| `LCG_EMBEDDING_DIM` | No | Embedding dimension override if probe fails at startup (default: auto-detected via probe). Does **not** override an authentication failure (HTTP 401/403) — see [Embedder sidecar](#embedder-sidecar) below. |
| `LCG_EMBEDDING_PROFILE` | No | Embedding instruction profile: `none` (default), `bge-v1.5`, `nomic-v1.5`, `e5`, or `embedder`. Tells LCG which role prefixes to put on search strings (query) and stored text (document) for asymmetric embedding models. An unknown name is a startup error listing the valid ones. See [Embedding instruction profiles](#embedding-instruction-profiles). |
| `LCG_EMBEDDING_QUERY_PREFIX` | No | Explicit query prefix; overrides the profile's query prefix. An explicitly empty value disables it. A query-prefix change never needs a re-embed. |
| `LCG_EMBEDDING_DOCUMENT_PREFIX` | No | Explicit document prefix; overrides the profile's document prefix. An explicitly empty value disables it. **Part of the workspace's embedding identity** — changing it needs a re-embed. |
| `LCG_PASSAGE_SEARCH_MODE` | No | Default retrieval mode for `knowledge_search_passages` when a request names no `mode`: `vector` (default; cosine similarity, then belief-aware re-ranking unless turned off with `belief_aware: false` or `LCG_PASSAGE_BELIEF_AWARE=off`, which can lower the `score` of a demoted passage) or `hybrid` (BM25 + vector, weighted RRF, with belief-aware re-ranking by default; see [IPC reference](ipc-mcp-reference.md#hybrid-passage-search-knowledge_search_passages-issue-789)). Read per request. An unrecognised value logs a warning to stderr and falls back to `vector`. A request's own `mode` always wins. |
| `LCG_PASSAGE_BELIEF_AWARE` | No | Workspace default for belief-aware re-ranking in `knowledge_search_passages` (a derived page ranks below the primary note it cites, a superseded passage below its successor), applied when a request omits `belief_aware`. Belief-aware ranking is **on by default** in both modes (#806). `off`, `0` or `false` turn it off, restoring the pre-#806 ordering byte-for-byte in `vector` mode (in `hybrid` it gives weighted RRF without the rules, which is not the old hybrid default); `on`, `1` or `true` leave it on. Case-insensitive. Read per request. An unrecognised value logs a warning to stderr once and is ignored (default applies). A request's own `belief_aware` always wins. |
| `LCG_RERANK` | No | `on` makes the passage **rerank** stage the workspace default for `knowledge_search_passages` (default `off`). A per-call `rerank: true`/`false` always wins. Query expansion has no workspace default by design. See [Passage rerank and query expansion](#passage-rerank-and-query-expansion). |
| `LCG_RERANK_CANDIDATES` | No | How many first-stage candidates the reranker rescores (default `50`, which is also the maximum; values above 50, and `0`, are clamped to the range 1–50 and a warning is logged). Per-call override: `rerank_candidates`. |
| `LCG_RERANK_MODEL` | No | Model name sent to `/v1/rerank` (default `mixedbread-ai/mxbai-rerank-large-v1`; `BAAI/bge-reranker-base` is a supported alternative). The name is informational: the sidecar scores with whichever model it loaded (`LOCAL_INFERENCE_RERANK_MODEL`) and ignores this field, so a mismatch is not detected or rejected. The sidecar reports its own model id in the response. |
| `LCG_RERANK_TIMEOUT_MS` | No | Bound for one rerank request attempt (default `10000`). It is not an overall deadline: a call first waits for a free connection (up to the same bound when all four are busy), then sends, so the worst case is about twice this bound, plus a re-dial (2 s) and a second attempt if the connection was broken. The recorded Core ML p50 for the default model is about 1.9 s at 50 candidates (p95 not measured), and the sidecar scores serially and keeps computing after a client times out, so concurrent searches queue behind abandoned requests. A call waits for a busy connection (the client keeps four per endpoint) for up to this same bound; only dialling the socket uses the 2 s connect bound. A timeout degrades to first-stage results (`reason: "unavailable"`) and is never retried; lower the candidate count or raise this bound if you see it. |
| `LCG_RERANK_SUPERSEDED_FACTOR` | No | Demotion multiplier applied to a superseded passage's whole blended score — first-stage rank prior and sigmoid logit together, before the recency term is added — in `0..=1` (default `0.5`); a value above `1` would boost superseded notes and stops the service at startup. |
| `LCG_RERANK_RECENCY_WEIGHT` | No | Weight of the small recency prior used to prefer later notes among near-equal matches (default `0.05`; `0` disables it). |
| `LCG_RERANK_RANK_WEIGHT_TOP` / `_MID` / `_TAIL` | No | Weight of the **first-stage rank** in the rerank blend (`a·reciprocal-rank + (1−a)·sigmoid(logit)`) at ranks 1–3, 4–10 and beyond (defaults `0.30`, `0.20`, `0.10`; each `0..=1`, a larger value stops the service at startup; `0` for all three orders by the cross-encoder alone). A cross-encoder alone can bury a first-stage top result, so the first stage keeps most weight at the top. Tuned on corpus v2 only; the v1 holdout was then measured by the maintainer (0.935 against 0.894 for `lcg-passages-belief`); see ADR-0796. |
| `LCG_EXPANSION_URL` | No | HTTP endpoint of an OpenAI-compatible LLM for query expansion (a base URL; a trailing `/v1/...` is stripped). LCG appends `/v1/chat/completions` to it, so the provider must serve that OpenAI-style path under the base: a provider whose API version segment is not `/v1` is not supported directly. Used when `--expander-*` is not passed; with neither, the embedder's sidecar (Apple Foundation Models) is used. |
| `LCG_EXPANSION_MODEL` | No | Model name sent to the expansion endpoint (default `apple-foundation`). |
| `LCG_EXPANSION_API_KEY` | No | Bearer token for the expansion endpoint named by `LCG_EXPANSION_URL`. It is sent **only** to that endpoint: an endpoint chosen with `--expander-http`/`--expander-uds`, or defaulted to the sidecar, never receives it (a note is logged at startup if the key is set but ignored). |
| `LCG_EXPANSION_TIMEOUT_MS` | No | Bound for one expansion request attempt (default `3000`; the same additive worst case as `LCG_RERANK_TIMEOUT_MS` applies when `expand: true` calls queue behind each other, so about twice this bound; raise it for a slow expansion LLM). On timeout the search uses the original query alone. The original query's results are held until the expansion call returns or this bound elapses, so a stalled (not refused) LLM costs the full timeout: an `expand: true` call can take up to this long. |
| `LCG_EMBEDDING_API_KEY` | No | Bearer token sent as `Authorization: Bearer <key>` on the HTTP embedder transport (`--embedder-http`/`LCG_EMBEDDING_URL`). Falls back to `GRAPHITI_EMBEDDING_API_KEY`, then to `OPENAI_API_KEY`. Unset (or empty) sends no `Authorization` header, unchanged from prior versions. Never applies to the UDS transport (`--embedder-uds`) — a local socket never accepts a credential regardless of this variable. |
| `OPENAI_API_KEY` | No | Convenience fallback for `LCG_EMBEDDING_API_KEY` (see above) — reuses an already-exported OpenAI key. Not a deprecated alias, so using it logs no *deprecation* warning — but since it sends whatever key is exported for OpenAI tooling generally to whatever `--embedder-http`/`LCG_EMBEDDING_URL` endpoint is configured (not necessarily OpenAI's own API), using this tier does log a one-line informational notice naming the endpoint the key will be sent to. |
| `LCG_EMBEDDING_TIMEOUT_MS` | No | Whole-request timeout, in milliseconds, applied per attempt by **both** embedder transports — HTTP (`--embedder-http`/`LCG_EMBEDDING_URL`) and UDS (`--embedder-uds`, the default when the sidecar's socket exists) — covering single embed, batch embed, and the startup probe (default `30000`, generous enough for a default-sized 64-text batch under normal latency). An unparseable or zero value is rejected with a configuration error rather than silently ignored. See [ADR-0510](adr/0510-oaiembedder-http-timeouts.md) (HTTP) and [ADR-0541](adr/0541-uds-embedder-transport-timeouts.md) (UDS). |
| `LCG_EMBEDDING_CONNECT_TIMEOUT_MS` | No | Connect-phase timeout, in milliseconds, independent of `LCG_EMBEDDING_TIMEOUT_MS` above (default `5000`), applied by both embedder transports. On the HTTP transport it bounds how long a call can stall before completing the TCP/TLS handshake. On the UDS transport (#541) it bounds a single combined budget for acquiring a usable pooled connection — waiting for a free pool slot **and**, if that slot is empty, dialing a fresh connection (socket connect + HTTP/1.1 handshake) — not two stacked timeouts. Same validation as `LCG_EMBEDDING_TIMEOUT_MS`. See [ADR-0510](adr/0510-oaiembedder-http-timeouts.md) (HTTP) and [ADR-0541](adr/0541-uds-embedder-transport-timeouts.md) (UDS). |
| `LCG_EMBED_BATCH_SIZE` | No | Number of texts per array-valued `/v1/embeddings` request when `OaiEmbedder::embed_batch` chunks a larger batch (default `64`, valid range `1`–`256`). The embedding sidecar's real per-request limit is unmeasured; lower this if a batch call fails due to a request-size limit. See [ADR-0445](adr/0445-embedder-batch-api.md). |
| `LCG_EXTRACTION_LLM` | No | Anthropic model for entity extraction, optional `primary:fallback` format. Only consulted on the Anthropic path (see `ANTHROPIC_API_KEY` below); ignored when a local extraction endpoint is selected. |
| `LCG_EXTRACTION_URL` | No | Fallback HTTP URL used when no `--extractor-uds`/`--extractor-http` flag is passed and `ANTHROPIC_API_KEY` is unset. If this var is also unset in that situation, no extraction provider is configured: the binary still starts, but every extraction-dependent call (`knowledge_process_chunk`, `knowledge_add_episode`, `knowledge_reprocess_entity_types`, `knowledge_reprocess_relation_types`) fails with an error identifying the missing configuration — extraction has no default-socket auto-detection (unlike the embedder), so a running sidecar alone is not enough. |
| `LCG_EXTRACTION_MODEL` | No | Model name sent in local extraction requests (default `local`) — decorative against the bundled sidecar (which ignores the request's `model` field), but meaningful for real OpenAI-compatible servers reached via `--extractor-http`. |
| `LCG_EXTRACTION_API_KEY` | No | Bearer token sent as `Authorization: Bearer <key>` on the HTTP extraction transport (`--extractor-http`/`LCG_EXTRACTION_URL`), for authenticated OpenAI-compatible proxies such as a corporate LLM gateway. Unset sends no header. Never applies to `--extractor-uds`. |
| `LCG_EXTRACTION_MAX_TOKENS_CEILING` | No | Uniform ceiling (in tokens) on the per-call `max_tokens` budget for entity/edge extraction, across both the hosted Anthropic path and self-hosted/OAI-compatible models (default `32768`). The per-call initial budget scales with input chunk size up to this ceiling; it exists to stop genuine non-termination (a model that never stops generating), not to optimize spend, so it is intentionally generous. An invalid value (non-numeric, or below the compiled-in 4096-token floor) logs a warning to stderr and falls back to the default. Setting it near the floor is valid but self-defeating: since the per-call budget is `clamp(chunk_len_bytes * ratio, 4096, ceiling)`, a ceiling close to 4096 collapses that range and every call effectively gets the same floor-sized budget regardless of chunk size — silently disabling proportional scaling, not just narrowing the runaway guard. |
| `LCG_CHUNK_TEXT_ADVISORY_MAX_CHARS` | No | Advisory threshold, in **characters** (not bytes), above which `knowledge_process_chunk`'s `chunk_text` is considered oversized (default `8000`). Ingestion is unaffected — no rejection, truncation, or internal splitting — but a call whose `chunk_text` exceeds this threshold gains a `warning` field in its result naming the actual character count and the recommended maximum, and emits a `chunk_text_oversized` telemetry event (see [Telemetry](telemetry.md)). Extraction quality degrades well before any context-window limit is reached, so splitting oversized input into multiple `knowledge_process_chunk` calls is the caller's responsibility. An invalid value (non-numeric, zero, or negative) logs a warning to stderr and falls back to the default. |
| `LCG_DOCUMENT_MAX_BYTES` | No | Document-size cap for `knowledge_process_document` and `knowledge_preview_chunks`, in **UTF-8 bytes** (default `524288`, 512 KiB). A larger document is refused with `document_too_large` before any extraction or write. An invalid value (non-numeric, zero, or negative) logs a warning to stderr and falls back to the default. |
| `LCG_DOCUMENT_CHUNK_MAX_CHARS` | No | Per-unit size, in **characters**, above which document mode splits a section into several extraction units (default `6000`, below the `8000`-character chunk advisory). The units stay one logical chunk for diffing. Changing it changes chunk boundaries, so the next ingest of an existing document re-extracts the sections it re-splits. Invalid values fall back to the default with a warning. |
| `LCG_RECORD_LLM` | No | Path to an LLM cassette (JSONL). If set, every extraction call is recorded to this file in addition to running live — see [Record/replay cassettes](testing-and-evaluation.md#recordreplay-cassettes). Mutually exclusive with `LCG_REPLAY_LLM`. |
| `LCG_REPLAY_LLM` | No | Path to a previously recorded LLM cassette (JSONL). If set, extraction is served entirely from the cassette — no extractor provider is resolved, no credentials are required, and no network call is ever made. Mutually exclusive with `LCG_RECORD_LLM`. See [Record/replay cassettes](testing-and-evaluation.md#recordreplay-cassettes). |
| `LCG_DEDUP_LLM` | No | Enables the **LLM-verified extraction dedup check** ([ADR-0652](adr/0652-llm-verified-extraction-dedup-via-extractor.md)). **Off by default** (opt-in). When on *and* an extraction provider is configured, every embedding-path dedup candidate that survives the identifier-mismatch veto is judged by the same extractor used for extraction (the Anthropic API, or the OpenAI-compatible `--extractor-uds` / `--extractor-http` endpoint) — one batched call per chunk (split into sequential groups of at most 16 pairs, each bounded by its own 30 s timeout). `duplicate` merges; `not duplicate`, an extractor error, a timeout or any malformed/unattributable answer inserts a separate entity (a missed merge is recoverable, a wrong merge is not) and logs a warning. Exact-name matches, veto-rejected pairs and identical-normalized-name pairs never cost a call, and WAL replay never calls the LLM. Values `0`, `false`, `off`, `no` and the empty string are off; anything else is on. If on with no extractor configured the service still starts, warns, and runs veto-only. The active mode is logged at startup (`dedup: mode=…`) and reported as `dedup_mode` (`"veto-only"` or `"llm-verified"`) by `knowledge_status`. Dedup calls report token usage with `role="dedup"`. Under `LCG_RECORD_LLM` / `LCG_REPLAY_LLM` the verdicts are recorded and served as `judge_duplicates` cassette records; a cassette without them yields "not a duplicate". |
| `LCG_DEDUP_ADAPTER_URL` | No | **Deprecated and ignored** (retired in 0.16.3). The bespoke `{candidate, incoming}` HTTP protocol it configured was never implemented by any server shipped with lcg. If set, startup logs a deprecation warning and nothing contacts the URL. Use `LCG_DEDUP_LLM`. |
| `LCG_WAL_DIR` | No | WAL **root** directory (default `.lcg/wal`) — holds one subdirectory per `group_id` (issue #378), e.g. `<LCG_WAL_DIR>/liminis/` for the default group. A pre-378 single-stream directory (loose `*.jsonl`/`.checkpoints/`/`.wal-bounds.json` with no `liminis/` subdirectory) is migrated into this layout automatically on first boot under the upgraded binary — see [Operations](operations.md#on-disk-layout) and [ADR-0378](adr/0378-multi-stream-wal-per-group-directory.md). |
| `LCG_WAL_MAX_BYTES_PER_FILE` | No | Per-file byte-size rotation threshold for the WAL (default `5242880` = 5 MB); set to `0` to disable byte-size rotation and rely on event count only |
| `LCG_WAL_MAX_EVENTS_PER_FILE` | No | Per-file event-count rotation threshold for the WAL (default `10000`); rotation fires when either this threshold or `LCG_WAL_MAX_BYTES_PER_FILE` is reached |
| `LCG_REPLAY_LOG_INTERVAL_SECS` | No | Throttle interval in seconds between `[WAL PROGRESS]` log lines written to stderr during WAL replay (default `30`). Set to `0` to emit a line on every progress event. |
| `ANTHROPIC_API_KEY` | No | API key for Anthropic entity/relationship extraction, sent as the `x-api-key` header. When set (and no explicit `--extractor-uds`/`--extractor-http` flag is passed), extraction uses the Anthropic Messages API — `https://api.anthropic.com/v1/messages` unless a gateway is configured (see the rows below and [Anthropic-compatible gateways](#anthropic-compatible-gateways-bedrock-openrouter-litellm)) — for ingestion (`knowledge_process_chunk` / `knowledge_add_episode`), entity/relation re-classification (`knowledge_reprocess_entity_types`, `knowledge_reprocess_relation_types`), summary consolidation and LLM dedup judgement. When unset (and `ANTHROPIC_AUTH_TOKEN` is unset too), extraction requires an explicit `--extractor-uds`/`--extractor-http` flag or `LCG_EXTRACTION_URL` pointing at a local OpenAI-compatible endpoint — it is not auto-detected — see [Extractor: local or hosted](#extractor-local-or-hosted) below. Not needed for read-only, embedding-only, or non-LLM tools. |
| `ANTHROPIC_AUTH_TOKEN` | No | Bearer credential for an Anthropic-compatible gateway, sent as `Authorization: Bearer <token>` **instead of** `x-api-key` (the SDK-conventional name). Setting it selects the Anthropic provider on its own — no dummy `ANTHROPIC_API_KEY` needed — and wins over `ANTHROPIC_API_KEY` if both are set **and a gateway URL is configured** (the startup log says so). On the stock `https://api.anthropic.com` endpoint with `ANTHROPIC_API_KEY` also set, the API key is kept (unchanged behaviour) and the token is ignored with a startup note, so a token exported for other Anthropic tooling cannot displace a working key. lcg reads it once at startup and never refreshes it; supply a long-lived token, or have an external refresher restart the process (see [token expiry](#token-expiry-and-the-fallback-model)). Environment only — there is deliberately no flag, since argv is visible in process listings. An empty or blank value is treated as unset (SDK convention), so a templated `ANTHROPIC_AUTH_TOKEN=` cannot break a working setup. |
| `ANTHROPIC_BASE_URL` | No | Base URL of an Anthropic-compatible gateway (the official Anthropic SDK convention): `/v1/messages` is appended. `https://gw.example.com` and `https://gw.example.com/` both become `https://gw.example.com/v1/messages`; a base ending in `/v1` is not doubled; a path prefix (`https://gw.example.com/anthropic`) is kept. A base already ending in `/v1/messages`, or carrying a query/fragment, is a startup error pointing you at `LCG_ANTHROPIC_ENDPOINT`. The `--anthropic-base-url` flag wins over this variable. On its own it does not select the Anthropic provider — you still need `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN`. An `ANTHROPIC_BASE_URL` already exported for other Anthropic tooling will redirect lcg too; the startup log names the effective endpoint. Ignored by the OpenAI-compatible extractor and by `lcg-eval`. |
| `LCG_ANTHROPIC_ENDPOINT` | No | Full Anthropic endpoint URL, used **verbatim** as the POST target (no path appended) — e.g. a Bedrock gateway's `https://gw.example.com/<deployment>/v1/invoke`. May carry a query string. Wins over `ANTHROPIC_BASE_URL`/`--anthropic-base-url` when both are set; `--anthropic-endpoint` overrides this variable. Distinct from `LCG_EXTRACTION_URL`, which belongs to the OpenAI-compatible provider and is never reinterpreted. |
| `LCG_ANTHROPIC_OMIT_MODEL` | No | `true`/`1` omits the top-level `model` field from every request body, for gateways that select the model by URL path. Default `false`. `LCG_EXTRACTION_LLM` still names the model for cost telemetry and Sonnet-specific behaviour. Anything other than `true`/`false`/`1`/`0` fails startup. |
| `LCG_ANTHROPIC_VERSION` | No | `anthropic_version` value (default `2023-06-01`). Required when `LCG_ANTHROPIC_VERSION_IN=body` (e.g. `bedrock-2023-05-31`). |
| `LCG_ANTHROPIC_VERSION_IN` | No | Where `anthropic_version` travels: `header` (default — the `anthropic-version` request header) or `body` (an `anthropic_version` field in the JSON body, no header — the Bedrock shape). Exactly one placement is used per request. `body` without an explicit `LCG_ANTHROPIC_VERSION` fails startup rather than silently sending the Anthropic-direct version to a Bedrock gateway. |
| `LCG_ANTHROPIC_PROMPT_CACHING` | No | `on` (default) or `off`. Prompt caching applies only to Sonnet-class models (model name contains `sonnet`): lcg sends `anthropic-beta: prompt-caching-2024-07-31` and marks the system prompt with `cache_control`. `off` suppresses **both**, for strict gateways that reject unknown headers or fields. Haiku models (the default) never send either. |
| `LIMINIS_WORKSPACE_ROOT` | No* | Absolute path to the workspace root. **Required** for the three corrections IPC methods (`knowledge_validate_corrections`, `knowledge_apply_corrections`, `knowledge_reprocess_entity_types`). If unset, those methods return a `-32000` error. The corrections file is read from `{LIMINIS_WORKSPACE_ROOT}/.liminis/knowledge-corrections.yaml`. |
| `LCG_REPLAY_BATCH_SIZE` | No | Rows per batch during WAL replay (default `64`, valid range `1`–`256`). Lower values reduce peak memory on a large rebuild; higher values replay faster. |
| `LCG_BUFFER_POOL_BYTES` | No | Upper bound on lbug's buffer pool (its page cache). A positive integer byte count (`2147483648`) or an integer with a binary suffix `KiB`, `MiB` or `GiB`, case-insensitive (`2GiB`, `512 mib`). Fractions (`2.5GiB`), decimal suffixes (`2GB`), `0`, an empty value and overflow are rejected. Unset = lbug's own default (roughly 80% of system memory). **An invalid value fails startup** with an error naming the variable and the value — it never silently falls back to the default. Applies to every database open (startup, recovery, rebuild, migration validation). lbug enforces its own minimum pool size; the service imposes no floor. The effective value is logged once at startup: `liminis-context-graph: lbug buffer pool: 2.0 GiB (2147483648 bytes) (LCG_BUFFER_POOL_BYTES); max threads: default (lbug auto: one per CPU)`. See [Memory sizing in containers](operations.md#memory-sizing-in-containers) and [ADR-0737](adr/0737-configurable-lbug-tuning-strict-env.md). |
| `LCG_MAX_THREADS` | No | Upper bound on lbug's worker threads. A positive integer. Unset = lbug's default (one per CPU, read from the host, which is misleading inside a CPU-limited container). `0`, an empty value or a non-numeric value fails startup with an error naming the variable and the value. Logged in the same startup line as `LCG_BUFFER_POOL_BYTES`. |
| `LCG_WRITABLE_GROUPS` | No | Environment equivalent of `--writable-groups`: a comma-separated allow-list of the groups clients may change (e.g. `LCG_WRITABLE_GROUPS=mine` or `mine,scratch`). Entries are trimmed; duplicates collapse; matching is exact and case-sensitive. **Strictly parsed:** a set-but-empty value, an empty entry (`a,,b`, a leading/trailing comma) or an entry containing whitespace fails startup with an error naming the variable — it is never treated as "unset". The `--writable-groups` flag wins when both are given (the env value is then not parsed). Unset (and no flag) = every group writable. Read once at startup; changing it needs a restart. The effective list is logged once: `liminis-context-graph: writable groups: [a, b] (all other groups are read-only)`. Ignored, with a warning, by an attached MCP process (`--connect`) — the daemon enforces it. See [Per-group write allow-list](ipc-mcp-reference.md#per-group-write-allow-list) and [ADR-0742](adr/0742-per-group-write-allow-list.md). |
| `LCG_REPLAY_EMBED_WINDOW_SIZE` | No | Rows buffered before their embedding recompute is resolved via one batched embedder call during WAL replay (default `64`, valid range `1`–`256`). Independent of `LCG_REPLAY_BATCH_SIZE` — this bounds embedder round-trips, not Cypher execution batches. Lower values reduce cancellation latency, peak memory, and per-failure blast radius (one transient embedder failure degrades every row in its window, not just one — set to `1` to reproduce pre-batching per-row isolation exactly); higher values reduce embedder round-trips on a large rebuild. See [ADR-0486](adr/0486-batch-wal-replay-embedding-recompute.md). |
| `LCG_REPLAY_FAILURE_SAMPLES` | No | How many distinct failing lines to retain and report per WAL replay (default `10`). Samples are deduplicated by failure shape, so one bad template cannot crowd out the rest. |
| `LCG_REPLAY_FIDELITY_THRESHOLD` | No | Float `0.0`–`1.0`. Replay warns when the fraction of successfully applied mutations falls below this, i.e. the rebuilt graph is not a faithful reconstruction of the WAL. |
| `LCG_MIGRATION_KEEP_BACKUP` | No | When set, a workspace-layout migration retains its pre-migration backup instead of removing it after a successful migration. |
| `LCG_SHUTDOWN_TIMEOUT_MS` | No | Grace period in milliseconds for in-flight requests to finish on `SIGTERM` before the service exits. |
| `LCG_ATTACHED_CALL_TIMEOUT_MS` | No | Idle-read timeout in milliseconds for MCP attached mode (`--connect`, default `30000`). See [MCP-over-stdio transport](ipc-mcp-reference.md#mcp-over-stdio-transport). |
| `LIMINIS_DEDUP_HYBRID_THRESHOLD` | No | Entity count per `group_id` above which dedup switches from brute-force cosine to the hybrid FTS + vector path. |
| `LIMINIS_LLM_COST_TABLE_PATH` | No | Path to a JSON model-pricing table used to populate `estimated_cost_usd` in `token_usage` telemetry. See [Telemetry](telemetry.md). |

That's 33 variables as of this page's writing, covering every `env::var`/`lcg_env_var` call site under `crates/*/src`.

**Deprecated `GRAPHITI_*` aliases.** Every `LCG_*` variable above that predates the rename also
accepts its old `GRAPHITI_*` spelling — `GRAPHITI_SOCKET_PATH`, `GRAPHITI_DB_PATH`,
`GRAPHITI_EMBEDDING_URL`, `GRAPHITI_EMBEDDING_MODEL`, `GRAPHITI_EMBEDDING_DIM`,
`GRAPHITI_EMBEDDING_API_KEY`, `GRAPHITI_EXTRACTION_LLM`, `GRAPHITI_DEDUP_LLM`,
`GRAPHITI_DEDUP_ADAPTER_URL` (deprecated and ignored, like `LCG_DEDUP_ADAPTER_URL`), and `GRAPHITI_WAL_DIR`. Using one logs `DEPRECATED: env var <old> is
deprecated; rename to <new>` at startup. They are honoured for now; prefer the `LCG_*` names.
`OPENAI_API_KEY` is not in this list — it's a convenience fallback for `LCG_EMBEDDING_API_KEY`,
not a legacy spelling of an LCG-specific variable, so using it triggers no deprecation warning.

## Embedder sidecar

> For a one-page comparison of every supported embedding option (platform, install, cost,
> dimension, and verification status), see [Embedding Options](embedding-options.md).

`OaiEmbedder` delegates embedding to an external service over the OpenAI-compatible
`POST /v1/embeddings` contract. The binary supports two transports, selected via CLI flags:

```sh
liminis-context-graph --embedder-uds /tmp/liminis-inference.sock            # Unix domain socket (default on macOS)
liminis-context-graph --embedder-http http://127.0.0.1:8765/v1/embeddings   # HTTP
```

**Default behaviour** (no flags): the binary looks for the Swift CoreML sidecar socket at
`/tmp/liminis-inference.sock`. If absent, it falls back to `LCG_EMBEDDING_URL` (HTTP). If
neither exists, it exits with a clear error.

The binary probes the embedder at startup to confirm it is reachable and auto-detect the
embedding dimension. If the probe fails and `LCG_EMBEDDING_DIM` is not set, the process
exits with an error rather than failing silently on the first embed request. This is the
behaviour for the default Unix-socket service (`liminis-context-graph` with no
`--mcp-stdio` flag), and it is unchanged regardless of how the embedder failed to respond.

**Standalone `--mcp-stdio` mode is different.** When an MCP client (Claude Desktop, an
editor's MCP integration, etc.) launches `liminis-context-graph --mcp-stdio` directly,
nobody is watching the process's stderr — the client just shows a generic "server failed
to start," and the exit-with-an-error message above is never seen. To cover the common
case where the client starts the embedder sidecar and `liminis-context-graph` as sibling
processes with no ordering guarantee, standalone `--mcp-stdio` mode retries an
unreachable embedder with bounded backoff (up to 5 seconds total) before giving up. If the
embedder still isn't reachable once that window elapses, the process does **not** exit —
it starts in a degraded state without opening the database. Call `knowledge_status` from
within the MCP session to discover this: it reports `degraded: true` with
`reason: "embedder_unreachable_at_startup"` and an empty `recovery_available` list (no
`knowledge_recover` strategy can safely run in this state, since no embedding dimension
was ever established — restart the process once the embedder is reachable). If the
embedder becomes reachable during the retry window, startup proceeds normally with no
degraded state at all. A hand-started socket-service process keeps the fail-fast behaviour
above unchanged either way — this retry-and-degrade behaviour applies only to standalone
`--mcp-stdio`.

Start the embedder sidecar **before** starting the `liminis-context-graph` binary.
Without it, the embedding-dependent IPC methods fail immediately with an embedding error:
`knowledge_find_entities`, `knowledge_find_relationships`, `knowledge_search_passages`,
`knowledge_process_chunk`, `knowledge_add_episode`, `knowledge_reprocess_entity_types`, and
`knowledge_canonicalize_relations` (its ontology-description fallback embeds each residual edge's
`fact`). Read-only methods that do not call the embedder (`health_check`, `knowledge_status`,
`knowledge_list_entities`, `knowledge_get_episodes`) work without the sidecar.

### macOS: Swift CoreML sidecar (default)

The repository ships a Swift CoreML sidecar at
[`native/local-inference/`](https://github.com/liminisapp/liminis-context-graph/tree/main/native/local-inference)
that serves OpenAI-compatible `/v1/embeddings` (BGE-base-en-v1.5) and `/v1/chat/completions`
(Apple Foundation Models) over UDS at `/tmp/liminis-inference.sock` — fully local inference for
embedding, and a fully local option for extraction: no API key, no network. macOS 26+ and Xcode
command-line tools are required. See
[`native/local-inference/README.md`](https://github.com/liminisapp/liminis-context-graph/blob/main/native/local-inference/README.md)
for build and run instructions.

`liminis-context-graph` discovers the sidecar's default UDS socket automatically for embedding —
start the sidecar first, then start the binary. **This default path is not shared with the
Electron `liminis` app.** The app does not use `/tmp/liminis-inference.sock` at all — it
launches its own sidecar process bound to a separate, per-workspace socket at
`<workspaceRoot>/.liminis/local-inference.sock`. A bare `liminis-context-graph` binary started
by hand cannot reach the app's sidecar (or vice versa); they are two different sockets serving
two different processes. Extraction does **not** auto-detect this socket
(see [Extractor: local or hosted](#extractor-local-or-hosted)): the sidecar's Foundation Models
backend is not recommended for extraction quality, so using it there requires the explicit
`--extractor-uds /tmp/liminis-inference.sock` flag rather than happening by default.

The sidecar's own `LOCAL_INFERENCE_MODE` environment variable (`embeddings` | `completions` |
`both`, default `both`) selects which of its own two endpoints it serves — see
[`native/local-inference/README.md`](https://github.com/liminisapp/liminis-context-graph/blob/main/native/local-inference/README.md#selecting-a-mode)
for the full table. Running `LOCAL_INFERENCE_MODE=completions` needs no CoreML model setup at
all, since it serves only `/v1/chat/completions`. This mode selection is independent of, and does
**not** change, the extraction-routing behavior above: Foundation-Models-backed completions are
still not a drop-in substitute for the extraction backend, and nothing should route
`--extractor-uds` at this sidecar on the assumption that enabling completions mode makes it one.

### HTTP transport (CI / Linux / custom embedders / hosted providers)

For environments without the Swift sidecar, pass `--embedder-http` pointing at an endpoint
speaking the same `{input, model}` request / `{data: [{embedding}]}` response wire shape
`OaiEmbedder` sends and expects. OpenAI's own `/v1/embeddings` is the concrete, verified case:

```bash
liminis-context-graph --embedder-http https://api.openai.com/v1/embeddings
```

A local unauthenticated server (Ollama, llama.cpp, LM Studio, text-embeddings-inference,
Infinity, vLLM) works exactly as before — set no key and none is sent. This does **not** make
every self-described "OpenAI-compatible" provider a drop-in target: only Bearer-token auth
against this specific wire shape is supported, not divergent request/response formats, and no
other third-party provider's compatibility is verified here.

See [ADR 0006](adr/0006-embedder-http-contract.md) and
[ADR 0016](adr/0016-oai-embedding-contract-uds-transport.md) for the wire contract
specification and transport decision record.

**Authenticating against a hosted endpoint.** Set `LCG_EMBEDDING_API_KEY` (or reuse an
already-exported `OPENAI_API_KEY`) and the binary sends `Authorization: Bearer <key>` on every
HTTP embedder request — both the startup probe and steady-state `embed()` calls:

```bash
export LCG_EMBEDDING_API_KEY=sk-...
liminis-context-graph --embedder-http https://api.openai.com/v1/embeddings
```

If the endpoint rejects the credential (HTTP 401/403), startup fails with a message identifying
it as an authentication problem and naming the relevant env vars — this is always fatal and,
unlike a generic probe failure, is **not** bypassable via `LCG_EMBEDDING_DIM`: a dimension
override cannot fix a rejected credential. The key itself never appears in the startup log line,
any other log output, or telemetry; if the configured URL embeds Basic-auth-style userinfo
(`user:pass@host`), that component is redacted before being echoed anywhere. See
[ADR 0497](adr/0497-embedder-http-bearer-auth.md).

### MCP client config recipes

An MCP client (Claude Desktop, Claude Code, or any other MCP client) launches
`liminis-context-graph` as a subprocess by specifying exactly `command`, `args`, `env`, and
`cwd` in an `mcpServers` entry — the same `mcpServers` JSON shape already used in
[Getting Started](getting-started.md#talk-to-it-over-mcp) and the
[IPC & MCP Reference](ipc-mcp-reference.md#example-mcp-client-config). Those CLI flags and env
vars are read by exactly the same resolution logic described above, regardless of what
launched the process — there is no MCP-specific configuration surface. The five recipes below
translate that resolution order into copy-pasteable client config for each embedder backend.

**Default macOS sidecar — no `env` block needed.** The default UDS socket
(`/tmp/liminis-inference.sock`) is auto-discovered, so an `mcpServers` entry for the bundled
Swift CoreML sidecar needs no embedder-related `env` entry at all:

```json
{
  "mcpServers": {
    "liminis-context-graph": {
      "command": "liminis-context-graph",
      "args": ["--mcp-stdio", "--scope=read,write"],
      "cwd": "/path/to/your-workspace"
    }
  }
}
```

This applies to macOS only: the default-UDS auto-discovery tier doesn't exist on non-Unix
platforms at all (see the environment variable table above), and the sidecar binary itself
(Swift/CoreML) only runs on macOS regardless.

**Local OpenAI-compatible server (Ollama, LM Studio, text-embeddings-inference, vLLM).** Point
`LCG_EMBEDDING_URL` at the local server and set a matching `LCG_EMBEDDING_MODEL` — a mismatched
model name produces a probe failure at startup rather than a clear error, so get it right:

```json
{
  "mcpServers": {
    "liminis-context-graph": {
      "command": "liminis-context-graph",
      "args": ["--mcp-stdio", "--scope=read,write"],
      "cwd": "/path/to/your-workspace",
      "env": {
        "LCG_EMBEDDING_URL": "http://127.0.0.1:11434/v1/embeddings",
        "LCG_EMBEDDING_MODEL": "nomic-embed-text"
      }
    }
  }
}
```

LM Studio, text-embeddings-inference, and vLLM follow the identical shape — only the port and
`LCG_EMBEDDING_MODEL` change:

| Server | Typical `LCG_EMBEDDING_URL` | Example `LCG_EMBEDDING_MODEL` |
|---|---|---|
| Ollama | `http://127.0.0.1:11434/v1/embeddings` | `nomic-embed-text` |
| LM Studio | `http://127.0.0.1:1234/v1/embeddings` | whatever model is currently loaded in LM Studio |
| text-embeddings-inference | `http://127.0.0.1:8080/v1/embeddings` | the model the server was launched with (`--model-id`) |
| vLLM | `http://127.0.0.1:8000/v1/embeddings` | the model vLLM was launched with (`--model`) |

Worth knowing (not necessarily a mistake): if the macOS default-UDS sidecar is also running on
the same machine, it silently wins over this `env` block per the resolution order described
above under [Embedder sidecar](#embedder-sidecar) — stop the sidecar, or pass an explicit
`--embedder-uds`/`--embedder-http` flag (see the next recipe), to make this `env` block take
effect.

**Hosted OpenAI-compatible provider with an API key.** Set `LCG_EMBEDDING_URL` (or pass
`--embedder-http` in `args`), `LCG_EMBEDDING_API_KEY` (falls back to `OPENAI_API_KEY` if
unset — see the environment variable table above), and a matching `LCG_EMBEDDING_MODEL`:

```json
{
  "mcpServers": {
    "liminis-context-graph": {
      "command": "liminis-context-graph",
      "args": ["--mcp-stdio", "--scope=read,write"],
      "cwd": "/path/to/your-workspace",
      "env": {
        "LCG_EMBEDDING_URL": "https://api.openai.com/v1/embeddings",
        "LCG_EMBEDDING_API_KEY": "sk-...",
        "LCG_EMBEDDING_MODEL": "text-embedding-3-small"
      }
    }
  }
}
```

Caution: an MCP client config file is not a secrets manager — the key above is stored in
plaintext wherever that config file lives on disk, the same as any other credential placed in
an `env` block. Treat the config file itself as sensitive.

**Explicit non-default UDS path.** If your sidecar-compatible embedder listens on a Unix
domain socket at a path other than `/tmp/liminis-inference.sock` — or you want to force UDS
selection even when a different sidecar occupies the default path — pass `--embedder-uds` as a
CLI flag in `args`, not as an `env` var:

```json
{
  "mcpServers": {
    "liminis-context-graph": {
      "command": "liminis-context-graph",
      "args": ["--mcp-stdio", "--scope=read,write", "--embedder-uds", "/tmp/my-embedder.sock"],
      "cwd": "/path/to/your-workspace",
      "env": {
        "LCG_EMBEDDING_MODEL": "bge-base-en-v1.5"
      }
    }
  }
}
```

No `env` block is required unless `LCG_EMBEDDING_MODEL` needs to differ from the default
`bge-base-en-v1.5`. An explicit `--embedder-uds`/`--embedder-http` flag always wins over an
embedder `env` var per the resolution order — worth a callout since it's a plausible source of
"I set the env var and it's still not being used" confusion, distinct from the sidecar-silent-
win case above: here it's *your own* flag taking precedence, not the sidecar's.

#### Switching an existing workspace's embedder

> Changing only the [instruction profile](#embedding-instruction-profiles) (not the model) is a
> smaller case: a query-prefix change needs no re-embed; a document-prefix change needs the
> rebuild described there.

The following applies only when *switching* the embedder for a workspace that already has a
`.lcg/` database with content in it. A fresh workspace with no pre-existing `.lcg/` has nothing
to re-ingest, so none of this applies to first-time setup.

**A dimension change is not a live config change.** `embedding_dim` is baked into the LadybugDB
schema as a fixed-width vector column at database-creation time. Switching, say, from the
macOS sidecar (768-dim, BGE-base-en-v1.5) to `text-embedding-3-small` (1536-dim) against an
existing `.lcg/` database does not resize those columns. `knowledge_rebuild_from_wal` alone is
**not** sufficient here: it recomputes vectors against the already-created schema and, when a
recomputed vector's length doesn't match the already-stored one, it keeps the old
wrong-dimension vector and counts the row under `embeddings_recompute_failed` rather than
fixing it (see [Operations](operations.md#knowledge_status-health-fields)). The correct remedy is a
full re-ingest, or `knowledge_recover` with `{"strategy": "rebuild_from_workspace_wal"}`, which
drops and recreates the database (including schema, at the new embedder's dimension) before
replaying every group's WAL back in.

**`LCG_EMBEDDING_DIM` does not help here.** As described above under
[Embedder sidecar](#embedder-sidecar), that variable only overrides a non-transport,
non-auth *probe* failure at startup — it cannot resolve a genuine dimension mismatch against
already-stored vectors, nor an unreachable or unauthenticated embedder.

**Pin `cwd` to control where `.lcg/` lands.** Every recipe above includes a `cwd` field for
this reason: an MCP client (not a shell you control) decides the launched process's working
directory, and `.lcg/` is created relative to it. Omitting `cwd` leaves `.lcg/` wherever the
client process's own default working directory happens to be — for most MCP clients, that is
not your project directory.

## Embedding instruction profiles

Open embedding models such as BGE, nomic and e5 are **asymmetric**: they were trained with a
role-specific instruction (a prefix) on each side of a search, so the *search string* and the
*stored text* should be embedded differently. LCG knows which side every piece of text is on —
a search string is a **query**; episode content, entity names and summaries, and edge facts are
**documents** — and can add the right prefix for you. With the default profile `none` it adds
nothing, and every string sent to the embedder is exactly what it was before this existed.

```sh
LCG_EMBEDDING_PROFILE=nomic-v1.5 liminis-context-graph --mcp-stdio
```

| Profile | Query prefix (search strings) | Document prefix (stored text) | Use with |
|---|---|---|---|
| `none` (default) | *(none)* | *(none)* | anything; behaviour unchanged |
| `bge-v1.5` | `Represent this sentence for searching relevant passages: ` | *(none)* | BGE-base/large-en-v1.5, including the bundled macOS sidecar |
| `nomic-v1.5` | `search_query: ` | `search_document: ` | nomic-embed-text v1.5 (Ollama) |
| `e5` | `query: ` | `passage: ` | e5 family |
| `embedder` | *(the embedder applies its own)* | *(the embedder applies its own)* | the bundled sidecar — see below |

The trailing space in each prefix is part of the prefix. A text that is empty or only whitespace
is never prefixed, and a text that already starts with the prefix still gets it (there is no
de-duplication). Symmetric text-vs-text comparisons (entity resolution, ontology
canonicalization) embed both sides as documents.

### Overrides

`LCG_EMBEDDING_QUERY_PREFIX` and `LCG_EMBEDDING_DOCUMENT_PREFIX` override the profile's value for
that side only, for a model or fine-tune no profile covers. An explicitly empty value
(`LCG_EMBEDDING_DOCUMENT_PREFIX=`) disables that side's prefix and is different from leaving the
variable unset (which uses the profile's value). An empty value only matters where it changes
something: on a side where the profile's prefix is already empty (always the case under `none`
and `embedder`) it is ignored, so a blank `LCG_EMBEDDING_QUERY_PREFIX=` left in a compose or
`.env` file does not switch the status `source` to `lcg` and does not conflict with
`embedder`. A non-empty override combined with the `embedder` profile is a startup error.

### When you need to re-embed

Only the **document** prefix changes what is stored, so only it is part of the workspace's
embedding identity (recorded next to the model name and dimension):

| You change | Re-embed? |
|---|---|
| the **query** prefix (including `none` → `bge-v1.5`, which has no document prefix) | **No.** Stored vectors are unaffected; the next search uses the new prefix. |
| the **document** prefix (e.g. `none` → `nomic-v1.5` or `e5`) | **Yes.** |

After a document-prefix change, `knowledge_status` reports `embedding_model_status: "mismatch"`
for the affected groups. Run `knowledge_rebuild_from_wal` (or
`knowledge_recover` with `{"strategy": "rebuild_from_workspace_wal"}`), which recomputes every
stored vector under the new document prefix and records it. Until you do, ordinary writes do
**not** clear the mismatch — LCG keeps the previously-recorded prefix so old- and new-prefix
vectors are never silently presented as one consistent index. A workspace written before this
feature has no recorded prefix and is treated as the empty prefix, so upgrading with profile
`none` is a `match` and forces nothing.

### The `embedder` profile (bundled sidecar)

The bundled macOS sidecar can apply its own model's instruction. It always reports its real model
id and, when the loaded model has instructions, an `instruction_roles` advertisement in its
`/v1/embeddings` response. **That advertisement changes nothing by itself.** With profile `none`
LCG sends exactly `{input, model}` as before. With `LCG_EMBEDDING_PROFILE=embedder`, and only if
the embedder advertised support, LCG sends plain text plus `input_type: "query" | "document"` and
the sidecar applies its model's instruction (for BGE v1.5: the query instruction only). If the
embedder advertised nothing, `embedder` falls back to `none` with a warning on stderr.

A text never receives two instructions: either LCG applies a named profile or override (and never
sends `input_type`), or the embedder applies its own (and LCG adds nothing). `embedder` combined
with an explicit prefix override is ambiguous and is a startup error.

### Does the BGE instruction help?

On the v1 Fernhill fixture (43 questions) BGE's query instruction showed no net gain for passage
search (paraphrase and temporal up, answer questions down) and a small gain for entity and fact
search, all within noise. `none` therefore stays the default, and `bge-v1.5` / `embedder` are
available to opt into. nomic and e5, in contrast, expect their prefixes on both sides. The
recommended profile per embedder is in [Embedding Options](embedding-options.md); the current
measurement and how to repeat it is in
[Retrieval Quality Evaluation](retrieval-quality-evaluation.md#comparing-embedding-profiles).

`knowledge_status` shows what is in effect under `embedding_instructions` (`profile`, `source` —
`none`, `lcg` or `embedder` — `query_prefix`, `document_prefix`), and the recorded document prefix
as `wal.embedding_doc_prefix` (also per group in `wal_groups`).

## Passage rerank and query expansion

`knowledge_search_passages` has two **optional** stages for finding a note by what it *does* rather
than what it is called ("the initiative covering shift rostering"). Both are **off by default**, are
switched on per call, and are independent of each other. A client that sets neither sees no change in
results, ordering, response shape or latency.

> **Status: rerank measured, expansion not.** The maintainer measured rerank on corpus v2 (and the v1
> holdout) with the default model: with a belief-aware first stage it scores 0.759 nDCG@10 against 0.653
> for `lcg-passages-belief` (+0.106 [+0.073, +0.142]), but at about 1.9 s p50, far over the 150 ms budget (see
> [ADR-0796](adr/0796-passage-rerank-and-query-expansion.md#belief-aware-first-stage-maintainer-head-2471ad5d)).
> Both stages stay off by default. Query expansion's per-category verdict is not yet recorded, so treat
> it as experimental.

| Stage | Turn on | What it does | Cost | Use it for |
|-------|---------|--------------|------|------------|
| **Rerank** | `rerank: true` per call, or `LCG_RERANK=on` | Rescores the top N first-stage candidates (default 50, the maximum) with a local cross-encoder, then blends in belief and recency so a superseded note does not outrank its replacement when both are among the reranked candidates (a replacement outside the top N is not pulled in) | target ≤ 150 ms p50 at k = 10 on Apple silicon (a budget). The default model does not meet it: the recorded Core ML p50 is about 1.1 s at 25 candidates and 1.9 s at 50 (the default), on one maintainer machine, p95 not measured (see the ADR); the earlier ~587 ms figure was a Python/MPS run | Deliberate searches by role or description (an agent, or a user who asks and waits); **not** search-as-you-type: the recorded default-model latency is seconds, not the 150 ms budget, until a faster model or a smaller N is measured |
| **Query expansion** | `expand: true` per call only | A local LLM rewrites the query into keyword, paraphrase and hypothetical-answer variants; their results are RRF-fused (before rerank, if both are on) | expected on the order of a second per query (not yet measured) | Deliberate agent searches. **Never** search-as-you-type |

Neither stage can fail a search. If the reranker or the LLM is unreachable, slow, or not configured,
the call returns the un-staged results and the response says so:

```json
{"passages": [...], "count": 10,
 "rerank":    {"applied": false, "candidates": 0, "reason": "unavailable"},
 "expansion": {"applied": true,  "variants": 2}}
```

`reason` is `not_configured`, `unavailable`, `no_candidates` or `no_variants`. A reranked passage
carries `rerank_score` (the cross-encoder logit) and, when known, `belief` (`current` or
`superseded`); `score` remains the first-stage cosine similarity. `min_score` gates first-stage
candidates, so a high `min_score` also shrinks what rerank can see.

**Where the services come from.** The reranker needs the sidecar started with
`LOCAL_INFERENCE_RERANK_MODEL` (see [`native/local-inference/README.md`](../native/local-inference/README.md#reranker-v1rerank)).
LCG reaches it on the embedder's sidecar by default; `--reranker-uds`/`--reranker-http` point it
elsewhere. Query expansion defaults to the sidecar's `/v1/chat/completions` route (Apple Foundation
Models); point it at another OpenAI-compatible model (for example a local Qwen) with
`--expander-uds`/`--expander-http` or `LCG_EXPANSION_URL` + `LCG_EXPANSION_MODEL`. Apple Foundation
Models' suitability for query rewriting is unmeasured.

"The embedder's sidecar" means its Unix socket (`--embedder-uds`, or the default socket when it
exists) or a **loopback** HTTP URL (`localhost`, `127.0.0.0/8`, `::1`). An embedding URL that points
at any other host is never reused for rerank or expansion, because it may be a third-party provider
chosen for embeddings alone and rerank would send it passage text. To use a remote host for either
stage, name it explicitly with `--reranker-http`, `--expander-http` or `LCG_EXPANSION_URL`.

**Endpoints are resolved once, at startup.** If the service starts before the sidecar (or the default socket does not exist yet), it has no reranker or expander for its lifetime and opted-in calls report `reason: "not_configured"` until it is restarted with the sidecar running, or given an explicit `--reranker-*` / `--expander-*` endpoint.

**Authentication.** The reranker client sends no credential: there is no `LCG_RERANK_API_KEY`, so
`--reranker-http` is for an unauthenticated endpoint on the local machine or a network you trust.
Do not point it at a public or credentialed service: it receives up to `LCG_RERANK_CANDIDATES`
(default 50, max 50) full passage bodies per reranked call, and an `http://` URL sends them in
clear text. Only expansion takes a key (`LCG_EXPANSION_API_KEY`), and it goes to the expansion
endpoint alone. Either remote endpoint is reached only by a call that opts in.

A malformed stage setting (`LCG_RERANK`, `LCG_RERANK_CANDIDATES`, `LCG_RERANK_*_FACTOR/WEIGHT`, a
non-numeric `LCG_*_TIMEOUT_MS`) stops the service at startup with an error naming the variable. An
endpoint that is merely unreachable does not: the call degrades and reports `applied: false`.

**What "belief-aware" means.** A passage whose episode has been superseded (a newer revision of the
document, a correction — see [belief time](adr/0705-belief-time-separate-from-validity.md)) has its
rerank score multiplied by `LCG_RERANK_SUPERSEDED_FACTOR`, and is always placed below its replacement
when both are among the candidates. Notes with no supersession record get a small recency preference
(`LCG_RERANK_RECENCY_WEIGHT`) so that of two equally good matches the later note ranks first. A
document superseded with a *future* effective date is still current until then.

## Extractor: local or hosted

**An extraction provider is required only for extraction operations, not for startup.** A
deployment with no `ANTHROPIC_API_KEY`, `--extractor-uds`/`--extractor-http`, or
`LCG_EXTRACTION_URL` configured starts normally and serves every read-only method
(`knowledge_find_entities`, `knowledge_find_relationships`, `knowledge_search_passages`,
`knowledge_status`, etc.) as well as `knowledge_rebuild_from_wal` — none of these touch the
extractor. Only `knowledge_process_chunk`, `knowledge_add_episode`,
`knowledge_reprocess_entity_types`, and `knowledge_reprocess_relation_types` require a
configured provider; each returns a clear, actionable error naming what to configure if called
without one, and the process keeps running and serving reads afterward (see
[ADR 0331](adr/0331-lazy-extraction-provider-validation.md)). This makes read-only deployments —
an MCP client that only queries and rebuilds from a published WAL, for example — a fully
supported configuration with no fake credential or placeholder endpoint required.

Entity/relationship extraction runs against one of two providers, selected with the following
precedence (highest first):

1. **Explicit CLI flag** — `--extractor-uds <path>` or `--extractor-http <url>` always selects the
   local OpenAI-compatible adapter, regardless of whether `ANTHROPIC_API_KEY` is set:

   ```bash
   liminis-context-graph --extractor-uds /tmp/liminis-inference.sock              # Unix domain socket
   liminis-context-graph --extractor-http http://127.0.0.1:8765/v1/chat/completions  # HTTP
   ```

   (`--extractor-uds` and `--extractor-http` are mutually exclusive.)
2. **`ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` set, no explicit flag** — extraction uses the
   Anthropic Messages API (hosted, or a gateway — see
   [Anthropic-compatible gateways](#anthropic-compatible-gateways-bedrock-openrouter-litellm)),
   unchanged from prior versions when no gateway setting is present. A reachable local sidecar
   never silently redirects this traffic. One guard applies to `ANTHROPIC_AUTH_TOKEN` alone: if it
   is the only Anthropic signal (no `ANTHROPIC_API_KEY`, no `ANTHROPIC_BASE_URL` /
   `LCG_ANTHROPIC_ENDPOINT` / `--anthropic-*` flag) **and** `LCG_EXTRACTION_URL` is set, the local
   extractor stays selected and a startup warning says the token was not used — the token is
   commonly exported for other Anthropic tooling and must not silently reroute a configured local
   extractor to `api.anthropic.com`. Add a gateway URL setting (or unset `LCG_EXTRACTION_URL`) to
   use the token.
3. **Neither of the above** — `LCG_EXTRACTION_URL` (HTTP), if set, else no extraction provider is
   configured: the binary still starts, and extraction-dependent calls fail with a clear error
   identifying the missing configuration when actually invoked (see above).

Unlike the embedder, extraction has **no default-socket auto-detection tier**: a running sidecar
alone never selects it for extraction, even with no `ANTHROPIC_API_KEY` set. This is deliberate,
not an oversight. Extraction requires an explicit signal — a CLI flag or `LCG_EXTRACTION_URL` —
before it will use a local endpoint at all.

> **The bundled sidecar's model is not recommended for extraction quality.** Prior evaluation
> found Apple Foundation Models' context window and capability insufficient for reliable
> entity/relationship extraction (see
> [Extraction-Quality Evaluation](extraction-quality-evaluation.md) for the full evaluation,
> methodology, and model rankings). All figures there describe **freeform extraction only** — the same
> corpus/backends run under an ontology (`Open`/`Strict`) are not measured there; see
> [Testing & Evaluation](testing-and-evaluation.md#running-under-an-ontology-openstrict) if you
> want to produce those figures yourself. For local extraction that meets a reasonable quality
> bar, run a model such as `qwen3.6-27b` behind an OpenAI-compatible server (e.g. `mlx_lm.server`)
> and point `--extractor-http`/`--extractor-uds` at it, or set `ANTHROPIC_API_KEY` to use the
> hosted baseline. The bundled sidecar's `/v1/chat/completions` route is still reachable for
> extraction if you want it anyway — pass `--extractor-uds /tmp/liminis-inference.sock`
> explicitly — the engine just never picks it for you.

The resolved choice is reported in a startup log line: `extractor: provider=...,
transport=..., endpoint=...` — `provider` is `anthropic` or `local` depending on which path was
selected. On the Anthropic path the line also names the auth scheme and any non-default request
shape, e.g. `extractor: provider=anthropic, transport=http, endpoint=https://gw.example.com/dep/v1/invoke, auth=bearer, model_in_body=false, anthropic_version_in=body`
(`auth` is `x-api-key` or `bearer`, never the value; userinfo is stripped from the URL and a
query string is shown as `?<redacted>`). Unlike the embedder, extraction performs no live reachability probe at startup —
Foundation Models' on-device warm-up can be slow, and there is no response shape to auto-detect —
so an unreachable local endpoint surfaces as an error on the first extraction call rather than at
startup. See [ADR 0041](adr/0041-local-openai-compatible-extraction-adapter.md) for the full
design, including why the local adapter uses `response_format: json_object` rather than
function-calling (the bundled sidecar has no `tools`/`tool_choice` support).

### Anthropic-compatible gateways (Bedrock, OpenRouter, LiteLLM)

The Anthropic provider can be pointed at any gateway that speaks the Anthropic **Messages**
request/response shape — for deployments that cannot reach `api.anthropic.com` directly (for
example, AWS ECS reaching Claude only through Bedrock) or that centralize LLM access, billing and
routing in a proxy. Every Anthropic-backed call (extraction, entity/relation reclassification,
summary consolidation, LLM dedup judgement) uses the same configured URL, auth and body shape.
With no gateway setting present, requests are byte-for-byte what they have always been.

The separate OpenAI-shaped path (`--extractor-http`, `LCG_EXTRACTION_URL`) is unaffected by these
settings; for gateways that only speak `/v1/chat/completions`, keep using it.

**Plain proxy** (OpenRouter's Anthropic-compatible endpoint, LiteLLM in `anthropic` mode, a
corporate gateway that forwards `x-api-key`):

```bash
export ANTHROPIC_BASE_URL=https://litellm.internal.example.com   # POST …/v1/messages
export ANTHROPIC_API_KEY=sk-gateway-key                           # sent as x-api-key
liminis-context-graph --mcp-stdio
# or: liminis-context-graph --mcp-stdio --anthropic-base-url https://litellm.internal.example.com
```

**Bedrock-style gateway** (full per-deployment invoke URL, OAuth2-minted Bearer token, model
chosen by the URL, `anthropic_version` in the body):

```bash
export LCG_ANTHROPIC_ENDPOINT=https://gw.example.com/claude-haiku-deployment/v1/invoke  # used verbatim
export ANTHROPIC_AUTH_TOKEN="$(fetch-oauth2-token)"       # Authorization: Bearer …, no x-api-key
export LCG_ANTHROPIC_OMIT_MODEL=true                       # no top-level "model" in the body
export LCG_ANTHROPIC_VERSION=bedrock-2023-05-31
export LCG_ANTHROPIC_VERSION_IN=body                       # "anthropic_version" in the body, no header
liminis-context-graph --mcp-stdio
```

The response is parsed as the standard Anthropic Messages shape (`content[…]` blocks, including
`tool_use`), so no response-side setting exists. lcg does not mint OAuth2 tokens itself; an
external step (a wrapper script, an ECS sidecar) supplies `ANTHROPIC_AUTH_TOKEN`.

**Precedence.** URL: `--anthropic-endpoint` > `LCG_ANTHROPIC_ENDPOINT` (both verbatim) >
`--anthropic-base-url` > `ANTHROPIC_BASE_URL` (both bases) > the stock endpoint. Auth:
`ANTHROPIC_AUTH_TOKEN` (Bearer) > `ANTHROPIC_API_KEY` (`x-api-key`) when a gateway URL is configured; on the stock endpoint with an API key set, the API key is kept and the token is ignored (logged). Exactly one scheme is ever
sent.

**Validation is lazy.** Gateway settings are checked only when the Anthropic provider is actually
selected. An unparseable URL, a non-`http`/`https` scheme, a set-but-empty value or an
unsupported placement value then fails startup with an error naming the offending setting — it
never silently falls back to the default endpoint. If an explicit `--extractor-http`/`--extractor-uds`
(or `LCG_EXTRACTION_URL`) wins instead, gateway settings are not validated, and one log line names
which were present and ignored (names only, never values). The flags are rejected with `--connect`
(attached MCP mode): set them on the daemon.

**Your credential goes to the endpoint you choose.** With a non-default endpoint the startup log
says so. Credential values, URL userinfo and query strings never appear in logs, error messages
or telemetry.

#### Token expiry and the fallback model

lcg reads `ANTHROPIC_AUTH_TOKEN` once, at startup. A short-lived token that expires mid-run
surfaces as authentication failures (the gateway's response body is captured in the extraction
failure sidecar as for any non-2xx). If `LCG_EXTRACTION_LLM` names a `primary:fallback` pair, the
router treats such a failure as a primary failure and permanently switches to the fallback for the
rest of the process — but the fallback uses the same expired token, so it fails too. Use a
long-lived token, or an external refresher that restarts the process before expiry.

`lcg-eval` (the developer evaluation tool) does not honor these settings; its judge uses its own
fixed endpoint.
