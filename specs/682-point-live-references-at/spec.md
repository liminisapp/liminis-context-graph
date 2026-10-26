# Feature Specification: Point live references at liminisapp and docs.liminis.app after the org move

**Feature Branch**: `fabrik/issue-682`
**Created**: 2026-10-04
**Status**: Specified
**Input**: User description: "Point live references at liminisapp and docs.liminis.app after the org move"

## Background

This repository moved from `verveguy/liminis-context-graph` to **`liminisapp/liminis-context-graph`** on 2026-10-04, and its docs moved from `v3rv.com/liminis-context-graph/` to **`https://docs.liminis.app/liminis-context-graph/`**. The old URLs redirect, so nothing is broken today, but live references should point at the new homes. They are what users copy (install commands), what package registries and `--help` show (crate metadata, CLI help text), and what the docs site uses to build canonical URLs, edit links and sitemaps. Leaving them on redirects adds a hop to every install and keeps the old org in user-visible metadata.

Historical references (ADRs, release notes, changelog, history and spike docs, specs, fixtures, issue/PR links) record where things were and must be left alone.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Install commands use the canonical repo (Priority: P1)

A new user copies a `curl … github.com/<org>/liminis-context-graph/releases/…` command from the README, getting-started guide or the local-inference README.

**Why this priority**: These commands are copy-pasted verbatim; they should not depend on a redirect.

**Independent Test**: Grep the three files for `verveguy/liminis-context-graph`; no install command matches, and each now names `liminisapp`.

**Acceptance Scenarios**:

1. **Given** `README.md`, `docs/getting-started.md` and `native/local-inference/README.md`, **When** inspecting every `curl` release-download command, **Then** each uses `github.com/liminisapp/liminis-context-graph/releases/…`.

---

### User Story 2 - Crate metadata and CLI help show the new homes (Priority: P1)

A user runs `lcg-service --help` or the eval CLI's `--help`, or inspects crate metadata.

**Why this priority**: Machine-readable and in-product references are the most durable ones.

**Independent Test**: `cargo metadata --no-deps` and both `--help` outputs.

**Acceptance Scenarios**:

1. **Given** the workspace, **When** running `cargo metadata --no-deps`, **Then** `repository` is `https://github.com/liminisapp/liminis-context-graph`.
2. **Given** the built binaries, **When** running `lcg-service --help` and the eval CLI's `--help`, **Then** the `Documentation:` line shows the new docs URL (`https://docs.liminis.app/liminis-context-graph/`) or the `liminisapp` repo URL.
3. **Given** `crates/core/scripts/capture_real_corpus.py`, **When** reading `USER_AGENT`, **Then** its contact URL is the `liminisapp` repo.

---

### User Story 3 - Docs site builds for the new host without changing paths (Priority: P1)

A maintainer builds the docs site, both normally and with `DOCS_BASE=/liminis-context-graph/v0.0.0-baseurl-check`.

**Why this priority**: The site config drives canonical URLs; a wrong host or a changed base path would break links and the versioned-copy scheme (ADR-0477).

**Independent Test**: Build the site both ways; inspect the emitted canonical/sitemap host and base paths.

**Acceptance Scenarios**:

1. **Given** `site/astro.config.mjs`, **When** the site is built, **Then** `site` is `https://docs.liminis.app`, the `GITHUB` constant points at `liminisapp`, and the user-site comment is accurate.
2. **Given** the same config, **When** inspecting `siteBase()` and `DOCS_BASE`, **Then** they are unchanged and base paths remain `/liminis-context-graph` (versioned copies at `/liminis-context-graph/vX.Y.Z/`).
3. **Given** `DOCS_BASE=/liminis-context-graph/v0.0.0-baseurl-check`, **When** building, **Then** the build succeeds.

---

### User Story 4 - Generated llms files and docs-drift check stay consistent (Priority: P2)

A maintainer regenerates `docs/llms.txt` and `docs/llms-full.txt`.

**Why this priority**: These are generated artifacts guarded by a drift check; hand edits would fail it.

**Independent Test**: Run the generator script; the docs-drift check passes and the files contain the new host and org.

**Acceptance Scenarios**:

1. **Given** the updated `scripts/generate-docs-llms-full.sh` (`SITE_URL` → `https://docs.liminis.app/liminis-context-graph`, GitHub links → `liminisapp`, comment updated), **When** the script is run, **Then** `docs/llms.txt` and `docs/llms-full.txt` are rewritten by it (not hand-edited) and match its output.

---

### User Story 5 - User-facing docs and security links point at the new homes (Priority: P2)

A reader follows a docs, security-advisory, contributing or code-of-conduct link.

**Why this priority**: Consistency; avoids redirect hops and stale org names.

**Independent Test**: The final repo-wide grep (SC-001).

**Acceptance Scenarios**:

