//! Parse GFM tables from a buffer using pulldown-cmark.
//!
//! Built around [`Parser::into_offset_iter`] so we get byte ranges for
//! every event — those ranges become each cell's `source_range`, which
//! is what makes minimum-diff cell patching possible.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{Alignment as PdAlignment, Event, Options, Parser, Tag, TagEnd};

use super::model::*;

/// Parse all tables in `buffer`, returning them in document order.
///
/// Tables are assigned monotonically increasing IDs starting at 1.
/// Parsed tables default to [`TableStyle::PreserveOriginal`] so
/// subsequent cell edits don't disturb user-formatted whitespace.
pub fn parse_tables(buffer: &str) -> Vec<MarkdownTable> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);

    // Collect events so we can index by position without juggling
    // lifetimes. pulldown-cmark events for table content are small
    // (no inline-text payload for the cell-boundary events we care
    // about) so the Vec is cheap even for large documents.
    let events: Vec<(Event, Range<usize>)> =
        Parser::new_ext(buffer, opts).into_offset_iter().collect();

    let mut tables = Vec::new();
    let mut next_id: TableId = 1;
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::Table(aligns)) = &events[i].0 {
            let alignments: Vec<Alignment> = aligns.iter().copied().map(map_align).collect();
            let table_range = events[i].1.clone();
            let (table, next) =
                collect_table(buffer, &events, i + 1, alignments, table_range, next_id);
            next_id += 1;
            tables.push(table);
            i = next;
        } else {
            i += 1;
        }
    }
    tables
}

fn map_align(a: PdAlignment) -> Alignment {
    match a {
        PdAlignment::None => Alignment::None,
        PdAlignment::Left => Alignment::Left,
        PdAlignment::Center => Alignment::Center,
        PdAlignment::Right => Alignment::Right,
    }
}

/// Walk events from `start` until we close the current table. Returns
/// the parsed table plus the index of the next unconsumed event.
fn collect_table(
    buf: &str,
    events: &[(Event, Range<usize>)],
    start: usize,
    alignments: Vec<Alignment>,
    table_range: Range<usize>,
    id: TableId,
) -> (MarkdownTable, usize) {
    let mut headers: Vec<Cell> = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut current_row: Vec<Cell> = Vec::new();
    let mut in_head = false;
    let mut current_cell_start: Option<usize> = None;

    let mut i = start;
    while i < events.len() {
        let (ev, r) = &events[i];
        match ev {
            Event::Start(Tag::TableHead) => in_head = true,
            Event::End(TagEnd::TableHead) => {
                headers = std::mem::take(&mut current_row);
                in_head = false;
            }
            Event::Start(Tag::TableRow) => current_row.clear(),
            Event::End(TagEnd::TableRow) => {
                if !in_head {
                    rows.push(std::mem::take(&mut current_row));
                }
            }
            Event::Start(Tag::TableCell) => {
                current_cell_start = Some(r.start);
            }
            Event::End(TagEnd::TableCell) => {
                let cell_range = current_cell_start
                    .take()
                    .map(|s| s..r.end)
                    .unwrap_or_else(|| r.clone());
                current_row.push(cell_from_range(buf, cell_range));
            }
            Event::End(TagEnd::Table) => {
                i += 1;
                break;
            }
            _ => {}
        }
        i += 1;
    }

    let n_cols = alignments.len();
    let table_src = &buf[table_range.clone()];
    // Parsed tables always default to PreserveOriginal so edits don't
    // disturb hand-formatted whitespace. The `Reformat Table` command
    // is the explicit way to promote a table to Pretty.
    let original_lines = Some(capture_original_lines(table_src));

    let table = MarkdownTable {
        id,
        source_range: table_range,
        alignments,
        headers,
        rows,
        style: TableStyle::PreserveOriginal,
        column_widths: vec![None; n_cols],
        formulas: HashMap::new(),
        original_lines,
    };
    (table, i)
}

/// Build a [`Cell`] from a byte range emitted by pulldown-cmark.
///
/// Expands the range outward to include adjacent whitespace inside the
/// cell (between the content and the surrounding pipes), so the cell's
/// `source_range` represents the full content area between pipes —
/// not just the trimmed content. This lets the serializer preserve
/// the user's intra-cell padding on edit.
pub fn cell_from_range(buf: &str, r: Range<usize>) -> Cell {
    let bytes = buf.as_bytes();
    let mut start = r.start;
    let mut end = r.end;

    // Walk left over inner whitespace until we hit the surrounding pipe.
    while start > 0 {
        let c = bytes[start - 1];
        if c == b' ' || c == b'\t' {
            start -= 1;
        } else {
            break;
        }
    }
    // Walk right over inner whitespace until we hit the surrounding pipe.
    while end < bytes.len() {
        let c = bytes[end];
        if c == b' ' || c == b'\t' {
            end += 1;
        } else {
            break;
        }
    }
    let raw = &buf[start..end];
    let leading_ws = raw
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count() as u8;
    let trailing_ws = raw
        .bytes()
        .rev()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count() as u8;
    Cell {
        source_range: start..end,
        content: raw.trim().to_string(),
        leading_ws,
        trailing_ws,
    }
}

/// Detect the formatting style of a parsed table source. Used by the
/// `Reformat Table` command and by smart-paste to pick a target style;
/// `parse_tables` itself always defaults parsed tables to
/// [`TableStyle::PreserveOriginal`].
pub fn detect_style(table_src: &str) -> TableStyle {
    let body_lines: Vec<&str> = table_src
        .lines()
        .filter(|l| !is_separator_line(l) && !l.trim().is_empty())
        .collect();

    let has_wide_pad = body_lines
        .iter()
        .any(|l| l.contains("  |") || l.contains("|  "));
    let has_uniform_compact = body_lines
        .iter()
        .all(|l| !l.contains("  |") && !l.contains("|  "));

    match (has_wide_pad, has_uniform_compact) {
        (true, _) => TableStyle::Pretty,
        (false, true) => TableStyle::Compact,
        _ => TableStyle::PreserveOriginal,
    }
}

