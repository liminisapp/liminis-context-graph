---
layout: default
title: Integrator Guide
---

# Integrator Guide

This guide is for a developer building a **client** of `liminis-context-graph` (LCG): something
that ingests text, searches it, shows what an entity is connected to, and removes content again.
LCG has about 60 methods; a client like that needs nine of them. This page names those nine,
shows how to connect, and says which parameters to set. It links to the
[IPC & MCP Reference](ipc-mcp-reference.md) for every field instead of repeating it, so the
reference stays the one place a response shape is defined.

Before you build anything, check the list of methods below. Most of the features a first client
tries to rebuild already exist: deleting by source (`knowledge_delete_by_source`), ingesting a
whole page (`knowledge_process_document`), per-episode `attributes` and `title`, and listing the
entities of a source (`knowledge_get_entities_by_source`).

## The minimal client profile

These nine methods are the **minimal client profile**. LCG keeps their names, required
parameters, result fields and error codes compatible across releases
([ADR-0862](adr/0862-minimal-client-profile-compatibility.md)). Everything else may change in a
minor release; build against these and you do not need to track it.

| Method | Use it to | Scope |
|---|---|---|
| `knowledge_capabilities` | Ask the running server what it is and what it supports. | read |
| `health_check` | Check that the server is alive, and ready for reads or writes. | not an MCP tool |
| `knowledge_process_chunk` | Ingest one chunk of text (and re-send it to update it). | write |
| `knowledge_process_document` | Ingest a whole document or web page, and re-send it to update it. | write |
| `knowledge_retire_document` | Stop believing a document that no longer exists, without deleting it. | write |
| `knowledge_delete_by_source` | Destroy everything ingested from a source. | write |
| `knowledge_search_passages` | Search the ingested text. | read |
| `knowledge_find_entities` | Search entities by name or meaning, and get the `uuid` that `knowledge_get_entity_neighbors` needs. | read |
| `knowledge_get_entity_neighbors` | Show an entity's connected facts and neighbouring entities. | read |

