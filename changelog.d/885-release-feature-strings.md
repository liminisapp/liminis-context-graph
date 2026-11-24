### Added

- **Nine more `knowledge_capabilities` feature strings for this release's changes (#885 follow-up).**
  The release runbook's coverage step found changes that shipped with neither a string nor a stated
  reason. New `features` entries, all additive (`PROTOCOL_VERSION` stays 1, no runtime behaviour
  changes): `belief_aware_passages` (#806), `delete_orphan_prune` (#859), `embedding_profiles` (#790),
  `literal_object_edges` (#823), `ontology_edge_direction` (#816), `ontology_hash` (#881),
  `passage_expansion` and `passage_rerank` (#796), and `passage_query_variants` (#802). The IPC
  reference's "Changes deliberately not advertised" now lists the bug fixes and `lcg-eval` changes
  that need no string, and why.
