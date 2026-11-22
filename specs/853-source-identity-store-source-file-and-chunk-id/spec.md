# Feature Specification: Source identity — store source file and chunk ID separately; exact matching in delete-by-source and entities-by-source

**Feature Branch**: `fabrik/issue-853`
**Created**: 2026-11-21
**Status**: Specified
**Input**: User description: "Implements #829 and #830.

## Problem
Each episode stores its source as a single string, `source_description = \"{source_file}:{chunk_id}\"` (`crates/core/src/handlers.rs` ~1219). URLs contain `:` (the scheme and any port), so the string can't be split back into its two parts. Two lookups match on it loosely:
- **`knowledge_delete_by_source`** (`remove_episodes_by_source`, `crates/core/src/db.rs` ~1546–1549) matches `= $src OR STARTS WITH \"$src:\"`. Deleting `https://a.com` therefore also **hard-deletes** every episode from `https://a.com:8443/...`. This is data loss.
- **`knowledge_get_entities_by_source`** (`db.rs` ~3585/3592) matches `CONTAINS $src`. So `https://a.com/p` also returns entities from `https://a.com/page`, `https://a.com/p?x=1` and `https://a.com/p/sub`. It also has no live-episode filter, so superseded episodes count.

## Change
- **New fields.** Add nullable `Episodic.source_file` and `Episodic.chunk_id` columns with `ALTER TABLE ... ADD`, following the #752 and #292 pattern. No storage-version change. Write both on every chunk and document ingest path. Keep `source_description` unchanged as the display value.
- **WAL.** The new fields go into the WAL records, so a rebuild restores them. A WAL from before this change replays with them NULL.
- **Exact matching.** `remove_episodes_by_source` and `get_entities_by_source` match on `source_file = $src` exactly. Legacy rows, where `source_file` is NULL, fall back to today's behaviour, but `delete_by_source` must never delete a NULL-`source_file` row whose `source_description` merely starts with `$src:` when `$src` contains `:`. Decide the safest legacy rule and record it in an ADR. Splitting on the last `:` is one option, and refusing ambiguous legacy matches is another.
- **Live only.** `get_entities_by_source` counts live (not superseded) episodes only.
- **Responses.** Return `source_file` and `chunk_id` additively wherever episodes and passages are returned (`knowledge_get_episodes`, `knowledge_search_passages` and the like).
- **Backfill.** Provide an idempotent admin backfill, or an on-open backfill, for legacy rows where the split is unambiguous. It's optional; justify the choice in the ADR.

## Acceptance
- Ingest `https://a.com` and `https://a.com:8443/x`, then call `delete_by_source(\"https://a.com\")`. It removes only the first source's episodes.
- Ingest `https://a.com/p` and `https://a.com/page`, then call `get_entities_by_source(\"https://a.com/p\")`. It returns only `/p`'s entities.
- A superseded episode's entities aren't returned.
- A rebuild from the WAL preserves the new fields, and a 0.19.1 binary still opens the upgraded database and ignores the new columns. Include an upgrade test.
- The ADR, CHANGELOG (under Fixed, flagged as a behaviour change for clients that relied on substring matching), docs and `llms-full.txt` are updated.

## Out of scope
Changes to `knowledge_mark_chunk_removed` or `knowledge_delete_chunk_episode` matching, unless they share the same bug. If they do, fix them here and say so."

## Background

Two community bug reports from WebBrain's Liminis plugin (#829, #830) describe one root cause. The plugin ingests web pages and uses the page URL as the source file.

Every chunk-derived episode records where it came from as one composed string, `"<source_file>:<chunk_id>"`. That string is the only record of an episode's source. A URL contains colons (the scheme, and sometimes a port), so the string cannot reliably be split back into its two parts. Clients have to split on the last colon and hope the chunk ID contains none.

Two source-scoped operations match on this composed string loosely, and both give wrong answers for URL sources:

- **Delete by source loses data.** It matches an episode when the string equals the source or starts with `"<source>:"`. For `https://a.com`, the prefix `https://a.com:` also matches every episode from `https://a.com:8443/...`. Deleting one site therefore **permanently deletes** another site's episodes. This is the most serious defect here, because the loss cannot be undone.
- **Entities-by-source over-matches.** It matches any episode whose string *contains* the source. A query for `https://a.com/p` also returns entities from `https://a.com/page`, `https://a.com/p?x=1` and `https://a.com/p/sub`. It also does not exclude superseded episodes, so entities that only older, replaced versions of a source supported are still returned.

