//! Document mode (issue #785, ADR-0785): `knowledge_process_document` ingests one whole document
//! under a completeness claim, chunks it server-side, diffs it against the previous committed
//! revision and applies ADR-0705's supersession operations itself.

pub mod chunker;
pub mod diff;
