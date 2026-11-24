### Fixed

- **Extraction no longer creates entities named after a note ID (#824).** Under an ontology with a
  `Document` type, extraction could return an entity that was really a note ID cited as a
  `[[wikilink]]` (`Sources: [[project-quill-core-1]]`), which absorbed edges that belonged to real
  entities and showed up in walks and entity search. The text prompt now says such citations are not
  entities, and an extracted entity whose normalised name equals the episode's own source ID is
  dropped. The host-asserted `Document` / `Source` entity is unaffected. Existing graphs are not
  repaired.
