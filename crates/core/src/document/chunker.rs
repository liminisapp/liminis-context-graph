//! The document-mode markdown chunker (issue #785, ADR-0785).
//!
//! Hand-rolled and line-oriented on purpose: it must report exact byte offsets into the submitted
//! revision and must never drop content, and a CommonMark event parser does neither by
//! construction (link reference definitions and front matter produce no events). Boundary
//! imperfection is cheap — ADR-0705 makes re-indexing additive — so the grammar is deliberately
//! small:
//!
//! * **Blocks** — ATX and (single-line) setext headings, fenced code (an unclosed fence runs to
//!   EOF; headings inside a fence are text), YAML front matter at byte 0, pipe tables (a header
//!   row followed by a delimiter row), list groups with their continuation lines, and paragraph
//!   runs for everything else (blockquotes, HTML, thematic breaks, plain text).
//! * **Sections** — a heading plus every block up to the next heading of *any* level, so
//!   subsections are their own chunks. Blocks before the first heading form the preamble, whose
//!   heading path is empty. A section with no body produces no chunk; its heading still reaches
//!   the extractor through every descendant's heading path and the document map.
//! * **Units** — a section longer than `max_chars` characters is split into units that exactly
//!   tile it: at block boundaries first, then list-item / table-row starts, then line starts, then
//!   whitespace, then character boundaries. All units of a section share its identity and hash.
//!
//! Invariants (pinned by the property test below): chunk spans are ordered and non-overlapping,
//! `doc[byte_start..byte_end] == text`, units tile their chunk, every offset is a UTF-8 boundary,
//! and every non-whitespace byte lies in exactly one chunk unless it is on the heading line of a
//! body-less section.

use serde::Serialize;

/// Bumped whenever a boundary rule changes, so a revision records which grammar chunked it.
pub const CHUNKER_VERSION: u32 = 1;

/// One heading of the document, in document order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeadingEntry {
    pub level: u8,
    pub text: String,
    pub byte_start: usize,
}

/// One extraction unit of a logical chunk: a contiguous byte span of the revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnitSpan {
    pub byte_start: usize,
    pub byte_end: usize,
    /// The header and delimiter rows of the table this unit starts inside, when the split cut a
    /// table (context only — never part of the unit's source text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_header: Option<String>,
}

/// One logical chunk (a section) of a document revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogicalChunk {
    pub index: usize,
    pub heading_path: Vec<String>,
    pub byte_start: usize,
    pub byte_end: usize,
    #[serde(skip)]
    pub text: String,
    /// Lowercase-hex SHA-256 of `text`: the diff's match key.
    pub chunk_hash: String,
    pub units: Vec<UnitSpan>,
}

impl LogicalChunk {
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }
}

/// The chunking of one document revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChunkedDocument {
    pub chunks: Vec<LogicalChunk>,
    pub map: Vec<HeadingEntry>,
    pub byte_len: usize,
    pub chunker_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Heading,
    Fence,
    FrontMatter,
    Table,
    List,
    Paragraph,
}

#[derive(Debug, Clone)]
struct Block {
    kind: BlockKind,
    /// Byte offset of the block's first line.
    start: usize,
    /// Byte offset just past the block's last non-terminator byte.
    end: usize,
    /// Heading level and text, for `Heading`.
    heading: Option<(u8, String)>,
    /// Byte starts of inner split points (list items, table rows), excluding `start`.
    items: Vec<usize>,
    /// Byte starts of every line after the first.
    lines: Vec<usize>,
}

/// One physical line: `start..content_end` is the line without its terminator.
#[derive(Debug, Clone, Copy)]
struct Line {
    start: usize,
    content_end: usize,
}

fn split_lines(doc: &str) -> Vec<Line> {
    let bytes = doc.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let nl = bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|p| start + p);
        let (mut content_end, next) = match nl {
            Some(p) => (p, p + 1),
            None => (bytes.len(), bytes.len()),
        };
        if content_end > start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        out.push(Line { start, content_end });
        start = next;
    }
    out
}

