//! Parse GFM tables from a buffer using pulldown-cmark.
//!
//! Built around [`Parser::into_offset_iter`] so we get byte ranges for
//! every event — those ranges become each cell's `source_range`, which
//! is what makes minimum-diff cell patching possible.

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

    // Second pass: bind any preceding `<!-- rmd-cols: ... -->` width
    // comments to their table. The comment is the persistence layer
    // for the column-resize feature; binding it here means the
    // resulting `MarkdownTable.column_widths` arrives populated for
    // both the post-processor (which paints widths into HTML) and
    // the serializer (which re-emits the comment on save).
    for table in &mut tables {
        if let Some((widths, comment_start)) =
            extract_column_widths_comment(buffer, table.source_range.start)
        {
            if widths.len() == table.alignments.len() {
                table.column_widths = widths;
            }
            // Whether or not the width count matched, fold the
            // comment into source_range so the next re-serialization
            // overwrites it cleanly (avoids a stale comment marooned
            // above the table).
            table.source_range.start = comment_start;
        }
    }
    tables
}

/// Detect an `<!-- rmd-cols: ... -->` width-persistence comment
/// occupying the single line immediately above `table_start`. Returns
/// the parsed widths and the byte offset of the start of the comment
/// line so the caller can extend `source_range` to cover both.
///
/// `<!-- rmd-cols: 180,,120 -->` parses as `[Some(180), None, Some(120)]`
/// — the empty between commas means "no explicit width for that
/// column".
pub fn extract_column_widths_comment(
    buffer: &str,
    table_start: usize,
) -> Option<(Vec<Option<u32>>, usize)> {
    if table_start == 0 {
        return None;
    }
    let before = &buffer[..table_start];
    // The byte immediately preceding the table must be a newline —
    // we only bind a comment that occupies its own line.
    if !before.ends_with('\n') {
        return None;
    }
    let line_end = table_start - 1; // exclusive position of the \n
    let bytes_before_nl = &before[..line_end];
    let comment_start = bytes_before_nl.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let comment_line = &before[comment_start..line_end];

    let trimmed = comment_line.trim();
    let inside = trimmed.strip_prefix("<!-- rmd-cols:")?;
    let inside = inside.strip_suffix("-->")?;
    let inside = inside.trim();

    let widths: Vec<Option<u32>> = inside
        .split(',')
        .map(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                s.parse::<u32>().ok()
            }
        })
        .collect();

    Some((widths, comment_start))
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
            Event::End(TagEnd::TableRow) if !in_head => {
                rows.push(std::mem::take(&mut current_row));
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
        original_lines,
        sort_indicator: None,
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

/// `true` if `s` is the column-widths persistence comment that
/// belongs above the next table. Used by both the parser (which
/// pulls the line into `source_range`) and the serializer (which
/// re-emits it through a dedicated codepath rather than treating it
/// as a table row).
pub fn is_rmd_cols_comment(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("<!-- rmd-cols:") && t.ends_with("-->")
}

/// Capture verbatim source lines for the PreserveOriginal round-trip
/// strategy. Empty lines and the rmd-cols width-persistence comment
/// inside the table block are skipped — the comment is re-emitted
/// through `to_gfm` directly, so we don't want it leaking into a
/// rebuilt header row.
pub fn capture_original_lines(table_src: &str) -> Vec<OriginalLine> {
    let mut row_index = 0usize;
    let mut seen_separator = false;
    let mut out = Vec::new();

    for line in table_src.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if is_rmd_cols_comment(line) {
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

    #[test]
    fn parses_column_widths_comment() {
        let src = "<!-- rmd-cols: 180,,120 -->\n| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n";
        let t = &parse_tables(src)[0];
        assert_eq!(t.column_widths, vec![Some(180), None, Some(120)]);
        // source_range starts at the beginning of the comment so a
        // re-serialize replaces both.
        assert_eq!(t.source_range.start, 0);
        assert!(src[t.source_range.clone()].starts_with("<!-- rmd-cols:"));
    }

    #[test]
    fn column_widths_comment_ignored_when_count_mismatches() {
        // Two-column table but the comment lists three widths — the
        // values shouldn't be applied (defaults stay all-None), but
        // the comment IS pulled into source_range so a future
        // re-serialize cleans it up.
        let src = "<!-- rmd-cols: 100,200,300 -->\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let t = &parse_tables(src)[0];
        assert_eq!(t.column_widths, vec![None, None]);
        assert_eq!(t.source_range.start, 0);
    }

    #[test]
    fn column_widths_comment_missing_leaves_defaults() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let t = &parse_tables(src)[0];
        assert_eq!(t.column_widths, vec![None, None]);
        // No comment → source_range starts where the table starts.
        assert!(src[..t.source_range.start].is_empty());
    }

    #[test]
    fn column_widths_comment_with_surrounding_text_not_bound() {
        // Comment on the same line as other text doesn't bind to the
        // table — we require it to occupy its own line.
        let src = "foo <!-- rmd-cols: 10,20 -->\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let t = &parse_tables(src)[0];
        assert_eq!(t.column_widths, vec![None, None]);
    }

    #[test]
    fn extract_column_widths_handles_extra_whitespace() {
        // Tolerate looser spacing inside the comment.
        let src = "<!-- rmd-cols:  100, , 200  -->\n| a | b | c |\n|---|---|---|\n";
        let (widths, start) = extract_column_widths_comment(src, src.find("| a").unwrap()).unwrap();
        assert_eq!(widths, vec![Some(100), None, Some(200)]);
        assert_eq!(start, 0);
    }
}
