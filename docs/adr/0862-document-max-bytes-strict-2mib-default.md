# ADR-0862: `process_document` Size Cap — 2 MiB Default, Strict `LCG_DOCUMENT_MAX_BYTES`

**Status**: Accepted
**Date**: 2026-11-23
**Issue**: #862 (implements #840). Supersedes the cap default and the lenient parse recorded in
[ADR-0785](0785-document-mode-revisions-and-commit-boundary.md) (section 5, "Cap"), which stays as
written.

## Context

`knowledge_process_document` refused a document over 512 KiB, yet real web pages reach about 600 K
characters. ADR-0785 made the cap configurable with `LCG_DOCUMENT_MAX_BYTES`, but a value that was
not a positive integer logged a warning and fell back to the default, so a typo or a `0` silently
left the wrong limit in place, and there was no way to write `4MiB`. The maintainer decided to
raise the default to 2 MiB unless measurement argued against it, and to treat the variable like
`LCG_SEARCH_MIN_SCORE` ([ADR-0812](0812-search-min-score-env-default.md)): strict, at startup.

## Decision

1. **The default is 2 MiB (2,097,152 bytes of UTF-8)**, up from 512 KiB. The measurements below do
   not argue against it.
2. **`LCG_DOCUMENT_MAX_BYTES` is parsed strictly, once, at startup**, in the new
   `crates/core/src/document_config.rs`, following `search_config.rs` and ADR-0737: an invalid value
   makes startup fail with `FATAL invalid LCG_DOCUMENT_MAX_BYTES="<value>": <reason>; expected …`,
   before any database open or socket bind, and never routes into degraded mode. The check runs
   after the attached `--connect` early return (the daemon owns the variable). The effective cap is
   logged at startup (`document size cap: N bytes (LCG_DOCUMENT_MAX_BYTES[ unset])`).
3. **Grammar.** After trimming outer whitespace: ASCII digits only (a byte count), or digits
   immediately followed by `KiB` or `MiB` (case-sensitive; 1024-based). Rejected, each with the
   value in the message: empty, `0`, signs (`-1`, `+4`), decimals (`1.5MiB`), `_` separators, inner
   whitespace (`4 MiB`), decimal and short units (`4KB`, `4MB`, `4m`), lower-case units (`4mib`),
   `GiB` and larger, hex and exponent forms, and any number that overflows `usize`
   (`checked_mul`). `GiB` is rejected on purpose: the plan was to accept it, but with the bound in
   (5) every `GiB` value would be out of range, so accepting it would only produce a more confusing
   error.
4. **Behaviour change.** A previously tolerated stray value (`0`, `abc`, an empty string, a negative
   number) now stops the service from starting. This is intended and is a CHANGELOG `Changed`
   entry. `LCG_DOCUMENT_CHUNK_MAX_CHARS` keeps its lenient reader and is out of scope.
5. **Upper bound: 256 MiB, rejected at startup** with a message naming the bound. A hard bound was
   preferred over a warning because the cap exists to bound memory, and one more silent knob would
   repeat the problem this change fixes. The bound is not a promise that 256 MiB is cheap (see the
   measurements); it only stops a mistyped unit from disabling the cap.
6. **Install once, default fallback.** `main` installs the value into a process-wide `OnceLock`
   (`document_config::install`); `chunk_for_ingest` reads `document_max_bytes()`, which falls back
   to 2 MiB when nothing was installed (in-process tests and attached `--connect` mode, where the
   daemon owns the cap). The environment is no longer re-read per request. `chunk_for_ingest` is a
   thin wrapper over `chunk_for_ingest_with_cap(content, cap)`, which tests use to exercise other
   caps in one process.
7. **Over the cap is refused, never truncated.** `chunk_for_ingest` is the single enforcement
   point, shared by `knowledge_process_document` and `knowledge_preview_chunks`, and runs before any
   chunking, extraction or write. The error is `document_too_large: content is N bytes, over the
   document-size cap of C bytes (LCG_DOCUMENT_MAX_BYTES); nothing was extracted or written`. The
   cap is measured in **bytes** of the content (a multibyte page is larger than its character
   count); a document exactly at the cap is accepted, one byte over is refused.
8. **A feature string, `document_max_bytes_env`**, is added to `knowledge_capabilities`, so a client
   can tell a server with the raised, configurable cap from an older one. The configured value is
   not exposed.
9. **No transport limit sits below the cap.** The socket reader is a line reader with no maximum
   line length (`connection.rs`), and the MCP-stdio codec in the pinned `rmcp` has
   `max_length = usize::MAX` by default and the service does not set one. A refused document is
   therefore refused by the cap, never by a transport that would make the cap in the message
   misleading. The service buffers at most 8 pipelined lines while it ingests, so the worst
   in-memory request queue at a 2 MiB cap is about 16 MiB.
10. **The oversize `knowledge_process_chunk` warning names `knowledge_process_document`**, in the
    `warning.message` and the tool description, replacing "consider splitting oversized input". The
    schema types the message as a plain string, so `contract/` is unchanged.

## Measurements

`#[ignore]` tests in `crates/core/tests/document_mode.rs` (`measure_*`) ingest 2 MiB documents with
the mock providers (`ScriptExtractor`, `MockEmbedder`), each in its own process under
`/usr/bin/time -l`. **Debug build, so these are an upper bound on release cost.** macOS, arm64.
A small-document test process in the same harness sits at 447 MB RSS (the database buffer pool), so
the document's own cost is the difference.

| 2 MiB document | Logical sections | Extractor calls | End to end | Peak RSS |
|---|---|---|---|---|
| Realistic markdown prose | 228 | 456 | 40.9 s | 725 MB |
| One section, an unclosed code fence | 1 | 351 | 17.5 s | 582 MB |
| One section, many small tables | 1 | 352 | 28.5 s | 654 MB |
| One section, a single 2 MiB line | 1 | 351 | 18.4 s | 581 MB |

Chunker alone (the part that could be super-linear on adversarial structure), same 2 MiB inputs:
many headings (70,646 sections) 316 ms; unclosed fence 35 ms; many tables 247 ms; unclosed front
matter 40 ms; single line 52 ms; realistic 21 ms. No shape is super-linear.

Reading the numbers: the server's own cost at 2 MiB is under a minute and about 135 to 280 MB above
baseline with a free extractor, which is acceptable for a cap an operator can lower. The real cost
of a large document is the extractor: about 350 to 450 calls at the default
`LCG_DOCUMENT_CHUNK_MAX_CHARS`, each an LLM call in production, and a document made of tens of
thousands of tiny sections is one call each (70,646 for the pathological case, about four times as
many as at the old 512 KiB cap). An unclosed code fence or a 2 MiB single line becomes one logical
section that is then split into units, so it is bounded by the unit size, not by the structure.
That cost scales with the document and is the reason the cap exists and stays operator-tunable.

## Consequences

- A 1.5 MB web page is accepted by default; a 3 MB one is refused, naming the cap and the variable;
  `LCG_DOCUMENT_MAX_BYTES=4MiB` accepts it.
- Operators with a stray `LCG_DOCUMENT_MAX_BYTES` must fix it before upgrading.
- Tests that need a different cap use `chunk_for_ingest_with_cap`; the install-once cell is covered
  by a process-level test (`crates/service/tests/document_max_bytes_env.rs`).
- ADR-0785 is not edited; this ADR is the current statement of the cap.