fn text<'a>(doc: &'a str, l: &Line) -> &'a str {
    &doc[l.start..l.content_end]
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// Leading-space count, or `None` when indented four or more (an indented code line).
fn indent_le3(s: &str) -> Option<&str> {
    let spaces = s.len() - s.trim_start_matches(' ').len();
    (spaces <= 3).then(|| &s[spaces..])
}

fn atx_heading(s: &str) -> Option<(u8, String)> {
    let rest = indent_le3(s)?;
    let hashes = rest.len() - rest.trim_start_matches('#').len();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let after = &rest[hashes..];
    if !(after.is_empty() || after.starts_with(' ') || after.starts_with('\t')) {
        return None;
    }
    let mut t = after.trim();
    // An optional closing sequence of `#`s, preceded by whitespace (or the whole content).
    let stripped = t.trim_end_matches('#');
    if stripped.len() != t.len() && (stripped.is_empty() || stripped.ends_with([' ', '\t'])) {
        t = stripped.trim_end();
    }
    Some((hashes as u8, t.to_string()))
}

/// The fence character and run length of a fence-opening line.
fn fence_open(s: &str) -> Option<(char, usize)> {
    let rest = indent_le3(s)?;
    let c = rest.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let run = rest.len() - rest.trim_start_matches(c).len();
    if run < 3 {
        return None;
    }
    // A backtick fence's info string may not contain a backtick.
    if c == '`' && rest[run..].contains('`') {
        return None;
    }
    Some((c, run))
}

fn fence_close(s: &str, c: char, run: usize) -> bool {
    let Some(rest) = indent_le3(s) else {
        return false;
    };
    let n = rest.len() - rest.trim_start_matches(c).len();
    n >= run && rest[n..].trim().is_empty()
}

fn setext_level(s: &str) -> Option<u8> {
    let rest = indent_le3(s)?.trim_end();
    if !rest.is_empty() && rest.chars().all(|c| c == '=') {
        return Some(1);
    }
    if rest.len() >= 2 && rest.chars().all(|c| c == '-') {
        return Some(2);
    }
    None
}

fn list_item(s: &str) -> bool {
    let Some(rest) = indent_le3(s) else {
        return false;
    };
    match rest.chars().next() {
        Some(b @ ('-' | '*' | '+')) => {
            let after = &rest[1..];
            if !(after.is_empty() || after.starts_with([' ', '\t'])) {
                return false;
            }
            // `- - -` / `***` are thematic breaks, not list items.
            let marks = rest.chars().filter(|&c| c == b).count();
            !(marks >= 3 && rest.trim().chars().all(|c| c == b || c == ' ' || c == '\t'))
        }
        Some(d) if d.is_ascii_digit() => {
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if digits > 9 {
                return false;
            }
            let after = &rest[digits..];
            (after.starts_with('.') || after.starts_with(')'))
                && (after.len() == 1 || after[1..].starts_with([' ', '\t']))
        }
        _ => false,
    }
}

fn table_delimiter(s: &str) -> bool {
    let t = s.trim();
    // A pipe is required, so a setext `---` underline is never read as a one-column delimiter.
    if !t.contains('-') || !t.contains('|') {
        return false;
    }
    let inner = t.trim_start_matches('|').trim_end_matches('|');
    if inner.is_empty() {
        return false;
    }
    inner.split('|').all(|cell| {
        let c = cell.trim();
        let c = c.strip_prefix(':').unwrap_or(c);
        let c = c.strip_suffix(':').unwrap_or(c);
        !c.is_empty() && c.chars().all(|ch| ch == '-')
    })
}

fn table_start(lines: &[Line], doc: &str, i: usize) -> bool {
    let cur = text(doc, &lines[i]);
    cur.contains('|') && i + 1 < lines.len() && table_delimiter(text(doc, &lines[i + 1]))
}

/// Whether line `i` starts a block that interrupts a paragraph.
fn interrupts(lines: &[Line], doc: &str, i: usize) -> bool {
    let s = text(doc, &lines[i]);
    atx_heading(s).is_some()
        || fence_open(s).is_some()
        || list_item(s)
        || table_start(lines, doc, i)
}