The fix is to give each episode a structured source identity: the source file and the chunk ID as separate values. The composed string stays as a display value. Source-scoped operations then match on that identity exactly. Earlier schema additions in this project (#752 episode origin, #292 chunk lineage) added nullable episode columns without a storage-format version change. This change follows the same approach, so databases stay readable by the previous release.

Episodes written before this change carry no separate source identity, so a rule is needed for those **legacy** rows. The rule must be safe: when a legacy row's source cannot be determined unambiguously, a destructive operation must not delete it. The Plan stage chooses the rule and records it in an ADR (ADR-0853, numbered by this issue per the project's ADR convention).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Deleting one URL source never deletes another (Priority: P1)

A client ingests content from several web pages, including two URLs where one is a colon-prefix of the other (`https://a.com` and `https://a.com:8443/x`). It then deletes by source for `https://a.com`. Only that source's episodes are removed. The other source's episodes are untouched.

**Why this priority**: The current behaviour silently and permanently deletes unrelated data. Of the defects here, it is the only one that destroys data.

**Independent Test**: Ingest one chunk for each of the two URLs. Delete by source for the shorter URL. Confirm the first source has no episodes left and the second source still has all of its episodes.

**Acceptance Scenarios**:

1. **Given** episodes ingested from `https://a.com` and from `https://a.com:8443/x`, **When** the client deletes by source `https://a.com`, **Then** only the `https://a.com` episodes are removed, and every `https://a.com:8443/x` episode remains.
2. **Given** a non-URL source such as `notes/today.md` with several chunks, **When** the client deletes by source `notes/today.md`, **Then** all of that source's episodes are removed, as they are today.
3. **Given** legacy episodes written before this change, whose source cannot be shown unambiguously to be the requested source, **When** the client deletes by source, **Then** those episodes are not deleted.
4. **Given** sources `notes/a` and `notes/ab`, **When** the client deletes by source `notes/a`, **Then** no `notes/ab` episode is removed.

---

### User Story 2 - Entities-by-source returns only that source's live entities (Priority: P1)

A client asks which entities came from a particular page. It gets back exactly the entities supported by live episodes of that source. It does not get entities from sources whose identifier merely contains the requested one. It does not get entities that only superseded episodes supported.

**Why this priority**: Plugins use this to show provenance and to drive per-page actions. Over-matching shows users wrong information and can send follow-up actions to the wrong entities.

**Independent Test**: Ingest distinct content from `https://a.com/p` and `https://a.com/page`, then query entities-by-source for `https://a.com/p`. Then re-ingest `https://a.com/p` so that its earlier episode is superseded, and query again.

**Acceptance Scenarios**:

1. **Given** episodes from `https://a.com/p` and `https://a.com/page`, **When** the client queries entities-by-source for `https://a.com/p`, **Then** only entities from `/p`'s episodes are returned.
2. **Given** episodes from `https://a.com/p?x=1` and `https://a.com/p/sub`, **When** the client queries `https://a.com/p`, **Then** none of their entities are returned unless a live `/p` episode also supports them.
3. **Given** an entity whose only supporting episode from the source has been superseded, **When** the client queries entities-by-source for that source, **Then** that entity is not returned.
4. **Given** an entity supported by both a superseded and a live episode of the source, **When** the client queries, **Then** the entity is returned.

---

### User Story 3 - Clients read source file and chunk ID directly (Priority: P2)

A client that lists episodes or searches passages gets the source file and the chunk ID as separate fields. It no longer has to parse the composed display string.

**Why this priority**: This removes the client-side "split on the last colon and hope" workaround described in #830. It adds fields and breaks nothing, but correctness does not depend on it the way it does on Stories 1 and 2.

**Independent Test**: Ingest a chunk whose source file is a URL containing a port. List the episodes and search passages, and confirm each result carries the exact source file and chunk ID alongside the existing display value.

**Acceptance Scenarios**:

1. **Given** a chunk ingested with source file `https://a.com:8443/x` and chunk ID `c1`, **When** the client lists recent episodes, **Then** the episode reports source file `https://a.com:8443/x`, chunk ID `c1`, and the unchanged display value `https://a.com:8443/x:c1`.
2. **Given** the same episode, **When** the client searches passages and it is returned, **Then** the passage carries the same source file and chunk ID.
3. **Given** a legacy episode with no stored source identity, **When** it is returned, **Then** the source file and chunk ID are reported as null or absent (or as backfilled values, see FR-012), and the display value is unchanged.

---

### User Story 4 - Source identity survives rebuild and upgrade (Priority: P2)

An operator upgrades to the release containing this change, keeps working, and later rebuilds the graph from the write-ahead log. Episodes keep their source file and chunk ID. If the operator rolls back to 0.19.1, it still opens the upgraded database and ignores the new data.

**Why this priority**: A rebuild that lost source identity would quietly send every episode back to the weaker legacy matching. A database the previous release cannot open would make rollback impossible.

**Independent Test**: Ingest chunks, rebuild from the WAL, and confirm the fields are preserved. Separately, take a database in the 0.19.1 layout, open and write it with the new build, and confirm it still opens in 0.19.1-compatible form. Replay a WAL written before this change and confirm it completes, with the new fields empty.