/// `true` if `s` is a GFM separator line — pipes, dashes, colons,
/// whitespace only, with at least one dash.
pub fn is_separator_line(s: &str) -> bool {
    let trimmed = s.trim();
    !trimmed.is_empty()
        && trimmed
            .bytes()
            .all(|c| matches!(c, b'|' | b'-' | b':' | b' '))
        && trimmed.contains('-')
}

/// Capture verbatim source lines for the PreserveOriginal round-trip
/// strategy. Empty lines inside the table block are skipped.
pub fn capture_original_lines(table_src: &str) -> Vec<OriginalLine> {
    let mut row_index = 0usize;
    let mut seen_separator = false;
    let mut out = Vec::new();

    for line in table_src.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let kind = if is_separator_line(line) {
            seen_separator = true;
            LineKind::Separator
        } else if !seen_separator {
            LineKind::Header
        } else {
            let k = LineKind::Body { row_index };
            row_index += 1;
            k
        };
        out.push(OriginalLine {
            raw: line.to_string(),
            cell_spans: split_cells(line),
            kind,
        });
    }
    out
}

/// Split a table line by unescaped pipes. Returns ranges of cell
/// content (between pipes). Handles `\|` escapes correctly.
///
/// Strips the leading empty span when the line starts with `|`, and
/// the trailing empty span when it ends with `|` — those are the
/// outer wrapping pipes, not real cells.
pub fn split_cells(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut start: usize = 0;
    let mut prev_was_backslash = false;
    let mut started = false;

    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\\' if !prev_was_backslash => {
                prev_was_backslash = true;
                continue;
            }
            b'|' if !prev_was_backslash => {
                if started {
                    out.push(start..i);
                }
                start = i + 1;
                started = true;
            }
            _ => {}
        }
        prev_was_backslash = false;
    }
    if started && start < bytes.len() {
        out.push(start..bytes.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_table() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let tables = parse_tables(src);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.headers.len(), 2);
        assert_eq!(t.headers[0].content, "a");
        assert_eq!(t.headers[1].content, "b");
        assert_eq!(t.rows.len(), 1);
        assert_eq!(t.rows[0][0].content, "1");
        assert_eq!(t.rows[0][1].content, "2");
    }

    #[test]
    fn parses_alignment_separator() {
        let src = "| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n";
        let tables = parse_tables(src);
        assert_eq!(
            tables[0].alignments,
            vec![Alignment::Left, Alignment::Center, Alignment::Right]
        );
    }

    #[test]
    fn parsed_table_defaults_to_preserve_original() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let tables = parse_tables(src);
        assert_eq!(tables[0].style, TableStyle::PreserveOriginal);
        assert!(tables[0].original_lines.is_some());
        // header + separator + 1 body row
        assert_eq!(tables[0].original_lines.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn cell_source_range_includes_padding() {
        let src = "|  name  | score |\n|--------|-------|\n|  Alice |  42   |\n";
        let tables = parse_tables(src);
        let header = &tables[0].headers[0];
        // " name " with two leading spaces, two trailing spaces.
        assert_eq!(&src[header.source_range.clone()], "  name  ");
        assert_eq!(header.leading_ws, 2);
        assert_eq!(header.trailing_ws, 2);
        assert_eq!(header.content, "name");
    }

    #[test]
    fn parses_multiple_tables_with_separate_ids() {
        let src = "\
| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |\n";
        let tables = parse_tables(src);
        assert_eq!(tables.len(), 2);
        assert_ne!(tables[0].id, tables[1].id);
        assert!(tables[0].source_range.end <= tables[1].source_range.start);
    }

    #[test]
    fn split_cells_handles_escaped_pipes() {
        let line = "| foo \\| bar | baz |";
        let cells = split_cells(line);
        assert_eq!(cells.len(), 2);
        assert_eq!(&line[cells[0].clone()], " foo \\| bar ");
        assert_eq!(&line[cells[1].clone()], " baz ");
    }

    #[test]
    fn is_separator_line_recognises_alignment_colons() {
        assert!(is_separator_line("|---|---|"));
        assert!(is_separator_line("|:---|---:|"));
        assert!(is_separator_line("| :---: | :---: |"));
        assert!(!is_separator_line("| a | b |"));
        assert!(!is_separator_line("|||"));
    }

    #[test]
    fn detects_pretty_compact_preserve() {
        let compact = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let pretty = "| name  | score |\n|-------|------:|\n| Alice |    42 |\n";
        assert_eq!(detect_style(compact), TableStyle::Compact);
        assert_eq!(detect_style(pretty), TableStyle::Pretty);
    }

    #[test]
    fn cell_content_inline_markdown_preserved() {
        let src = "| **bold** | [link](x) |\n|---|---|\n| `code` | _em_ |\n";
        let t = &parse_tables(src)[0];
        assert_eq!(t.headers[0].content, "**bold**");
        assert_eq!(t.headers[1].content, "[link](x)");
        assert_eq!(t.rows[0][0].content, "`code`");
        assert_eq!(t.rows[0][1].content, "_em_");
    }

    #[test]
    fn empty_cells_round_trip() {
        let src = "| a |  |\n|---|---|\n|   | b |\n";
        let t = &parse_tables(src)[0];
        assert_eq!(t.headers[0].content, "a");
        assert_eq!(t.headers[1].content, "");
        assert_eq!(t.rows[0][0].content, "");
        assert_eq!(t.rows[0][1].content, "b");
    }
}
