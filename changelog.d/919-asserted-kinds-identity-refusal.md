### Fixed

- **Ingest was refused after a restart in groups with asserted entity kinds (#919).** Since
  0.20.0, a group with entities created by `knowledge_assert_entity` with an explicit `kind` (for
  example `Conversation` or `File`) refused `knowledge_add_episode` with `-32003` after a restart
  when it had no identity stamp. That includes every group whose ontology has no `identity` type,
  and workspaces upgraded from 0.19.1 on their first ingest. The cause was 0.20.0's check of
  stored entity kinds (#778), which read an asserted kind as a removed identity flag. That check
  is withdrawn; the stamp restore from the WAL ontology sidecar stays.

### Known issues

- **#778 is open again.** An identity removal made while both the group's stamp and its WAL
  ontology sidecar's `identity_types` are missing is not detected, as in 0.19.1. A stamp that is
  present, or restorable from the sidecar, is still checked.