**Acceptance Scenarios**:

1. **Given** episodes ingested after this change, **When** the graph is rebuilt from the WAL, **Then** every rebuilt episode has the same source file and chunk ID as before.
2. **Given** a WAL written by a release before this change, **When** it is replayed, **Then** replay succeeds and the replayed episodes have no stored source identity (they are legacy rows).
3. **Given** a database created by 0.19.1, **When** the new release opens it, **Then** it opens without a storage-format version change and gains the new fields.
4. **Given** a database the new release has upgraded and written to, **When** a 0.19.1 binary opens it, **Then** it opens successfully and operates normally, ignoring the new fields.

---

### Edge Cases

- **A source file that contains colons**, such as a scheme, a port, IPv6 literals, or `mailto:`/`urn:` forms. Matching on the stored source file must be exact, whatever the colons are.
- **A chunk ID that contains colons.** It makes the composed string ambiguous even for a non-URL source. The legacy rule has to handle this safely.
- **One source being a string prefix of another** (`notes/a` vs `notes/ab`; `https://a.com/p` vs `https://a.com/page`). Neither may match the other.
- **Legacy rows** (no stored source identity) coexisting with new rows for the same source. Delete-by-source and entities-by-source must cover both, applying exact matching to new rows and the legacy rule to legacy rows.
- **Episodes added through the raw add-episode method**, which takes a free-form source description and no separate source file or chunk ID. They have no structured source identity and stay under the legacy rule permanently.
- **Document-ingest episodes** have to carry source identity the same way chunk-ingest episodes do.
- **Group scoping.** Both operations still honour the caller's group scope.
- **Superseded episodes in delete-by-source.** Deleting a source removes all of that source's episodes, superseded ones included. Only entities-by-source is limited to live episodes.
- **Case and normalisation.** Matching compares the stored identifier byte for byte. A trailing slash or a different letter case is a different source.
- **Empty source.** An empty source argument is rejected, as it is today.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every episode created by chunk ingest must store its source file and its chunk ID as two separate values, in addition to the existing composed display value.
- **FR-002**: Every episode created by document ingest must store its source file and chunk ID as separate values, consistent with the chunk path.
- **FR-003**: The composed display value must stay unchanged in content and format for all existing and new episodes.
- **FR-004**: Delete-by-source must match episodes whose stored source file is exactly equal to the requested source. It must not match on prefix or substring.
- **FR-005**: For legacy episodes (no stored source file), delete-by-source must apply a legacy matching rule that **never** deletes an episode whose source cannot be shown unambiguously to equal the requested source. In particular, a legacy episode belonging to `https://a.com:8443/x` must never be deleted by a request for `https://a.com`. The rule may fail to match a legacy row (under-match); it must never match a different source (over-match). The chosen rule and its rationale must be recorded in an ADR.
- **FR-006**: Entities-by-source must return only entities supported by episodes whose stored source file is exactly equal to the requested source. Legacy episodes are matched by the same safe legacy rule as FR-005.
- **FR-007**: Entities-by-source must only consider live episodes, using the project's existing definition of a live (not superseded) episode.
- **FR-008**: Both operations must keep honouring caller-supplied group scoping and their existing input validation.
- **FR-009**: Source file and chunk ID must be recorded in the write-ahead log for every mutation that creates or carries them, so that a rebuild from the log restores them exactly.
- **FR-010**: Replaying a write-ahead log written before this change must succeed, producing episodes with no stored source identity.
- **FR-011**: The schema addition must be additive and must not change the storage-format version. A 0.19.1 binary must be able to open and operate on a database the new release has upgraded and written to. An automated upgrade test must cover the 0.19.1 → new-release open and the reverse compatibility property.
- **FR-012**: The release *may* provide an idempotent backfill (admin-triggered or on-open) that sets source file and chunk ID on legacy rows **only** where the split of the composed value is unambiguous. Whether to provide it, and in which form, must be decided and justified in the ADR. If provided, running it more than once must produce the same result as running it once, and it must never assign an identity to an ambiguous row.
- **FR-013**: Every response that returns episodes or passages (including get-episodes and search-passages, and any other method returning episode or passage records) must include source file and chunk ID as additional fields. They are null or absent for legacy rows that have not been backfilled. No existing field is removed or changed.
- **FR-014**: The MCP tool surface and the Python-side IPC protocol must stay compatible: new response fields are additive, and request shapes for the two fixed operations are unchanged.
- **FR-015**: The CHANGELOG must record the fix under **Fixed**, flagged as a behaviour change for clients that relied on substring or prefix matching in delete-by-source or entities-by-source.
- **FR-016**: The ADR, the user-facing docs for the affected methods, and `llms-full.txt` must describe the exact-match semantics, the legacy rule, the live-only filter, the new response fields, and the backfill decision.
- **FR-017**: The chunk-level removal operations (mark-chunk-removed, delete-chunk-episode) must be checked for the same loose-matching defect. If they share it, they must be fixed under the same exact-match semantics and the fix noted in the CHANGELOG and ADR. If not, the ADR must state that they were checked and found unaffected.