fn scan_blocks(doc: &str) -> Vec<Block> {
    let lines = split_lines(doc);
    let mut blocks = Vec::new();
    let mut i = 0usize;
    let push = |blocks: &mut Vec<Block>,
                kind: BlockKind,
                first: usize,
                last: usize,
                heading: Option<(u8, String)>,
                items: Vec<usize>| {
        blocks.push(Block {
            kind,
            start: lines[first].start,
            end: lines[last].content_end,
            heading,
            items,
            lines: lines[first + 1..=last].iter().map(|l| l.start).collect(),
        });
    };

    // Front matter: `---` at byte 0, closed by `---` or `...`.
    if !lines.is_empty() && text(doc, &lines[0]).trim_end() == "---" {
        if let Some(close) = (1..lines.len()).find(|&j| {
            let t = text(doc, &lines[j]).trim_end();
            t == "---" || t == "..."
        }) {
            push(
                &mut blocks,
                BlockKind::FrontMatter,
                0,
                close,
                None,
                Vec::new(),
            );
            i = close + 1;
        }
    }

    while i < lines.len() {
        let s = text(doc, &lines[i]);
        if is_blank(s) {
            i += 1;
            continue;
        }
        if let Some(h) = atx_heading(s) {
            push(&mut blocks, BlockKind::Heading, i, i, Some(h), Vec::new());
            i += 1;
            continue;
        }
        if let Some((c, run)) = fence_open(s) {
            let close = (i + 1..lines.len()).find(|&j| fence_close(text(doc, &lines[j]), c, run));
            let last = close.unwrap_or(lines.len() - 1);
            push(&mut blocks, BlockKind::Fence, i, last, None, Vec::new());
            i = last + 1;
            continue;
        }
        if table_start(&lines, doc, i) {
            let mut last = i + 1;
            while last + 1 < lines.len() {
                let t = text(doc, &lines[last + 1]);
                if is_blank(t) || !t.contains('|') {
                    break;
                }
                last += 1;
            }
            // Row starts after the delimiter row are the table's inner split points.
            let items = (i + 2..=last).map(|j| lines[j].start).collect();
            push(&mut blocks, BlockKind::Table, i, last, None, items);
            i = last + 1;
            continue;
        }
        if list_item(s) {
            let mut last = i;
            let mut items = Vec::new();
            let mut j = i + 1;
            while j < lines.len() {
                let t = text(doc, &lines[j]);
                if is_blank(t) {
                    // A blank line continues the list only when the next non-blank line is
                    // another item or an indented continuation.
                    let Some(k) = (j + 1..lines.len()).find(|&k| !is_blank(text(doc, &lines[k])))
                    else {
                        break;
                    };
                    let next = text(doc, &lines[k]);
                    let indented = next.starts_with("  ") || next.starts_with('\t');
                    if list_item(next) || indented {
                        j = k;
                        continue;
                    }
                    break;
                }
                if atx_heading(t).is_some() || (fence_open(t).is_some() && indent_le3(t) == Some(t))
                {
                    break;
                }
                if list_item(t) && indent_le3(t).is_some_and(|r| r.len() == t.len()) {
                    items.push(lines[j].start);
                }
                last = j;
                j += 1;
            }
            push(&mut blocks, BlockKind::List, i, last, None, items);
            i = last + 1;
            continue;
        }
        // Single-line setext heading: a plain line directly followed by an underline.
        if i + 1 < lines.len() {
            if let Some(level) = setext_level(text(doc, &lines[i + 1])) {
                push(
                    &mut blocks,
                    BlockKind::Heading,
                    i,
                    i + 1,
                    Some((level, s.trim().to_string())),
                    Vec::new(),
                );
                i += 2;
                continue;
            }
        }
        // Paragraph run.
        let mut last = i;
        while last + 1 < lines.len() {
            let t = text(doc, &lines[last + 1]);
            if is_blank(t) || interrupts(&lines, doc, last + 1) {
                break;
            }
            last += 1;
        }
        push(&mut blocks, BlockKind::Paragraph, i, last, None, Vec::new());
        i = last + 1;
    }
    blocks
}

