---
layout: default
title: Ontology
---

# Ontology

`liminis-context-graph` supports an **optional workspace-scoped ontology** that declares the entity types and relation types the LLM should use during extraction. Without an ontology, the LLM derives types ad-hoc (free-form behavior). With one, vocabulary is consistent and queryable across all chunks.

## File location

Place the ontology at `{workspace}/.lcg/ontology.yaml`.

**Requires a service restart to take effect.** The workspace-wide ontology is loaded once at startup and held in memory. Editing the file while the service runs has no effect until the next restart. (A group's *per-group* file can instead be picked up on a live service with [`knowledge_reload_ontology`](#reloading-a-groups-ontology); that call re-resolves one group but does not re-read this workspace file.)

## Per-group ontologies

A single lcg instance can hold many co-resident `group_id`s (multi-group hydrate), and those
groups often want different vocabularies — a content channel's `Person`/`Organization` ontology
should not constrain an unrelated catalog group co-resident in the same workspace. Place a
group-specific ontology at:

```text
{workspace}/.lcg/ontology/<group_id>.yaml
```

using the same file format described above. A `group_id` containing characters unsafe as a
filesystem path component (anything outside ASCII alphanumerics, `_`, and `-`) is percent-encoded
using the same bijective scheme already applied to per-group WAL directory names — every byte
outside that safe set becomes `%XX` (uppercase hex). A `group_id` that's already a safe path
component (e.g. `catalog`, `content-v2`) is used as the filename unchanged.

**Known v1 limitation: no case-insensitive collision guard.** Two already-safe `group_id`s that
differ only by ASCII case (e.g. `Catalog` and `catalog`) resolve to the same filename on a
case-insensitive filesystem (the default for macOS APFS and Windows NTFS). Per-group WAL
directories guard against this exact case with an explicit, loudly-failing check
(`wal_group::check_no_case_insensitive_collision`, invoked when a group's WAL writer is first
created); per-group ontology file resolution does not yet apply the same guard, so on an
affected filesystem one group's ontology could silently load for the other. Avoid `group_id`s
that differ from another co-resident group's only by letter case until this is closed.

**Resolution and fallback.** For a given `group_id`:

1. If `{workspace}/.lcg/ontology/<group_id>.yaml` exists and parses successfully, it governs
   extraction, `mode` (including strict validation), canonicalization, and reprocessing
   (`knowledge_reprocess_entity_types`, `knowledge_reprocess_relation_types`) for that group only.
2. Otherwise, the workspace-wide `{workspace}/.lcg/ontology.yaml` (described above) governs that
   group, exactly as it did before per-group ontologies existed.
3. If neither exists, that group extracts free-form, same as an ontology-less workspace today.

A malformed or unreadable per-group file is treated exactly like a missing one: resolution falls
through to step 2 (the workspace-wide ontology) if one exists, or step 3 (free-form extraction) if
it doesn't — never a startup failure or a hard error for that group. This degrades gracefully to
whatever ontology this workspace already has validated (which may be none at all), and the failure
is logged so it's observable rather than silent. Like the workspace-wide file, per-group files are
loaded once (on that group's first use in the running process) and cached — restart the service, or
call [`knowledge_reload_ontology`](#reloading-a-groups-ontology) for that group, to pick up a changed
file.

**Direct-assert is unaffected.** `knowledge_assert_entity`/`knowledge_assert_relationship` accept
arbitrary `labels` regardless of any per-group or workspace ontology — per-group resolution only
governs *extraction-guided* groups (`knowledge_add_episode` and the maintenance operations above).
An entity's `kind` (issue #615) is part of its identity, not its ontology type: extraction creates
kind `Entity` unless the ontology marks the entity's type [identity-bearing](#identity-bearing-types)
(issue #616), while direct-assert may give an entity any `kind`. The ontology restamp/reprocess
operations rewrite type labels but preserve the entity's kind in its `labels`.

**`canonicalize_relations`** resolves and applies the target group's own ontology, scoped to the
`group_id` the call already requires. **`backfill_relation_types`** is ontology-independent — it
derives pseudo relation types from edge fact text, not from a declared vocabulary — so per-group
ontology resolution has nothing to change there.

**Published ontology is documentation, not policy.** When a group's stream is published (the
existing whole-directory copy described in [Operations](operations.md)), the ontology that guided
that group's extraction travels alongside it as `.wal-ontology.json` — informational, with one exception below. A
consumer hydrating that stream can inspect it to see what vocabulary produced the graph, but it is
never applied to the consumer's own extraction, `mode: strict` validation, canonicalization, or
reprocessing for that group — the consumer's own local configuration (per-group file, workspace
file, or neither) is always what governs. A stream published without this file still replays and
behaves identically; only the documentation available to the consumer is degraded. The one field
a rebuild reads back is `identity_types` — the group's identity-bearing set — which
`knowledge_rebuild_from_wal` uses to restore a missing identity-set stamp (see below); it never
overwrites an existing stamp, and a sidecar without the field restores nothing.

## Drift detection

If the ontology governing a group's data changes between service restarts or reloads — the file is edited,
or a group starts/stops resolving through a per-group file versus the workspace fallback — the
graph's entity/relation types may no longer match the vocabulary that produced them. Drift
detection catches this and recommends **Recreate + re-ingest** (or a WAL rebuild/replay) as the
remediation.

**Workspace-level drift** is unchanged from before per-group ontologies existed: computed eagerly
at startup by comparing the workspace ontology's content hash against `.lcg/ontology-hash.json`,
and reported via the top-level `ontology.drifted`/`ontology.drift_summary` fields in
`knowledge_status` (see below).

**Per-group drift** covers the surface per-group ontologies (above) added: both a group governed
by its own dedicated `.lcg/ontology/<group_id>.yaml` file, and a group that falls back to the
workspace ontology. A change to either source — or a change in *which* source a group resolves
through (e.g. its per-group file is added or removed) — is detected as drift for that group.
Persisted per-group state lives at `.lcg/ontology-hash/<group_id>.json` (same `<group_id>`
percent-encoding as per-group ontology files), one file per group, separate from the
workspace-level `.lcg/ontology-hash.json` file.

**Per-group drift is computed lazily, not scanned eagerly at startup.** A group's drift status
becomes available the first time that group's ontology is resolved in the running process — the
same trigger point as ontology resolution itself (a group's first `knowledge_add_episode`, or any
other extraction-guided operation for that group). A group never used in the current process has
**no drift status at all** ("not yet computed"), distinct from "not drifted" — see the
`knowledge_status` example below. This mirrors per-group ontology resolution's own use-triggered
caching: a group that has **not yet been resolved** in this process picks up whatever its file
(or the workspace fallback) contains *at the time of that first use*, so an edit made while the
service keeps running is visible the first time the group is actually used. It's only a group
whose resolution is **already cached** — because it was used earlier in this process — that needs
a restart (or a [`knowledge_reload_ontology`](#reloading-a-groups-ontology) call) to see a later
edit; once a group's drift status has been computed, it does not change again mid-process even if
its file is edited again afterward, unless that group is reloaded.

Drift clears after a successful remediation for the specific group remediated: either a fresh
ingest for that group (`knowledge_add_episode`, e.g. following "Recreate + re-ingest") or a
`knowledge_rebuild_from_wal` replay for that group. Clearing one group's drift never clears (or
affects) any other group's drift status.

## Reading the effective ontology

`knowledge_get_ontology {group_id?, include_asserted?}` (MCP tool of the same name, `read` scope)
returns the ontology that applies to a group — the same group → workspace → none resolution used
for extraction — as a planning menu for `knowledge_walk`: mode, entity types (description, parent)
and relation types (description, `source_type` / `target_type` lists, `inverse_aliases`,
`symmetric`). Assert-only types (`extract: false`) are omitted unless `include_asserted: true`; a
group with no ontology gets `has_ontology: false` and `mode: "free-form"`. See
[IPC/MCP reference → Typed relation walks](ipc-mcp-reference.md#typed-relation-walks-knowledge_get_ontology-knowledge_walk-issue-820).

## Reloading a group's ontology

A group's ontology is resolved on its first use and then cached, so editing its file does not
affect a running service. To make **one group** adopt an edited ontology without restarting (and
so without interrupting every co-resident group), call the admin-scoped `knowledge_reload_ontology`
(MCP tool of the same name; `admin` scope only):

```json
{"method": "knowledge_reload_ontology", "params": {"group_id": "content"}}
```

The group's cached resolution is discarded and immediately re-resolved through the normal
precedence (per-group file, then the workspace ontology, then free-form). The response:

```json
{
  "group_id": "content",
  "previous_hash": "3f1a…",
  "new_hash": "9b7c…",
  "changed": true,
  "drift": {"group_id": "content", "drifted": true, "drift_summary": "…"},
  "identity_refusal": null
}
```

- `previous_hash` is `null` if the group had not been resolved in this process yet; `new_hash` is
  `"none"` when the group resolves to no ontology.
- `changed` is `true` iff the two hashes differ. Reloading an unchanged file returns
  `changed: false` and changes nothing (drift, refusal state and stored identity stamp included), so
  it is safe to call defensively. A never-resolved group that resolves to no ontology is also
  `changed: false`.
- `drift` is the group's per-group drift status after the reload, the same entry
  `knowledge_status.group_ontology_drift` reports. It compares against the **last-ingested**
  ontology, not the previous cached one, so `changed` and `drift.drifted` are independent.
- `identity_refusal` is `null`, or — when the new ontology adds or removes `identity: true` on a type
  the group already holds — an object `{message, label, count, adding}`. The group is then refused
  exactly as a restart would refuse it (see [Identity-bearing types](#identity-bearing-types)):
  `knowledge_add_episode` fails with `-32003` and the group is listed in
  `knowledge_status.group_identity_refusals`. The reload call itself still succeeds. Restore the
  previous identity-bearing set in the file and reload again to clear the refusal.

After a successful reload, new extraction for the group is typed by the new ontology, and
`knowledge_canonicalize_relations` and `knowledge_reprocess_entity_types` resolve against it. Other
groups' cached resolutions, hashes and drift status are untouched, and no stored data or schema is
modified.

Things to know:

- The reload takes the service write lock, so it waits for in-flight read-lock passes
  (`knowledge_reprocess_*`, `knowledge_canonicalize_relations`, backfills) to finish; while it waits,
  new reads and writes queue behind it.
- Extraction that is already in flight is not interrupted: an episode whose extraction started under
  the old ontology may still commit afterwards with entities typed by the old ontology, but it will
  not overwrite the group's recorded ontology hash or clear the drift the reload reported.
  Reprocess/canonicalize calls that resolved their ontology just before a reload likewise finish
  under the old one; only reload-then-call ordering is guaranteed.
- The workspace-wide `.lcg/ontology.yaml` is still read only at startup. A group that relies on it
  re-resolves to the startup value; a workspace-wide edit still requires a restart.
- If the identity-set check cannot run (database unavailable), the reload fails and changes nothing.
- A missing `group_id` is rejected with an error; a malformed or unreadable per-group file is treated
  as missing, as at first resolution.

## Format

```yaml
# mode: open | strict
# open (default): declared types are preferred; free-form fallback allowed
# strict: out-of-vocabulary entities and edges are never dropped for their type alone — an
#   entity whose type doesn't match the declared vocabulary is reclassified to Unclassified,
#   with its original type preserved in the entity's attributes (see ADR-0312); a declared
#   alias on an edge is normalized to its canonical relation type, and anything else is
#   reclassified to relation_type: UNCLASSIFIED with the original label preserved in the
#   edge's attributes (see ADR-0310). Edges can still be dropped for unrelated reasons
#   (self-referential, unresolvable endpoint) — see edges_dropped_unresolvable.
mode: strict

entity_types:
  - name: Person           # normalized to PascalCase
    description: A human individual, not a role or title.
  - name: Organization
  - name: Document
  - name: Rfc
    parent: Document       # optional: Rfc is a subtype of Document
  - name: Adr
    parent: Document       # optional: Adr is also a subtype of Document
  - name: Paper

relation_types:
  - name: AUTHORED         # normalized to SCREAMING_SNAKE_CASE
    description: A person wrote a paper.
    source_type: Person    # optional signature: which entity type(s) may be the source
    target_type: Paper     # ...and the target (a name or a list; see "Edge direction")
    aliases: [WROTE, PENNED]   # optional: alternate spellings normalized to AUTHORED
    inverse_aliases: [AUTHORED_BY]   # optional: names that read the other way (swap endpoints)
    keywords: [author]         # optional: lowercase substrings used by the offline
                                # knowledge_canonicalize_relations pass (fuzzy match)
  - name: AFFILIATED_WITH
    source_type: Person
    target_type: Organization
```

`aliases` and `keywords` on a relation type have three consumers, each with different matching rules:

- **The `strict`-mode edge prompt** (`build_fact_types_section`) renders both `aliases` and `keywords` for every declared relation type, so the model can see the full set of accepted spellings and is more likely to emit the canonical name directly.
- **Ingest-time `strict`-mode filtering** (`episode.rs`) consults only `aliases`, as an exact match after the same case/separator normalization applied to every relation type name (`normalize_relation_type`) — e.g. `wrote` normalizes to `WROTE` and resolves via the alias map to `AUTHORED`. `keywords` play no role in ingest-time filtering.
- **The offline `knowledge_canonicalize_relations` maintenance pass** (see [IPC & MCP Reference](ipc-mcp-reference.md#relation-typing-canonicalize_relations-backfill_relation_types-reprocess_relation_types)) consults both: `aliases` via the same exact map, and `keywords` as lowercase substrings for its fuzzy-matching fallback.

### Edge direction

An edge is directed, and a relation's *type name* says which way: `OWNS` reads "source OWNS
target". The extractor, left alone, tends to make the **sentence's grammatical subject** the source,
so "Sable Core is owned by Lena Moreau" comes out as `Sable Core OWNS Lena Moreau` — reversed.
Where direction carries meaning ("who reports to X" vs "who does X report to") that is a confidently
wrong answer. The ontology therefore declares direction, and LCG prompts for it, checks it, and can
repair it ([ADR-0816](adr/0816-ontology-edge-direction-check-and-repair.md)). **The ontology's reading
of the relation wins over the sentence's subject.**

Fields on a relation type, all optional and additive (an ontology that uses none of them loads
unchanged and keeps its hash):

| Field | Meaning |
|-------|---------|
| `source_type` / `target_type` | The entity type(s) allowed at that end: a single name (`source_type: Person`) or a list (`source_type: [Person, Team]`). A type matches if it, **or one of its declared ancestors** (`parent`), is listed. An unset side accepts any type. |
| `inverse_aliases` | Names that read the relation the other way, e.g. `APPROVED_BY` for `APPROVES`. They resolve to the canonical type **with source and target swapped**. |
| `symmetric: true` | Endpoint order carries no meaning (e.g. `COLLABORATES_WITH`). It is never checked or swapped. |

```yaml
relation_types:
  - name: OWNS
    source_type: [Person, Team]      # a person or team owns...
    target_type: [Project, System, Policy, Agreement]
  - name: APPROVES
    source_type: Person              # half signature: target is "any"
    inverse_aliases: [APPROVED_BY, HAS_EXCEPTION_APPROVER]
  - name: COLLABORATES_WITH
    symmetric: true
```

A signature naming an entity type the ontology does not declare logs a load warning naming the
relation and the type. It is not an error, and that side of the signature is then ignored (if every
listed type on a side is undeclared, the side behaves as unset). An inverse alias that collides with
another relation's name or alias also warns; the forward reading wins.

**In the extraction prompt** (every mode, including no ontology) the edge prompt states that
`source_name` is the subject of the relation *as its type name reads*, with the worked example
"Sable Core is owned by Lena Moreau" → `Lena Moreau OWNS Sable Core`. The relation list renders
signatures even when only one side is declared — `(Person → any)`, `(Person | Team → Project | System)` —
marks symmetric relations, and (strict mode) lists each inverse alias as "`APPROVED_BY` = `APPROVES`
with source and target swapped".

**At ingest** (strict *and* open mode, for every relation that declares a signature), once the
relation type is resolved and both endpoints' entity types are known, and before the edge is written:

1. An inverse alias swaps the endpoints first (strict mode resolves aliases at ingest; this applies
   even if the canonical type declares no signature, since the inverse reading is itself the
   direction).
2. Endpoints fit the signature as extracted → stored as is. (If *both* orientations fit, the
   extracted order is kept.)
3. Endpoints fit only when reversed → **swapped**, and counted in `edges_swapped`.
4. They fit neither way → **stored unchanged** with `"signature_mismatch": true` in the edge's
   `attributes`, counted in `edges_signature_mismatch`. The edge is never dropped: many such edges
   are incidental facts between loosely typed entities.
5. Symmetric relations, relations with no usable signature, and edges with an untyped endpoint
   (`Entity`/`Unclassified` only) are skipped.

In open mode no alias resolution happens at ingest, so the check applies to edges whose relation type
is exactly a declared name; an inverse-alias spelling in open mode is fixed by the repair pass.

**For graphs that are already stored**, `knowledge_canonicalize_relations` applies the same rules
without re-extraction (`dry_run` previews them; see the
[IPC & MCP Reference](ipc-mcp-reference.md#relation-typing-canonicalize_relations-backfill_relation_types-reprocess_relation_types)).
It also retypes inverse-alias edges and swaps them in one pass, clears a stale `signature_mismatch`
mark once an edge fits, and is idempotent. `knowledge_assert_relationship` and other direct edge
writers do **not** run the check.

Editing a signature, `inverse_aliases` or `symmetric` changes the ontology content hash (an ontology
that does not use them is unaffected), so it shows up as drift until the group is re-processed.

### Entity type hierarchy

The optional `parent: <TypeName>` field on an entity type declares a single-parent (tree) subtype relationship. A node typed `Rfc` will carry labels `["Entity", "Document", "Rfc"]` — enabling both specific queries (`WHERE 'Rfc' IN e.labels`) and rollup queries (`WHERE 'Document' IN e.labels`).

- **Additive**: the specific type is never replaced by its parent; ancestor labels are added alongside it.
- **Transitive**: a 3-level chain `SubDoc → Rfc → Document` stamps all four labels.
- **Safe degrades**: an undeclared parent is cleared with a warning; cycles are detected and broken at startup (no crash).
- **Flat ontologies unaffected**: types without `parent` fields behave exactly as before — `["Entity", <SpecificType>]`.
- **Drift detection**: adding, removing, or changing a `parent` changes the ontology content hash, which triggers a `drifted: true` status in `knowledge_status`. Run `knowledge_reprocess_entity_types` to propagate new hierarchy to existing nodes.

### Identity-bearing types

By default extraction merges same-named entities by name, whatever type the model gave them: a
`Person "Aurora"` and a `Technology "Aurora"` become one node. Mark a type `identity: true` to
make it part of the entity's identity instead:

```yaml
entity_types:
  - name: Person
    identity: true         # optional; absent means false
  - name: Organization
    identity: true
  - name: Technology       # not identity-bearing: keeps merging by name
```

An entity whose **primary extracted type** is identity-bearing is created with `kind` set to that
type ([ADR-0615](adr/0615-kind-scoped-entity-identity.md) makes kind part of identity,
`(group_id, kind, name)`), so `Person "Aurora"` and `Technology "Aurora"` are two entities:
kind `Person`, and kind `Entity` carrying the `Technology` label. A second `Person "Aurora"` in a
later chunk resolves to the same kind-`Person` node, and a `Person` is never merged into an
existing `Entity`-kind "Aurora" (or the reverse). Everything not identity-bearing behaves exactly
as before, and an ontology with no `identity` keys is unchanged.

**Which types to flag.** Only stable, well-defined types whose extracted typing is consistent —
typically `Person` and `Organization`. Extracted typing is inconsistent for fuzzier types (the
same "Aurora" may be `Technology` in one chunk and `Product` in another); flag such a type and one
thing splits into several nodes. Leave those unflagged.

**Rules.**

- Kind comes from the entity's *primary* type alone. It is never inherited through `parent`: with
  `Document` identity-bearing and `Rfc` (parent `Document`) not, an `Rfc` gets kind `Entity`
  (labels `Entity`, `Document`, `Rfc`). The result never depends on label or declaration order.
- The flag is matched against the normalized (PascalCase) type name, like every other type match.
  An undeclared type (open mode) or `Unclassified` (strict mode) is never identity-bearing.
  `Merged` and `Unclassified` cannot be flagged.
- Asserted entities (`knowledge_assert_entity`) are unaffected: they use the caller's `kind`. To keep a host-asserted identity type out of extraction entirely, also set [`extract: false`](#assert-only-types-extract-false).
- **Edges name endpoints by name only.** If one batch contains the same name under two kinds, or a
  name from an earlier chunk resolves to more than one kind, that endpoint is ambiguous and the
  edge is dropped (counted in `edges_dropped_unresolvable`) rather than guessed at.
- `knowledge_reprocess_entity_types` never changes an entity's kind, and never removes it from
  `labels`. When a fresh classification disagrees with an entity's identity-bearing kind it
  reports the entity in a `kind_disagreements` array (`entity_id`, `entity_name`, `kind`,
  `classified_type`) and acts on nothing; deciding to merge or split is left to a person.

**Evaluated at extraction time only.** Identity is decided once, when an entity is created, and
never inferred afterwards. A WAL rebuild cannot apply a flag retroactively: replay re-executes
each record's stored `kind` mechanically and never calls the extractor, so flipping a flag and
rebuilding yields exactly the same graph. Nor can kind be re-derived from stored labels — if
"Aurora" was extracted as `Person` in one chunk and `Technology` in another, name-merging already
folded them into one node, and lcg cannot tell whether that was one thing typed inconsistently or
two things wrongly merged.

**Changing the flag is refused when it would reinterpret existing data.** lcg records, per group,
which types were identity-bearing (`.lcg/identity-set/<group_id>.json`, outside the WAL and the
DB, so it survives a clear or a rebuild-from-WAL into the same workspace root — it does **not**
travel with the WAL; after a rebuild into a fresh root it is restored from the WAL's
`.wal-ontology.json`, and a group with no stamp still works because the check below is
kind-aware). When a group's ontology first resolves in a process, its identity-bearing set is
compared with the record:

- A difference that touches only types no existing entity in the group carries **is accepted**;
  the record is updated and it takes effect for later extractions.
- Adding or removing the flag for a type whose existing entities **would change kind** **is
  refused**: adding the flag to `T` is refused only if an entity whose leaf type is `T` has a
  `kind` other than `T` (default `Entity`, NULL or empty); removing it is refused only if an entity
  has `kind = T`. Entities of a subtype of `T` are not affected (kind is never inherited), and
  entities whose kind already matches — e.g. replayed from a WAL with no stamp — are not
  reinterpreted, so an unchanged ontology is never refused. `knowledge_add_episode` for that group fails
  with JSON-RPC error `-32003` (`error.data.reason = "identity_set_change_refused"`, plus
  `group_id`, `label`, `entity_count`), and the group appears in `knowledge_status`'s
  `group_identity_refusals`:

  ```text
  identity-bearing set change refused for group "content": adding the `identity` flag for type
  "Person" would reinterpret 12 existing entities carrying that label. Identity is decided once,
  at creation, and never inferred afterwards. Restore the group's previously recorded
  identity-bearing set, or re-ingest the group from source.
  ```

A refusal modifies no data and affects only the offending group — reads, reprocessing, other
groups and service startup are unaffected. It holds until the group is reloaded with the recorded set restored
([`knowledge_reload_ontology`](#reloading-a-groups-ontology)) or the service is restarted with it
restored; either loads the group normally. The remedy for a change you
really want is to re-ingest the group from source (lcg does not automate this).

Because the flag is part of the ontology content hash, adding one also raises the ordinary
[drift](#drift-detection) warning; that warning is separate from, and weaker than, this refusal.

### Assert-only types (`extract: false`)

A host that asserts its own structural types through `knowledge_assert_entity` /
`knowledge_assert_relationship` — a `Source` entity and a `DERIVED_FROM` relation, say — does not
want the LLM extractor minting them from prose. Mark such a type `extract: false` (entity and
relation types both accept it; absent or `true` means extractable):

```yaml
entity_types:
  - name: Person
  - name: Source
    identity: true         # still identity-bearing for asserts and the identity-set guard
    extract: false         # but never offered to, nor accepted from, the extractor
relation_types:
  - name: KNOWS
  - name: DERIVED_FROM
    extract: false
```

`extract: false` is an extraction-time concept only; everything else treats the type as fully
declared.

- **Prompts.** The entity and edge extraction prompts omit the type (and its description,
  aliases and keywords). An ontology with no `extract: false` type renders the same prompts as one that never declared the flag.
  If *every* entity type is `extract: false` the entity section falls back to the default
  vocabulary; if every relation type is, the relation section is omitted.
- **Stray extractor output.** A prompt is advisory, so an extracted entity typed with an
  `extract: false` type (matched after normalization: `source`, `SOURCE` and `Source` are the same)
  is reclassified to `Unclassified` with the original label preserved in `original_entity_type` —
  in **both** `strict` and `open` mode, and it never receives the type's identity `kind` or its
  label. An extracted edge of such a relation type is reclassified to `relation_type: UNCLASSIFIED`
  with `original_relation_type` preserved. Both count toward `entities_reclassified_unclassified` /
  `edges_reclassified_unclassified`. Every other undeclared label keeps its normal `open`-mode
  behaviour. See [ADR-0637](adr/0637-extract-false-assert-only-types.md).
- **Asserts are unaffected.** `knowledge_assert_entity {kind: "Source"}` and
  `knowledge_assert_relationship` with `relation_type: "DERIVED_FROM"` work exactly as for any
  declared type.
- **Identity and hierarchy.** An `identity: true, extract: false` type counts in the identity set
  and the `-32003` guard. The flag is per type and not inherited through `parent`: children of an
  `extract: false` type are extractable and still get the parent's ancestor label.
- **Reprocessing and canonicalization.** `knowledge_reprocess_entity_types`,
  `knowledge_reprocess_relation_types` and `knowledge_canonicalize_relations` never retype into an
  `extract: false` type. Already-asserted entities and edges of such a type are not off-ontology
  candidates; under `scope: all` an asserted `Source` entity may appear in `kind_disagreements`
  because the classifier cannot choose `Source`.
- **Drift, not refusal.** Flipping `extract` changes the content hash (so it reports as
  [drift](#drift-detection)) but never the identity set, so
  [`knowledge_reload_ontology`](#reloading-a-groups-ontology) accepts it. Ontologies with no
  `extract: false` type hash exactly as before.

See [`docs/examples/ontology.example.yaml`](https://github.com/liminisapp/liminis-context-graph/blob/main/docs/examples/ontology.example.yaml) for a fully annotated scientific-paper-domain example.

### Extracted entities named after a source note ID

**An extracted entity named after a note ID is dropped.** Declaring a `Document` type can invite
the model to extract a note ID as an entity (e.g. `project-quill-core-1`), which then absorbs edges
that belong to the real entities. In practice the ID is usually a *citation*: wiki and answer notes
end with `Sources: [[project-quill-core-1]]`, and the model extracts the cited note. Extraction
deterministically drops an entity whose name equals an ID-shaped `[[wikilink]]` target in the
episode body (a link to a real entity such as `[[Quill Core]]` is kept), or the episode's *own*
source ID (the chunk's
`source_file`, `doc_id`, or `chunk_id` prefix, compared case-insensitively and ignoring path
prefix, wikilink brackets and a `.md`-style extension, and only when that ID is slug-shaped — a note
filed under its subject's name such as `Alice.md` has no ID to match, so the real entity is kept),
plus any edge touching it
([ADR-0824](adr/0824-own-source-id-entity-filter.md)). The asserted `Document`/`Source` entity your
host supplies is the supported way to represent the note. An ID-shaped name that is neither the
episode's own ID nor linked in its body is kept. The drop is reported as `entities_dropped_source_id` on `knowledge_process_chunk`.

## Modes

| Mode | Entity types | Relation types |
|------|-------------|----------------|
| `open` (default) | Preferred by the LLM; free-form fallback allowed | Same |
| `strict` | An entity whose normalized type doesn't match the declared vocabulary is retained with an `Unclassified` label in place of the rejected type, and its original type preserved in `attributes` — never dropped for its type alone (ADR-0312) | The edge-extraction prompt tells the model to use only the declared vocabulary (including aliases). A declared alias is normalized to its canonical name; anything still out-of-vocabulary after normalization is retained with `relation_type: UNCLASSIFIED` and its original label preserved in `attributes` — never dropped for its type alone (ADR-0310) |

## `knowledge_status` summary

The `knowledge_status` IPC response always includes an `ontology` field (workspace-level) and a
`group_ontology_drift` field (per-group, issue #451):

```json
{
  "ontology": {
    "present": true,
    "loaded": true,
    "mode": "strict",
    "entity_type_count": 4,
    "relation_type_count": 4,
    "drifted": false,
    "drift_summary": null
  },
  "group_ontology_drift": [
    { "group_id": "catalog", "drifted": true, "drift_summary": "entity types added: [Equipment]" },
    { "group_id": "content", "drifted": false, "drift_summary": null }
  ],
  "group_identity_refusals": []
}
```

When no ontology is loaded, `ontology.present` is `false` and counts are `0`; `ontology.drifted`
still reflects the workspace-level drift state (see [Drift detection](#drift-detection) above).

`group_ontology_drift` is an array with **one entry per group whose ontology has been resolved at
least once in the current process** — a group not yet used has no entry at all (not a false
`drifted: false`); see [Drift detection](#drift-detection) above for the lazy, use-triggered
timing. A single-ontology workspace (no `.lcg/ontology/` directory) behaves identically: the array
is empty until a group is used, then contains that group only, and the `ontology` field's shape
and values are unaffected either way.

`group_identity_refusals` lists `{group_id, message}` for each group whose identity-bearing-set
change was refused this process (see [Identity-bearing types](#identity-bearing-types)); it is empty
when nothing was refused.

The response also includes an `indices_built` boolean, a `name_index_trusted` boolean, and a
`name_index_fallback_scans` integer — these describe search-index and name-lookup health rather
than ontology state. See [Operations](operations.md) for those fields.
