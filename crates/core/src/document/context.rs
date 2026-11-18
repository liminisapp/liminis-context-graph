//! The `<DOCUMENT_CONTEXT>` block a document-mode extraction receives (issue #785, FR-011).
//!
//! It carries the title, source path, origin, the chunk's heading path and a short document map,
//! and is placed *before* the source-text tag of the extraction prompt. The "context only"
//! instruction lives inside the block itself, so the system prompts — and with them every
//! chunk-mode prompt and cassette key — stay byte-identical (FR-019).

use super::chunker::HeadingEntry;

/// Headings shown in the document map before it is windowed around the current section.
pub const MAP_MAX_ENTRIES: usize = 40;

/// Everything the block describes for one extraction unit.
pub struct ContextInput<'a> {
    pub title: Option<&'a str>,
    pub source_path: Option<&'a str>,
    pub origin: &'a str,
    pub heading_path: &'a [String],
    pub map: &'a [HeadingEntry],
    /// Byte offset of the chunk; the map entry starting there is marked as the current section.
    pub chunk_byte_start: usize,
    /// `(1-based unit, unit count)` when the section was split.
    pub part: Option<(usize, usize)>,
    pub table_header: Option<&'a str>,
}

fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Renders the block, tags included.
pub fn render(input: &ContextInput<'_>) -> String {
    let mut out = String::from(
        "<DOCUMENT_CONTEXT>\n\
         This block describes the document the text below was taken from. It is context, not \
         source text: use it to interpret the text (for example, whether a passage describes a \
         proposed, rejected or chosen option), but do not extract entities or facts from this \
         block itself. Extract only from the text that follows it.\n",
    );
    if let Some(t) = input.title.map(one_line).filter(|t| !t.is_empty()) {
        out.push_str(&format!("Title: {t}\n"));
    }
    if let Some(p) = input.source_path.map(one_line).filter(|p| !p.is_empty()) {
        out.push_str(&format!("Source: {p}\n"));
    }
    out.push_str(&format!("Origin: {}\n", input.origin));
    let path: Vec<String> = input.heading_path.iter().map(|h| one_line(h)).collect();
    if path.is_empty() {
        out.push_str("Section: (document preamble, before the first heading)\n");
    } else {
        out.push_str(&format!("Section: {}\n", path.join(" > ")));
    }
    if let Some((k, n)) = input.part {
        out.push_str(&format!(
            "Part: {k} of {n} (the section was split; the other parts are extracted separately)\n"
        ));
    }
    if let Some(h) = input.table_header {
        out.push_str("Table header (the text continues this table):\n");
        out.push_str(h);
        out.push('\n');
    }
    if !input.map.is_empty() {
        out.push_str("Document map:\n");
        let current = input
            .map
            .iter()
            .position(|h| h.byte_start == input.chunk_byte_start);
        let (lo, hi) = if input.map.len() <= MAP_MAX_ENTRIES {
            (0, input.map.len())
        } else {
            let centre = current.unwrap_or(0);
            let lo = centre
                .saturating_sub(MAP_MAX_ENTRIES / 2)
                .min(input.map.len() - MAP_MAX_ENTRIES);
            (lo, lo + MAP_MAX_ENTRIES)
        };
        if lo > 0 {
            out.push_str(&format!("  ({lo} earlier headings omitted)\n"));
        }
        let min_level = input.map.iter().map(|h| h.level).min().unwrap_or(1);
        for (i, h) in input.map.iter().enumerate().take(hi).skip(lo) {
            let indent = "  ".repeat(usize::from(h.level.saturating_sub(min_level)));
            let marker = if Some(i) == current {
                "  <- this section"
            } else {
                ""
            };
            out.push_str(&format!("{indent}- {}{marker}\n", one_line(&h.text)));
        }
        if hi < input.map.len() {
            out.push_str(&format!(
                "  ({} later headings omitted)\n",
                input.map.len() - hi
            ));
        }
    }
    out.push_str("</DOCUMENT_CONTEXT>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(level: u8, text: &str, at: usize) -> HeadingEntry {
        HeadingEntry {
            level,
            text: text.into(),
            byte_start: at,
        }
    }

    #[test]
    fn renders_title_path_origin_and_marked_map() {
        let map = vec![
            h(1, "Decision", 0),
            h(2, "Options", 12),
            h(2, "Outcome", 40),
        ];
        let path = vec!["Decision".to_string(), "Options".to_string()];
        let s = render(&ContextInput {
            title: Some("ADR 7"),
            source_path: Some("notes/adr7.md"),
            origin: "derived",
            heading_path: &path,
            map: &map,
            chunk_byte_start: 12,
            part: None,
            table_header: None,
        });
        assert!(s.starts_with("<DOCUMENT_CONTEXT>\n") && s.ends_with("</DOCUMENT_CONTEXT>"));
        assert!(s.contains("Title: ADR 7\n"));
        assert!(s.contains("Source: notes/adr7.md\n"));
        assert!(s.contains("Origin: derived\n"));
        assert!(s.contains("Section: Decision > Options\n"));
        assert!(s.contains("  - Options  <- this section\n"));
        assert!(s.contains("- Decision\n"));
        assert!(s.contains("do not extract entities or facts from this block"));
    }

    #[test]
    fn long_maps_are_windowed_around_the_current_section() {
        let map: Vec<HeadingEntry> = (0..100).map(|i| h(2, &format!("H{i}"), i * 10)).collect();
        let s = render(&ContextInput {
            title: None,
            source_path: None,
            origin: "primary",
            heading_path: &["H70".to_string()],
            map: &map,
            chunk_byte_start: 700,
            part: Some((2, 3)),
            table_header: Some("| a |\n|---|"),
        });
        assert!(s.contains("- H70  <- this section"));
        assert!(s.contains("(50 earlier headings omitted)"));
        assert!(s.contains("(10 later headings omitted)"));
        assert!(s.contains("Part: 2 of 3"));
        assert!(s.contains("Table header (the text continues this table):\n| a |\n|---|\n"));
        assert!(!s.contains("Title:"));
        assert_eq!(s.matches("\n- H").count() + s.matches("  - H").count(), 40);
    }

    #[test]
    fn preamble_has_no_marked_section() {
        let map = vec![h(1, "A", 50)];
        let s = render(&ContextInput {
            title: Some("T\nwith newline"),
            source_path: None,
            origin: "primary",
            heading_path: &[],
            map: &map,
            chunk_byte_start: 0,
            part: None,
            table_header: None,
        });
        assert!(s.contains("Section: (document preamble"));
        assert!(s.contains("Title: T with newline\n"));
        assert!(!s.contains("<- this section"));
    }
}
