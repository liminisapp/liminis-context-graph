# ADR-0853: Episode Source Identity and Exact Source Matching

**Status**: Accepted
**Date**: 2026-11-21
**Issue**: #853 (implements #829, #830)

## Context

Every chunk-derived episode records where it came from as one composed string,
`source_description = "{source_file}:{chunk_id}"`, and until this change that string was the only
record of an episode's source. A URL contains colons (the scheme, and sometimes a port), so the
string cannot reliably be split back into its two parts. Two source-scoped operations matched on
it loosely, and both gave wrong answers for URL sources (#829, #830, reported by a plugin that
uses page URLs as `source_file`):

- **`knowledge_delete_by_source`** (`Conn::remove_episodes_by_source`) matched
  `source_description = $src OR source_description STARTS WITH $src + ':'`. Deleting
  `https://a.com` therefore also hard-deleted every episode of `https://a.com:8443/...`. This is
  permanent data loss.
- **`knowledge_get_entities_by_source`** (`Conn::get_entities_by_source`) matched
  `source_description CONTAINS $src`, so `https://a.com/p` also returned the entities of
  `https://a.com/page`, `https://a.com/p?x=1` and `https://a.com/p/sub`. It had no live-episode
  filter either, so superseded episodes still counted.

## Decision

### 1. Two nullable `Episodic` columns: `source_file`, `chunk_id`

`Episodic` gains `source_file STRING` and `chunk_id STRING`, added by the probe-then-`ALTER`
migration exactly as #752 (`origin`), #292 (`write_path`) and #785 (document-mode columns) added
theirs. They are nullable, there is no storage-format version change, and they are listed in the
graphiti-parity divergence notes in `schema.rs`.