### Key Entities *(if applicable)*

- **Episode source identity**: The pair (source file, chunk ID) that identifies where an episode's content came from. Both values are optional, so that legacy episodes are representable. This pair is authoritative for source-scoped operations.
- **Display value (`source_description`)**: The existing composed string `"<source_file>:<chunk_id>"`, kept for display and backward compatibility. It is no longer used as the authority for matching when a source identity is present.
- **Legacy episode**: An episode with no stored source identity, written before this change, replayed from a pre-change WAL, or created by the raw add-episode method. Source-scoped operations match it only under the safe legacy rule.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In the two-URL scenario (`https://a.com`, `https://a.com:8443/x`), deleting by source `https://a.com` removes 100% of that source's episodes and 0 of the other source's.
- **SC-002**: In the prefix-URL scenario (`https://a.com/p` plus `/page`, `?x=1` and `/sub` variants), entities-by-source for `https://a.com/p` returns no entity that only the other sources support.
- **SC-003**: Entities-by-source returns 0 entities whose only supporting episodes from the requested source are superseded.
- **SC-004**: After a rebuild from the write-ahead log, 100% of post-change episodes report the same source file and chunk ID as before the rebuild.
- **SC-005**: The automated upgrade test passes: a 0.19.1-layout database opens in the new release, and the upgraded database opens in 0.19.1-compatible form, with no storage-format version change.
- **SC-006**: Across the legacy-rule test cases (URLs with ports, chunk IDs containing colons, prefix-related sources), delete-by-source never deletes a legacy episode belonging to a different source.
- **SC-007**: Existing clients that do not use the new response fields see no change in request handling or in existing response fields, apart from the documented narrowing of the two fixed operations.

## Assumptions

- **Legacy rule is chosen in Plan.** This spec fixes only the safety invariant: legacy matching may under-match but must never over-match. Splitting on the last colon, or refusing ambiguous legacy matches, both satisfy it if implemented correctly. The ADR picks one and records why.
- **Same legacy rule for both operations.** Entities-by-source uses the same legacy rule as delete-by-source, for consistency, even though over-matching is less harmful there.
- **Backfill is optional.** Leaving legacy rows unbackfilled is an acceptable outcome provided FR-005/FR-006 hold. The ADR records the decision.
- **"Live" means the project's existing live-episode predicate**, the one chunk-lineage lookups already use. No new definition of supersession is introduced.
- **Delete-by-source still deletes superseded episodes** of the matched source. The live-only restriction applies to entities-by-source only, as the issue states.
- **Raw add-episode is not changed.** It does not gain source-file or chunk-ID inputs in this issue, so its episodes stay legacy rows.
- **Exact means byte-exact.** No URL normalisation (case, trailing slash, default port) is applied. Clients pass the same identifier they ingested with.
- **The upgrade test is in-tree.** "A 0.19.1 binary still opens the upgraded database" is verified by a test that reproduces the 0.19.1 schema and open path (or an equivalent fixture), not by running a downloaded 0.19.1 release binary in CI.
- **Entity provenance lists are unchanged.** The per-entity `source_descriptions` lists are not restructured.

## Out of Scope *(optional)*

- Changing the format or content of the composed display value.
- URL canonicalisation or normalisation of source identifiers.
- Adding source-file or chunk-ID inputs to the raw add-episode method.
- Changing the matching semantics of mark-chunk-removed or delete-chunk-episode, **unless** FR-017 finds they share the loose-matching defect.
- The citation-target lookup added by #824, which also matches on the composed display value. It is a separate feature and can become a follow-up issue if it shows the same over-matching.
- Restructuring per-entity provenance lists (`source_descriptions` on entity nodes and edges).
- A storage-format version bump or any non-additive schema migration.

## Source References *(optional)*

- #829: community bug report, entities-by-source substring matching on URL sources.
- #830: community bug report, ambiguous composed source string and delete-by-source over-matching.
- #752 / ADR-0752: precedent for adding a nullable episode column without a storage-version change.
- #292 / ADR-0292: precedent for chunk-lineage episode columns, and the existing live-episode predicate.
- #824: citation lookup that also matches on the composed display value (out of scope; possible follow-up).
- CLAUDE.md: "Build artifact" (IPC parity with the Python side), "When adding a new `knowledge_*` dispatch method" (MCP tool registry), and the ADR numbering rule (ADR-0853).