1. **Given** current docs (`docs/*.md`), `README.md` and `CONTRIBUTING.md`, **When** inspecting links, **Then** `v3rv.com/liminis-context-graph/…` became `docs.liminis.app/liminis-context-graph/…` and non-issue `github.com/verveguy/liminis-context-graph/…` links became `liminisapp`.
2. **Given** `SECURITY.md` and the README security line, **Then** the advisory link targets `liminisapp`.
3. **Given** `docs/release-process.md`, **Then** post-release check URLs use the new host.
4. **Given** `CODE_OF_CONDUCT.md`, **Then** its reporting link (currently the old repo's security-advisory URL) targets `liminisapp`.

---

### Edge Cases

- Issue/PR/discussion links (`verveguy/liminis-context-graph/issues/NNN`) in live docs stay unchanged, even in files otherwise being updated.
- Links to the private Liminis app repo (`verveguy/liminis`) stay unchanged.
- Anything under `.github/workflows/` stays untouched, including `--assignee verveguy` and historical comments.
- Docs-site `base` paths stay `/liminis-context-graph`; only the host changes.
- A line mixing a live link and an issue link must have only the live link rewritten.
- `docs/llms*.txt` must be regenerated from the script, since they embed the host and org URLs.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: `Cargo.toml` `repository` MUST be `https://github.com/liminisapp/liminis-context-graph`.
- **FR-002**: Every `curl … github.com/verveguy/liminis-context-graph/releases/…` install command in `README.md`, `docs/getting-started.md` and `native/local-inference/README.md` MUST use `liminisapp`.
- **FR-003**: The advisory link in `SECURITY.md` and the README security line MUST use `liminisapp`.
- **FR-004**: The `Documentation:` URLs in `crates/service/src/cli.rs` and `crates/eval/src/cli.rs` MUST point at `https://docs.liminis.app/liminis-context-graph/` (or the `liminisapp` repo URL where a repo link is the intent), and `USER_AGENT` in `crates/core/scripts/capture_real_corpus.py` MUST use `liminisapp`.
- **FR-005**: `site/astro.config.mjs` MUST set `site: 'https://docs.liminis.app'`, point `GITHUB` at `liminisapp`, and update the comment about the user site. `siteBase()` and `DOCS_BASE` MUST NOT change (ADR-0477).
- **FR-006**: `scripts/generate-docs-llms-full.sh` MUST use `SITE_URL="https://docs.liminis.app/liminis-context-graph"`, `liminisapp` GitHub links, and an updated comment; `docs/llms.txt` and `docs/llms-full.txt` MUST then be regenerated with the script, not hand-edited.
- **FR-007**: Current docs (`docs/*.md`, excluding `docs/adr/**`, `docs/history/**`, `docs/spikes/**`, `docs/releases/**`), `README.md` and `CONTRIBUTING.md` MUST have `v3rv.com/liminis-context-graph/…` rewritten to `docs.liminis.app/liminis-context-graph/…` and non-issue `github.com/verveguy/liminis-context-graph/…` links rewritten to `liminisapp`; `docs/release-process.md` post-release check URLs MUST use the new host.
- **FR-008**: `CODE_OF_CONDUCT.md`'s reporting link MUST be updated since it points at the old repo.
- **FR-009**: Historical references MUST remain unchanged: ADRs, `docs/releases/**`, `CHANGELOG*`, history and spike docs, specs, test-fixture READMEs, and issue/PR/discussion links.
- **FR-010**: Nothing under `.github/workflows/` MAY be edited, and references to `verveguy/liminis` (private app repo) MUST be left as-is.
- **FR-011**: Docs-site `base` paths MUST remain exactly `/liminis-context-graph`.

### Key Entities

- **Live reference**: a URL a user copies, a tool consumes, or a build uses (metadata, install commands, help text, site config, generators, current docs).
- **Historical reference**: a URL inside a record of past state (ADRs, release notes, changelog, history/spikes, specs, fixtures, issue/PR links) that is intentionally preserved.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: `git grep -nE 'v3rv\.com/liminis-context-graph|github\.com/verveguy/liminis-context-graph'` lists only historical references (ADRs, release notes, changelog, history/spikes, specs, fixtures, `.github/workflows/**`, issue/PR links, and pre-spec `ideas/`).
- **SC-002**: `cargo metadata --no-deps` shows the new `repository`.
- **SC-003**: `lcg-service --help` and the eval CLI's `--help` print the new URLs.
- **SC-004**: The docs-drift check passes after regeneration (generated llms files match the script's output).
- **SC-005**: The docs site builds, including with `DOCS_BASE=/liminis-context-graph/v0.0.0-baseurl-check`.
- **SC-006**: `git diff` shows zero changes under `.github/workflows/`, `docs/adr/**`, `docs/releases/**`, `docs/history/**`, `docs/spikes/**` and `CHANGELOG*`.

## Assumptions

- Old URLs keep redirecting indefinitely, which is why historical references can stay.
- Two live references not named in the issue are the same kind as those that are: `REPO` default in `scripts/docs-publish-latest-stable-version.sh` and `REPOSITORY` in `site/scripts/sync-docs.mjs` (used for "Edit this page" links), and `site_repository` in `scripts/generate-docs-llms-full.sh`. They are treated as live generator/config references and updated to `liminisapp`, provided the docs-publish script's API call (`gh api repos/<REPO>/releases`) resolves via the new owner (it does, since the repo now lives there).
- `ideas/self-describing-group-ontology.md` is a pre-spec sketch and is left unchanged.
- Where help text previously linked the repo, either the docs URL or the `liminisapp` repo URL is acceptable; the docs URL is preferred for a `Documentation:` line.
- Only the host changes for the docs site; versioned copies continue under `/liminis-context-graph/vX.Y.Z/`.

## Out of Scope

- Editing ADRs, release notes, changelog, history/spike docs, specs, or test-fixture READMEs.
- Editing anything under `.github/workflows/`.
- References to the private `verveguy/liminis` app repo.
- Changing docs-site base paths, `siteBase()`, or `DOCS_BASE`.
- Any behavior change beyond URL/host/org strings.

## Source References

- `Cargo.toml`, `README.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`
- `crates/service/src/cli.rs`, `crates/eval/src/cli.rs`, `crates/core/scripts/capture_real_corpus.py`
- `site/astro.config.mjs`, `site/scripts/sync-docs.mjs`
- `scripts/generate-docs-llms-full.sh`, `scripts/docs-publish-latest-stable-version.sh`
- ADR-0477 (versioned docs copies)