- **Written by** `knowledge_process_chunk` (`source_file` = the request's `source_file`) and
  document ingest (`source_file` = `source_path`, or `doc_id` when no path is given). Both pass
  `EpisodeOpts.source_file`; `add_episode_with_opts` stamps `source_file` and sets
  `chunk_id = name`. Both paths already pass the chunk ID as the episode name, so
  `source_description == source_file + ":" + chunk_id` holds by construction. `chunk_id` is
  derived, never a second caller-supplied value.
- **Not written by** `knowledge_add_episode`, the library `add_episode` or the eval runner. Those
  take a free-form description and no structured identity, so their episodes stay legacy rows.
- `source_description` is unchanged in content and format. It remains the display value.

**WAL.** Both `CREATE (:Episodic …)` templates (`insert_episodic` and the document-mode
`insert_document_episodic`) append `source_file: $source_file, chunk_id: $chunk_id` **last**, so
`exec_params` records the values and a rebuild restores them. The dump/compaction template
(`dump.rs` `EPISODIC_CYPHER`) and `dump_episodics_page` append both columns last too, following
the ADR-0785 convention, and map an empty value back to NULL so a legacy row round-trips as legacy.
A WAL written before this change replays its old template and leaves both columns NULL.

This deliberately changes the chunk-mode `CREATE` text, which ADR-0785 (FR-019) had kept
byte-identical *relative to document mode*. That property was about not letting document mode
perturb chunk mode; #752 and #292 changed the same template the same way. A separate `SET`
statement was rejected because it would double the WAL lines per episode for the sole benefit of a
0.19.1 *rebuild* of a post-change WAL, which FR-011 does not require (see §7).

### 2. The source of an episode: at most one, from its own name

`crate::source_identity::episode_source_file` maps every episode to **at most one** source file;
`episode_matches_source` matches when that source equals the request byte for byte.

| Row | Source file |
|---|---|
| `source_file` set (non-empty) | `source_file`, authoritative |
| legacy, `write_path` `chunk` / `document` | `source_description` minus the suffix `":" + name`; none if it does not end that way |
| legacy, `write_path` NULL / empty (pre-0.19.0) | `source_description` minus `":" + name` when it ends that way, otherwise `source_description` whole |
| legacy, `write_path` `episode` (raw `add_episode`) | `source_description` whole (exact equality only) |
| legacy, any other `write_path` | none (fail safe) |

Because each row has at most one source, it can never match two different requests, which rules
out over-matching by construction. The old predicates mapped one row to many sources.

**The split point comes from the row's own `name`, never from a colon.** For a chunk- or
document-path row the name *is* the chunk ID, so stripping `":" + name` recovers the source file
exactly, whatever colons the URL or the chunk ID contain. That covers ports, IPv6 literals,
`mailto:`/`urn:` forms, and document-mode chunk IDs (`{doc_id}::{hex}`, which always contain `::`).
ADR-0824's `own_source_ids` already derives the source file the same way.

**Rejected: splitting on the last colon.** It is wrong for every document-mode row and for any
chunk ID containing a colon.

**Rejected: exact `source_description = $src` only for NULL-`write_path` rows.** It is always
safe, but it would silently stop `delete_by_source` from working on every chunk episode written
before 0.19.0 (when `write_path` shipped). That is a large regression for existing users.

**Residual case, accepted.** A NULL-`write_path` row is treated like a chunk row when its
description is in composed form. That is the ADR-0292 precedent (`write_path_is_chunk_eligible`
treats NULL as chunk). The only row that can be misattributed is a pre-0.19.0 raw `add_episode`
row whose description is exactly `"<X>:<its own name>"`; it is attributed to `X`. That is the
project's own definition of the composed form, and the row is still attributed to exactly one
source. The whole-string fallback applies only when the composed form does **not** match, so a
chunk row `https://a.com` + chunk `8443` (`"https://a.com:8443"`) belongs to `https://a.com` and
does not also match a request for `https://a.com:8443`.

**`episode` rows get exact equality only.** Their name is known to be free-form, so stripping it
could attribute the row to a shorter source it never named (`"https://a.com:8443"` with name
`8443` would be read as `https://a.com`). This is the behaviour change for clients that wrote
`source_description = "file:section"` through `knowledge_add_episode` and relied on
`delete_by_source("file")` prefix-matching it. Those rows now match only `"file:section"`.

The legacy rule may under-match, and an under-matched row is never deleted by `delete_by_source`.
`knowledge_delete_chunk_episode` (by chunk ID) remains the way to remove one.

### 3. Where the rule runs: a Rust filter over a Cypher candidate superset

Both operations use one helper, `Conn::episode_uuids_for_source`. Its Cypher returns a candidate
superset:

```
ep.source_file = $src
OR ((ep.source_file IS NULL OR ep.source_file = '')
    AND (ep.source_description = $src OR ep.source_description STARTS WITH $src + ':'))
```

It returns `uuid, source_file, source_description, name, write_path`, and
`episode_matches_source` decides each row in Rust. Every legacy row the rule can attribute to
`$src` satisfies one of the two description predicates, so the candidate set is complete. The Rust
filter removes the over-matches. This needs no Cypher string concatenation, which is unverified in
lbug, and it keeps one pure, exhaustively unit-tested function as the single source of truth.

- **`remove_episodes_by_source`** keeps its structure: one `BEGIN TRANSACTION`; per group,
  candidate query, Rust filter, `DETACH DELETE` by uuid list, and `drain_mutations_into` for
  per-group WAL attribution (ADR-0402). Superseded episodes of the source are still deleted.
- **`get_entities_by_source`** runs in two steps. It first resolves the matching *live* episode
  uuids (no group filter). Then it fetches
  `MATCH (ep:Episodic)-[:MENTIONS]->(e:Entity) WHERE ep.uuid IN $uuids [AND e.group_id IN $gids]
  RETURN DISTINCT … LIMIT $limit`. Group filtering stays on the entity's group, as before, and
  `LIMIT` applies after the source filter, so the result is never under-filled. The response
  shape and the `get_episode_info_for_entities` enrichment are unchanged.

### 4. Live means believed now

`get_entities_by_source` counts an episode only if it is live under the chunk-lineage predicate
(`live_episode_uuids_for_chunk`, ADR-0705/0707) and its document revision, if any, has committed
(the exclusion `origins_for_doc` and the citation lookup already use, ADR-0785):

```
(ep.superseded_at IS NULL
 OR (ep.superseded_by_kind = 'document' AND ep.superseded_at > $live_at))
AND (ep.revision_id IS NULL OR ep.revision_committed_at IS NOT NULL)
```

`$live_at` is the service clock now. Without the revision clause, an aborted document ingest's
entities would be reported. Together the two clauses are equivalent to `belief::live_at` at the
current instant. An entity is returned when at least one live episode of the source mentions it.

### 5. Response fields

`source_file` and `chunk_id` are returned additively on `knowledge_get_episodes` items (also
projectable through `fields`), on `knowledge_search_passages` passages (vector, FTS and hybrid
paths), and on `knowledge_get_affected_work` candidates. They are omitted for a legacy row. No
existing field is removed or changed, and request shapes are unchanged, so the MCP tool surface
and the Python IPC protocol stay compatible.

### 6. No backfill, no index

**No backfill.** At read time the rule already applies exactly the derivation a backfill would
store, so a backfill would change no match result. Its only benefit would be filling the response
fields on legacy rows, and FR-013 allows those to be absent. An on-open backfill would add
per-group WAL attribution, open-time cost and WAL growth. An admin method would add a tool, a
write-policy entry and scope-table rows. Either can follow if clients ask for the fields on old
rows.

**No ART index on `source_file`.** While legacy rows exist, the candidate query still has to scan
`source_description` with `STARTS WITH`, so an index gains nothing asymptotically. It would also
add an index to the catalog that 0.19.1 has never seen. ADR-0291's pattern applies if a follow-up
wants one once legacy rows are rare.

### 7. Upgrade and downgrade

- **Upgrade:** a 0.19.1 database gains the two columns on open, NULL on every existing row. The
  lbug pin is unchanged since v0.19.1, so the storage format is identical.
- **Rollback, open and operate:** 0.19.1 names its columns explicitly in every statement, so it
  opens the upgraded database and reads, writes and deletes normally, ignoring the new columns.
  `tests/source_identity.rs` reproduces that path in-tree with v0.19.1's DDL, insert template,
  episode read and delete-by-source match.
- **Rollback, rebuild:** 0.19.1 cannot *rebuild* from a WAL this release wrote. Every
  post-change `CREATE (:Episodic {… source_file …})` fails to prepare on its schema, and under
  batched replay that drops every episode written after the upgrade. The #752, #292 and #785
  columns already carry the same property. It is documented in the CHANGELOG, not fixed.

### 8. Chunk-level removals checked (FR-017)

`knowledge_mark_chunk_removed` (`supersede_chunk_episodes`) and `knowledge_delete_chunk_episode`
(`remove_episodes_by_chunk_id`) match `ep.name = $name` exactly. They do not have the
loose-matching defect, so they are unchanged. They do have a separate property, recorded as a
possible follow-up: a chunk ID reused under two different source files collides, because matching
is by name alone. Chunk revise-supersession (`knowledge_process_chunk` with the default
`supersede: true`) shares that property.

## Consequences

- Deleting a URL source can no longer delete another source's episodes, and entities-by-source
  returns only that source's live entities.
- Clients read `source_file`/`chunk_id` directly instead of splitting the display string.
- **Behaviour change:** clients that relied on prefix matching in `delete_by_source`, or substring
  matching in `get_entities_by_source`, see fewer matches. In particular, a raw
  `knowledge_add_episode` row with `source_description = "file:section"` is no longer matched by
  `"file"`.
- Some legacy rows (e.g. a chunk-path row whose description does not end in `":" + name`) are never
  attributed, so `delete_by_source` reports `deleted_count: 0` for them.

## Out of scope / follow-ups

- The #824 citation-target lookup (`episode_id_rows_for_target`) still uses substring predicates on
  `source_description` as a pre-filter. It compares exactly afterwards and is a separate feature.
- The cross-source chunk-ID collision in the by-name chunk operations (§8).
- A backfill or an index on `source_file` (§6).