`health_check` is dispatched on the socket but is **not** an MCP tool, so an MCP client cannot call
it; it uses `knowledge_status` and `knowledge_capabilities` instead (see [Health](#health)).

The same list, in the form the compatibility test reads. Do not edit one without the other; the
test fails when this block and the test's list disagree.

<!-- minimal-profile:begin -->
```text
health_check
knowledge_capabilities
knowledge_delete_by_source
knowledge_find_entities
knowledge_get_entity_neighbors
knowledge_process_chunk
knowledge_process_document
knowledge_retire_document
knowledge_search_passages
```
<!-- minimal-profile:end -->

What the guarantee covers, and what it does not, is in the ADR. In short: method names, required
parameters, the names and types of result fields and the documented error codes (for example
`document_too_large`) stay; new optional parameters and new result fields may be added at any
time, so **ignore fields you do not know**.

## Connecting

**Socket.** The service listens on `<workspace>/.lcg/service.sock`, a Unix domain socket. On
Windows it is a named pipe whose name is written to `.lcg/service.endpoint`
([Windows: named pipe](ipc-mcp-reference.md#windows-named-pipe)). `LCG_SOCKET_PATH` overrides the
location. The protocol has no authentication; anything that can open the socket can use it.

**Framing.** [JSON-RPC 2.0](https://www.jsonrpc.org/specification), one JSON object per line, in
each direction, terminated by `\n`. A request needs `jsonrpc`, `id`, `method` and `params`:

```python
import json, socket

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(".lcg/service.sock")
f = s.makefile("rw", encoding="utf-8", newline="\n")

def call(method, params=None, id=1):
    f.write(json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params or {}}) + "\n")
    f.flush()
    reply = json.loads(f.readline())
    if "error" in reply:
        raise RuntimeError(reply["error"])
    return reply["result"]
```

A failure comes back as a JSON-RPC `error` object with a `message`; some messages begin with a
stable code, such as `document_too_large:` or `empty_document:`.

**One request at a time per connection.** The service answers the requests on a connection
strictly in order. A search sent on the same connection as a slow ingest waits behind it. Open a
second connection for reads, and one connection per concurrent ingest. Connections do not block
each other. See
[Connections, concurrency and timeouts](ipc-mcp-reference.md#connections-concurrency-and-timeouts-issue-861).

**Timeouts.** Ingest time is dominated by the embedder and the LLM, so there is no fixed bound.
Set the client timeout to the slowest ingest you expect, or send `"_progress_token": "x"` with
`knowledge_process_chunk` or `knowledge_process_document` and treat silence between lines as the
timeout: the service streams `{"type":"progress",...}` lines and a `heartbeat` about every 10
seconds, then the terminal result. If a call times out, **close the connection** and retry on a
new one: closing cancels the original before its commit, and an identical resubmission is a no-op
(see [Ingest, update, retire, delete](#ingest-update-retire-delete)). Keep reading while a
request runs; a client that stops reading is treated as disconnected.

An MCP client uses `--mcp-stdio` instead of the socket and gets the same methods as tools; the
rest of this guide applies unchanged
([MCP-over-stdio transport](ipc-mcp-reference.md#mcp-over-stdio-transport)).

## Discover the server

Call `knowledge_capabilities` first. It takes no parameters, reads no database and answers even
when the service is degraded:

```json
{"server_version": "0.19.1", "protocol_version": 1,
 "features": ["cancel_on_disconnect", "…"], "methods": ["health_check", "…"]}
```

- `protocol_version` changes only on a breaking protocol change. Refuse to run against a version
  you do not know.
- `features` is a sorted, append-only list of stable strings. Test for a feature before you use
  what it adds, for example `process_document`, `retire_document`, `entity_provenance`,
  `walk_provenance`, `unchanged_noop` and `document_max_bytes_env`. A missing feature means an
  older server: fall back or tell the user.
- `methods` lists every method the dispatcher accepts, so a client never has to probe with a
  trial call.

The feature table is in [Capabilities](ipc-mcp-reference.md#capabilities).

## Groups

A **group** (`group_id`) is a separate graph inside one workspace, with its own entities, episodes
and WAL stream. **Always pass your own `group_id`.** The default group `liminis` is the Liminis
app's own graph: anything you write there is entity-resolved together with the user's notes.
Omitting `group_id` on a write still works, goes to `liminis` and returns an
`implicit_default_group_warning`, so you see it in testing.

What an omitted group means differs by method; for the profile methods:

| Method | Group parameter | If you omit it |
|---|---|---|
| `knowledge_process_chunk` | `group_id` | Writes to `liminis`, with a warning. |
| `knowledge_process_document` | `group_id` | Writes to `liminis`, with a warning. |
| `knowledge_retire_document` | `group_id` | **Refused.** Required. |
| `knowledge_delete_by_source` | `group_ids` (array) | **Refused.** Required and non-empty; there is no all-groups delete. |
| `knowledge_search_passages` | `group_ids` (array) | Searches **every** group. |
| `knowledge_find_entities` | `group_ids` (array) | Searches **every** group. |
| `knowledge_get_entity_neighbors` | `group_ids` (array) | Every group. |
| `knowledge_capabilities`, `health_check` | none | Takes no group. |

So: writes take a singular `group_id`, reads and deletes take a plural `group_ids` array, and a
read that omits it spans every group. Pass `group_ids: ["your-group"]` on every read, or you will
see other clients' data. An explicit `group_id: ""` is a distinct group, not the default. The
full table for all methods is
[Omitted-group behaviour per method](ipc-mcp-reference.md#omitted-group-behaviour-per-method-issue-856).

An operator can start the service with `--writable-groups=<your-group>` (or
`LCG_WRITABLE_GROUPS`) so that a write to any other group, including the defaulted one, is
refused ([ADR-0742](adr/0742-per-group-write-allow-list.md)). A refused write returns error code
`-32004`.

## Ingest, update, retire, delete

The four operations side by side. In every row the group is the one you ingested into.

| You want to | Call | What happens | Reversible? |
|---|---|---|---|
| **Ingest** a whole document or web page | `knowledge_process_document` with `doc_id`, `content`, `group_id` | The server chunks it by section, extracts each section, and records a revision. | Yes: retire or delete it. |
| **Ingest** one chunk you cut yourself | `knowledge_process_chunk` with `chunk_text`, `chunk_id`, `source_file`, `group_id` | One episode. Facts are extracted and written. | Yes. |
| **Update** a document | `knowledge_process_document` again, with the same `doc_id` and the new full `content` | Only changed sections are re-extracted, the old versions are superseded, and sections you removed are retired. An identical resend makes no extraction call (`status: "unchanged"`). | Yes: history is kept. |
| **Update** a chunk | `knowledge_process_chunk` again, with the same `chunk_id` and new text | The earlier episode is superseded once the new one succeeds. An identical resend is a no-op (`status: "unchanged"`). | Yes: history is kept. |
| **Retire** a document that no longer exists | `knowledge_retire_document` with `group_id`, `doc_id` | Every live episode of it is superseded (`document_removed`). The episodes and facts stay readable as history; searches stop believing them. Idempotent. | Nothing is deleted; the history stays. |
| **Delete** a source for good | `knowledge_delete_by_source` with `source_file`, `group_ids` | The episodes whose stored `source_file` equals the value exactly, superseded ones included, are destroyed. Returns the entities it left stranded. | **No.** |

How to choose:

- **Prefer `knowledge_process_document` for anything longer than a few paragraphs**, including
  web pages. You send the whole document every time; you do not chunk, diff or track what
  changed. Use `knowledge_process_chunk` only when you already have natural chunks with stable
  ids (a chat message, a log entry). A chunk over about 8,000 characters still succeeds but
  returns a `warning` that points you to `knowledge_process_document`.
- **Update is just ingest again.** There is no separate update call. Re-send the full document and
  let the server work out what changed.
- **Retire when the content is gone but the history matters**; **delete when the record itself
  must go** (a privacy request, a mistake). Retiring deletes nothing; deleting cannot be undone.
- `knowledge_delete_by_source` needs `group_ids`, matches `source_file` byte for byte (deleting
  `https://a.com` leaves `https://a.com:8443/x` alone), and by default leaves the entities that
  lost their last mention in place. They are returned as `orphaned_entity_uuids`; pass
  `"prune_orphans": true` to remove them in the same call
  ([Deletion](ipc-mcp-reference.md#deletion-delete_chunk_episode-delete_by_source)).
- A document is identified by `doc_id`, a chunk by `chunk_id`, and a source by `source_file`.
  `knowledge_process_document` uses `source_path` (or `doc_id`) as the episode's source, so
  `delete_by_source` on that value removes the document's episodes. Pick ids you can reproduce.

**Document size.** `knowledge_process_document` accepts up to **2 MiB** (2,097,152 bytes of
UTF-8) by default, enough for a web page of about 600,000 characters. A larger document is
**refused** with `document_too_large`, the message names the cap and `LCG_DOCUMENT_MAX_BYTES`, and
nothing is written or truncated. The operator raises or lowers the cap with that variable
(`4MiB`, `512KiB` or a plain byte count, up to 256 MiB), which is checked once at startup. A
client cannot change it; detect the feature with `document_max_bytes_env` and cut the document at
a section boundary yourself if you must stay under a smaller cap. See
[Configuration](configuration.md) and [Document mode](ipc-mcp-reference.md#document-mode-send-the-whole-document-the-server-does-the-diff-issue-785).

A minimal ingest, update and delete:

```python
call("knowledge_process_document", {
    "group_id": "my-app", "doc_id": "page-42", "title": "Release notes",
    "source_path": "https://example.com/notes", "content": page_text,
    "attributes": '{"crawl": "2026-11-23"}',
})
call("knowledge_process_document", {   # later: the page changed, send it all again
    "group_id": "my-app", "doc_id": "page-42", "title": "Release notes",
    "source_path": "https://example.com/notes", "content": new_page_text,
})
call("knowledge_delete_by_source", {
    "group_ids": ["my-app"], "source_file": "https://example.com/notes",
})
```

## Search

Two searches, for two questions.

**Passages: "which text says this?"** `knowledge_search_passages` ranks the ingested text.

| Parameter | Recommendation |
|---|---|
| `query` | Required. A natural-language question or phrase. |
| `group_ids` | **Always set**, for example `["my-app"]`. Omitted means every group. |
| `num_results` | Default 10, at most 100. Ask for what you will show. |
| `min_score` | The floor on the vector cosine similarity (0.0 to 1.0). Omit it to use the server default, `LCG_SEARCH_MIN_SCORE`, which is **0.5** when unset. A model that scores relevant text high and unrelated text only slightly lower needs a higher floor (0.7 or so); the operator who chose the embedder sets it for everyone, and your explicit value, including `0.0`, always wins. `knowledge_status` reports the effective default as `search_min_score_default`. |
| `mode` | `vector` or `hybrid`; omit it to use the server default. In `hybrid`, `score` is a rank-only value (at most about 0.023) and is **not** a relevance. Use `similarity` (the cosine, present when the vector leg matched) and `legs` to judge a hit. `min_score` does not stop lexical-only hits. |

The result is `{passages, count}`. Each passage carries the `attributes` and `origin` of its
episode, so there is no second fetch for them. Superseded passages are still returned (ranked
below their successor); see [Hybrid passage search](ipc-mcp-reference.md#hybrid-passage-search-knowledge_search_passages-issue-789).

**Entities: "what do I know about X?"** `knowledge_find_entities` finds entities by name or
meaning and returns each one's `uuid`. Set `group_ids` and `num_results` (`num_results`, not
`limit`; an unknown key is ignored and the default of 10 applies). Add **`include_episodes:
true`** to get, for each entity, the live episodes that mention it: `episode_uuids`,
`source_descriptions` and `source_files` (at most 50 per entity, newest first; `episodes_truncated`
says when it was cut). Without the flag those fields are empty lists, which is the single most
common thing integrators trip over. Feature `entity_provenance`
([Entity provenance](ipc-mcp-reference.md#entity-provenance-which-episodes-mention-an-entity-issue-857-feature-entity_provenance)).

**Neighbours: "what is it connected to?"** `knowledge_get_entity_neighbors` takes the `entity_uuid`
from `knowledge_find_entities`, plus `group_ids` and `num_results` (default 50), and returns the
facts (edges) around it and the entities at their other ends. Unlike `find_entities
include_episodes`, its `episode_uuids` include superseded episodes, and an edge's list is every
episode that mentions either endpoint, not evidence for that one fact.

```python
hits = call("knowledge_find_entities",
            {"query": "release schedule", "group_ids": ["my-app"], "num_results": 5,
             "include_episodes": True})
entity = hits["nodes"][0]
around = call("knowledge_get_entity_neighbors",
              {"entity_uuid": entity["uuid"], "group_ids": ["my-app"], "num_results": 20})
passages = call("knowledge_search_passages",
                {"query": "when is the next release?", "group_ids": ["my-app"],
                 "num_results": 5, "min_score": 0.6})
```

`knowledge_find_entities` and `knowledge_get_entity_neighbors` return `nodes` (and `count`; the
neighbours call also returns `edges`, `center_uuid`, `node_count` and `edge_count`). The snippets
show the call shape; the response schemas are in
[Response contract](ipc-mcp-reference.md#response-contract-json-schemas-issue-852).

## Provenance and citations

Every fact traces back to the text it came from, so a client can cite its sources.

- **Source identity.** Each episode stores the `source_file` and `chunk_id` it was ingested with
  (feature `episode_source_fields`). A passage result carries them (and so do the items of
  `knowledge_get_episodes`), so a citation is the `source_file` plus the passage text
  ([Source identity](ipc-mcp-reference.md#source-identity-source_file-and-chunk_id-issue-853)).
- **Entity to episodes.** `knowledge_find_entities` with `include_episodes: true` gives the live
  episodes behind each entity (above). Turn the UUIDs into episodes with `knowledge_get_episodes`
  and its `uuids` parameter.
- **Fact to episodes.** `knowledge_walk` with `include_provenance: true` adds, to every edge,
  `episodes` (the live producing episodes, at most 20), `source_files` (the distinct sources of
  *all* live producers) and `episodes_truncated`. Check the `walk_provenance` feature first
  ([Provenance](ipc-mcp-reference.md#provenance-include_provenance)). The walk is outside the
  minimal profile.
- **Document spans.** An episode ingested through `knowledge_process_document` carries
  `revision_id`, `byte_start` and `byte_end`: `content[byte_start..byte_end]` of the revision is
  the episode's text, and `heading_path` locates it, so a citation can link to a section.

Superseded and retired episodes are never listed as live provenance. To show what was believed
before a change, read the history reads with `as_of`
([Belief time](ipc-mcp-reference.md#belief-time-corrections-supersede-they-do-not-delete-issue-705)).

## Health

Run these checks, in this order, before you send work and while you run.

1. **Connect, then ask `health_check`.** A successful `connect()` does **not** mean the service is
   ready: the socket is bound before the database opens. Poll `health_check` until the state you
   need.
2. **Read three fields**, each a different fact
   ([Health check contract](ipc-mcp-reference.md#health-check-contract),
   [ADR-0855](adr/0855-health-check-ready-for-reads.md)):

   | Field | Gate on it for | Meaning |
   |---|---|---|
   | `ok` | Restart decisions and liveness probes. | The process is alive and answered. |
   | `ready_for_reads` | Sending reads. | The database is loaded. `true` even while `busy`. |
   | `healthy` | Sending writes. | The database is loaded and no write is pending or running. It is **not** a liveness signal. |

   `state` is `healthy`, `busy` (alive, serving reads, a write is running, such as a rebuild;
   keep waiting for writes and do not restart) or `degraded` (alive but the database did not
   load; see `reason`). **No answer** means dead or wedged: restart it.
3. **Do not retry a `degraded` state forever.** It can be a real outcome, such as unrecovered
   corruption; the operator recovers it ([Operations](operations.md)).
4. **MCP clients** have no `health_check` tool. Call `knowledge_capabilities` for liveness and
   `knowledge_status` for readiness: wait until `connected` and `queryable` are `true` and
   `initializing` is `false`.

Every `health_check` response also carries `server_version` and `protocol_version`, so one call
is enough to confirm you are talking to a server you support.

## Where to go next

- [IPC & MCP Reference](ipc-mcp-reference.md): every method, field and error.
- [Configuration](configuration.md): every environment variable and flag, including
  `LCG_SEARCH_MIN_SCORE` and `LCG_DOCUMENT_MAX_BYTES`.
- [Test Providers](test-providers.md): `--embedder test --extractor test`, to test your client
  without an LLM or a network.
- [Operations](operations.md): the WAL, degraded mode and recovery.