/// Greedy packing of `[s, e)` into spans of at most `max` characters, cutting only at the
/// boundaries of the first level that has any inside the range, recursing into a piece that is
/// still too long. The spans exactly tile `[s, e)`.
fn pack(doc: &str, s: usize, e: usize, max: usize, levels: &[Vec<usize>]) -> Vec<(usize, usize)> {
    let chars = |a: usize, b: usize| doc[a..b].chars().count();
    if chars(s, e) <= max {
        return vec![(s, e)];
    }
    let Some((level, rest)) = levels.split_first() else {
        return pack_whitespace(doc, s, e, max);
    };
    let mut cuts: Vec<usize> = level.iter().copied().filter(|&b| b > s && b < e).collect();
    if cuts.is_empty() {
        return pack(doc, s, e, max, rest);
    }
    cuts.push(e);
    let mut out = Vec::new();
    let mut cur = s;
    let mut cur_chars = 0usize;
    let mut piece_start = s;
    for b in cuts {
        let piece_chars = chars(piece_start, b);
        if piece_chars > max {
            if cur < piece_start {
                out.push((cur, piece_start));
            }
            out.extend(pack(doc, piece_start, b, max, rest));
            cur = b;
            cur_chars = 0;
        } else if cur_chars + piece_chars > max {
            out.push((cur, piece_start));
            cur = piece_start;
            cur_chars = piece_chars;
        } else {
            cur_chars += piece_chars;
        }
        piece_start = b;
    }
    if cur < e {
        out.push((cur, e));
    }
    out
}

/// Last-resort split: after whitespace where possible, otherwise at character boundaries.
fn pack_whitespace(doc: &str, s: usize, e: usize, max: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cur = s;
    while doc[cur..e].chars().count() > max {
        // Byte offset just past the `max`-th character from `cur`.
        let limit = doc[cur..e]
            .char_indices()
            .nth(max)
            .map(|(i, _)| cur + i)
            .unwrap_or(e);
        let window = &doc[cur..limit];
        let cut = window
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| cur + i + c.len_utf8())
            .filter(|&p| p > cur && p < limit)
            .unwrap_or(limit);
        out.push((cur, cut));
        cur = cut;
    }
    if cur < e {
        out.push((cur, e));
    }
    out
}

/// Folds a whitespace-only span into its predecessor (or, first in line, its successor), so no
/// unit is extracted that has nothing to extract. Tiling is preserved; a unit may then exceed
/// the limit by trailing whitespace only.
fn merge_blank_units(doc: &str, spans: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    let mut carry: Option<usize> = None;
    for (s, e) in spans {
        if doc[s..e].trim().is_empty() {
            match out.last_mut() {
                Some(prev) => prev.1 = e,
                None => carry = Some(carry.unwrap_or(s)),
            }
            continue;
        }
        out.push((carry.take().unwrap_or(s), e));
    }
    out
}

/// Chunks `doc`. `max_chars` (≥ 1) bounds every unit's character count, trailing whitespace
/// aside.
pub fn chunk_document(doc: &str, max_chars: usize) -> ChunkedDocument {
    let max_chars = max_chars.max(1);
    let blocks = scan_blocks(doc);
    let mut map = Vec::new();
    let mut chunks: Vec<LogicalChunk> = Vec::new();
    // Heading stack: (level, text).
    let mut stack: Vec<(u8, String)> = Vec::new();

    // Group blocks into sections: a heading block opens one; the preamble has none.
    let mut sections: Vec<(Option<usize>, Vec<usize>, Vec<String>)> = Vec::new();
    let mut current: (Option<usize>, Vec<usize>, Vec<String>) = (None, Vec::new(), Vec::new());
    for (bi, b) in blocks.iter().enumerate() {
        if b.kind == BlockKind::Heading {
            let (level, htext) = b.heading.clone().unwrap_or((1, String::new()));
            map.push(HeadingEntry {
                level,
                text: htext.clone(),
                byte_start: b.start,
            });
            sections.push(std::mem::take(&mut current));
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            stack.push((level, htext));
            current = (
                Some(bi),
                Vec::new(),
                stack.iter().map(|(_, t)| t.clone()).collect(),
            );
        } else {
            current.1.push(bi);
        }
    }
    sections.push(current);

    for (heading, body, heading_path) in sections {
        let Some(&last) = body.last() else {
            continue; // Body-less section (or empty preamble): no chunk.
        };
        let start = heading.map_or(blocks[body[0]].start, |h| blocks[h].start);
        let end = blocks[last].end;
        // Split levels: block starts (the first body block stays with its heading), inner item
        // starts, then every line start.
        let block_starts: Vec<usize> = body.iter().skip(1).map(|&b| blocks[b].start).collect();
        let mut item_starts: Vec<usize> = Vec::new();
        let mut line_starts: Vec<usize> = Vec::new();
        for &b in heading.iter().chain(body.iter()) {
            item_starts.extend(&blocks[b].items);
            line_starts.extend(&blocks[b].lines);
            line_starts.push(blocks[b].start);
        }
        item_starts.sort_unstable();
        line_starts.sort_unstable();
        line_starts.dedup();
        let spans = merge_blank_units(
            doc,
            pack(
                doc,
                start,
                end,
                max_chars,
                &[block_starts, item_starts, line_starts],
            ),
        );
        let units = spans
            .into_iter()
            .map(|(s, e)| UnitSpan {
                byte_start: s,
                byte_end: e,
                table_header: body
                    .iter()
                    .map(|&b| &blocks[b])
                    .find(|b| b.kind == BlockKind::Table && b.start < s && s < b.end)
                    .and_then(|t| t.lines.first().map(|&l2| (t.start, l2)))
                    .map(|(ts, l2)| {
                        let second_end = doc[l2..].find('\n').map_or(doc.len(), |p| l2 + p);
                        doc[ts..second_end].trim_end_matches('\r').to_string()
                    }),
            })
            .collect();
        let text = doc[start..end].to_string();
        chunks.push(LogicalChunk {
            index: chunks.len(),
            heading_path,
            byte_start: start,
            byte_end: end,
            chunk_hash: crate::sha256_hex::sha256_hex(text.as_bytes()),
            text,
            units,
        });
    }

    ChunkedDocument {
        chunks,
        map,
        byte_len: doc.len(),
        chunker_version: CHUNKER_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(d: &ChunkedDocument) -> Vec<Vec<String>> {
        d.chunks.iter().map(|c| c.heading_path.clone()).collect()
    }

    fn texts(d: &ChunkedDocument) -> Vec<&str> {
        d.chunks.iter().map(|c| c.text.as_str()).collect()
    }

    /// The invariants the module doc promises, checked for any input.
    fn check_invariants(doc: &str, d: &ChunkedDocument, max: usize) {
        let mut prev_end = 0usize;
        let mut covered = vec![false; doc.len()];
        for c in &d.chunks {
            assert!(c.byte_start >= prev_end, "chunks overlap or are unordered");
            assert!(doc.is_char_boundary(c.byte_start) && doc.is_char_boundary(c.byte_end));
            assert_eq!(&doc[c.byte_start..c.byte_end], c.text);
            assert_eq!(
                c.chunk_hash,
                crate::sha256_hex::sha256_hex(c.text.as_bytes())
            );
            assert!(!c.units.is_empty());
            let mut u_prev = c.byte_start;
            for u in &c.units {
                assert_eq!(u.byte_start, u_prev, "units must tile their chunk");
                assert!(u.byte_end > u.byte_start);
                assert!(doc.is_char_boundary(u.byte_start) && doc.is_char_boundary(u.byte_end));
                let unit = &doc[u.byte_start..u.byte_end];
                assert!(!unit.trim().is_empty(), "whitespace-only unit");
                // A leading blank carried into the first unit, or trailing blanks folded into
                // a predecessor, are the only overshoot allowed.
                assert!(
                    unit.trim().chars().count() <= max,
                    "unit over limit: {unit:?}"
                );
                u_prev = u.byte_end;
            }
            assert_eq!(u_prev, c.byte_end, "units must tile their chunk");
            for b in covered.iter_mut().take(c.byte_end).skip(c.byte_start) {
                *b = true;
            }
            prev_end = c.byte_end;
        }
        // Every non-whitespace byte outside a chunk sits on a heading line.
        let heading_lines: Vec<(usize, usize)> = d
            .map
            .iter()
            .map(|h| {
                // A setext heading spans two lines; cover through the underline.
                let first = doc[h.byte_start..]
                    .find('\n')
                    .map_or(doc.len(), |p| h.byte_start + p + 1);
                let second = doc[first.min(doc.len())..]
                    .find('\n')
                    .map_or(doc.len(), |p| first + p + 1);
                (h.byte_start, second)
            })
            .collect();
        for (i, &c) in doc.as_bytes().iter().enumerate() {
            if covered[i] || (c as char).is_ascii_whitespace() {
                continue;
            }
            assert!(
                heading_lines.iter().any(|(s, e)| i >= *s && i < *e),
                "byte {i} ({:?}) dropped from {doc:?}",
                c as char
            );
        }
    }

    #[test]
    fn sections_split_at_every_heading_level() {
        let doc = "Intro text.\n\n# Title\n\nBody one.\n\n## Sub\n\nBody two.\n\n### Deep\n\nBody three.\n\n## Other\n\nBody four.\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(
            paths(&d),
            vec![
                vec![],
                vec!["Title".to_string()],
                vec!["Title".to_string(), "Sub".to_string()],
                vec!["Title".to_string(), "Sub".to_string(), "Deep".to_string()],
                vec!["Title".to_string(), "Other".to_string()],
            ]
        );
        assert_eq!(texts(&d)[0], "Intro text.");
        assert_eq!(texts(&d)[1], "# Title\n\nBody one.");
        assert_eq!(d.map.len(), 4);
    }

    #[test]
    fn body_less_heading_yields_no_chunk_but_stays_in_path_and_map() {
        let doc = "# Decision\n## Options\n\nWe chose Z.\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(d.chunks.len(), 1);
        assert_eq!(d.chunks[0].heading_path, vec!["Decision", "Options"]);
        assert_eq!(d.map.len(), 2);
    }

    #[test]
    fn headings_only_document_has_no_chunks() {
        let d = chunk_document("# A\n## B\n", 6000);
        assert!(d.chunks.is_empty());
        assert!(chunk_document("", 6000).chunks.is_empty());
        assert!(chunk_document("  \n\n\t\n", 6000).chunks.is_empty());
    }

    #[test]
    fn setext_headings_are_recognised() {
        let doc = "Title\n=====\n\nBody.\n\nSub\n---\n\nMore.\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(paths(&d), vec![vec!["Title"], vec!["Title", "Sub"]]);
    }

    #[test]
    fn headings_inside_fences_are_text_and_unclosed_fence_runs_to_eof() {
        let doc = "# Real\n\n```md\n# not a heading\n```\n\nAfter.\n\n## Two\n\n~~~\n# also not\nstill code";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(paths(&d), vec![vec!["Real"], vec!["Real", "Two"]]);
        assert!(d.chunks[1].text.ends_with("still code"));
        assert_eq!(d.map.len(), 2);
    }

    #[test]
    fn front_matter_is_preamble_content() {
        let doc = "---\ntitle: x\n---\n\n# H\n\nBody\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(d.chunks[0].heading_path, Vec::<String>::new());
        assert_eq!(d.chunks[0].text, "---\ntitle: x\n---");
        // An unclosed `---` at byte 0 is not front matter, and nothing is dropped.
        let doc2 = "---\nno close\n";
        check_invariants(doc2, &chunk_document(doc2, 6000), 6000);
    }

    #[test]
    fn crlf_and_multibyte_offsets_are_exact() {
        let doc = "# Ünïcødé\r\n\r\nCafé — naïve 😀 text.\r\n\r\n## Zwei\r\n\r\nÄrger\r\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(d.chunks[0].heading_path, vec!["Ünïcødé"]);
        assert_eq!(d.chunks[1].text, "## Zwei\r\n\r\nÄrger");
    }

    #[test]
    fn tables_and_lists_are_blocks() {
        let doc = "# T\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n| 3 | 4 |\n\n- one\n- two\n  continued\n\n- three\n\nPara.\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(d.chunks.len(), 1);
        let blocks = scan_blocks(doc);
        let kinds: Vec<BlockKind> = blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Heading,
                BlockKind::Table,
                BlockKind::List,
                BlockKind::Paragraph
            ]
        );
    }

    #[test]
    fn oversized_table_split_carries_its_header_as_context() {
        let mut doc = String::from("# T\n\n| name | value |\n|---|---|\n");
        for i in 0..40 {
            doc.push_str(&format!("| row{i} | {i} |\n"));
        }
        let d = chunk_document(&doc, 120);
        check_invariants(&doc, &d, 120);
        let c = &d.chunks[0];
        assert!(c.units.len() > 1);
        assert!(c.units[0].table_header.is_none());
        for u in &c.units[1..] {
            assert_eq!(
                u.table_header.as_deref(),
                Some("| name | value |\n|---|---|")
            );
        }
    }

    #[test]
    fn oversized_section_splits_at_blocks_then_lines_then_whitespace_then_chars() {
        // Blocks.
        let doc = format!("# S\n\n{}\n\n{}\n", "a".repeat(50), "b".repeat(50));
        let d = chunk_document(&doc, 60);
        check_invariants(&doc, &d, 60);
        assert_eq!(d.chunks[0].units.len(), 2);
        // One paragraph of many lines.
        let doc = format!("# S\n\n{}", "line of words\n".repeat(20));
        let d = chunk_document(&doc, 50);
        check_invariants(&doc, &d, 50);
        // One long line with spaces.
        let doc = format!("# S\n\n{}", "word ".repeat(100));
        let d = chunk_document(&doc, 30);
        check_invariants(&doc, &d, 30);
        // One unbroken multibyte run.
        let doc = format!("# S\n\n{}", "é".repeat(100));
        let d = chunk_document(&doc, 7);
        check_invariants(&doc, &d, 7);
        // All units share one logical chunk and one hash.
        assert_eq!(d.chunks.len(), 1);
    }

    #[test]
    fn duplicate_sections_hash_identically() {
        let doc = "# A\n\nSame.\n\n# A\n\nSame.\n";
        let d = chunk_document(doc, 6000);
        check_invariants(doc, &d, 6000);
        assert_eq!(d.chunks[0].chunk_hash, d.chunks[1].chunk_hash);
        assert_ne!(d.chunks[0].byte_start, d.chunks[1].byte_start);
    }

    #[test]
    fn deterministic_generated_corpus_keeps_every_invariant() {
        // A small LCG over fragments covering every block kind and its edge cases; no
        // dev-dependency needed and the corpus is the same on every run.
        const FRAGS: &[&str] = &[
            "# H1\n",
            "## H2 ##\n",
            "###### H6\n",
            "####### seven\n",
            "#nospace\n",
            "Setext\n===\n",
            "Under\n---\n",
            "---\n",
            "***\n",
            "```rust\nfn x() {}\n```\n",
            "```\n# inside\n",
            "~~~~\ncode\n~~~~\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n",
            "- item\n- item two\n  cont\n",
            "1. one\n2) two\n",
            "> quote\n> more\n",
            "<div>\nhtml\n</div>\n",
            "plain text line\n",
            "\n",
            "\n\n",
            "    indented # code\n",
            "Ünïcødé 😀\r\n",
            "word word word word word word word word word\n",
            "[ref]: http://x\n",
        ];
        let mut seed: u64 = 0x785;
        for case in 0..300 {
            let mut doc = String::new();
            if case % 7 == 0 {
                doc.push_str("---\nk: v\n---\n");
            }
            let n = 1 + (case % 25);
            for _ in 0..n {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                doc.push_str(FRAGS[(seed >> 33) as usize % FRAGS.len()]);
            }
            let max = 10 + (case * 13) % 200;
            let d = chunk_document(&doc, max);
            check_invariants(&doc, &d, max);
            assert_eq!(
                d,
                chunk_document(&doc, max),
                "chunking must be deterministic"
            );
        }
    }
}
