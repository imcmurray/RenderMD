//! Serialize `MarkdownTable` back to GFM source and apply cell edits.
//!
//! Three strategies share this module:
//!
//! - [`to_gfm_preserve`] — replays the captured `original_lines`,
//!   substituting only cells whose content changed. The result is
//!   byte-identical to the input for unedited cells.
//! - [`to_gfm_pretty`] — recomputes column widths and produces an
//!   aligned, professional-looking output.
//! - [`to_gfm_compact`] — single-space padding only; minimal output.
//!
//! [`update_cell`] is the load-bearing entry point for click-to-edit:
//! it dispatches to in-place patching (PreserveOriginal) or
//! whole-table re-serialization (Pretty / Compact) based on the
//! table's [`TableStyle`].

use std::ops::Range;

use super::model::*;
use super::parse::{capture_original_lines, split_cells};

/// Serialise a table to GFM source per its style.
pub fn to_gfm(table: &MarkdownTable, buffer: &str) -> String {
    match table.style {
        TableStyle::Compact => to_gfm_compact(table),
        TableStyle::Pretty => to_gfm_pretty(table),
        TableStyle::PreserveOriginal => to_gfm_preserve(table, buffer),
    }
}

/// Single-space padding. Always reformats.
pub fn to_gfm_compact(table: &MarkdownTable) -> String {
    let mut s = String::new();
    push_row(&mut s, &table.headers, |c| {
        format!(" {} ", escape_cell(&c.content))
    });
    push_separator(&mut s, &table.alignments, |a| match a {
        Alignment::None => "---".to_string(),
        Alignment::Left => ":---".to_string(),
        Alignment::Center => ":---:".to_string(),
        Alignment::Right => "---:".to_string(),
    });
    for row in &table.rows {
        push_row(&mut s, row, |c| format!(" {} ", escape_cell(&c.content)));
    }
    s
}

/// Pretty-printed with column widths recomputed from current cell
/// contents. The default for *newly created* tables.
pub fn to_gfm_pretty(table: &MarkdownTable) -> String {
    let widths = column_widths(table);
    let mut s = String::new();

    s.push('|');
    for (col, cell) in table.headers.iter().enumerate() {
        s.push_str(&pad_for(
            table.alignments[col],
            &escape_cell(&cell.content),
            widths[col],
        ));
        s.push('|');
    }
    s.push('\n');

    // Separator. Width matches the column (minus 2 for the leading/
    // trailing space) so the table outline is rectangular.
    s.push('|');
    for (col, align) in table.alignments.iter().enumerate() {
        let dashes_only = widths[col];
        let inner = match align {
            Alignment::None => "-".repeat(dashes_only),
            Alignment::Left => {
                if dashes_only < 1 {
                    "-".to_string()
                } else {
                    format!(":{}", "-".repeat(dashes_only - 1))
                }
            }
            Alignment::Right => {
                if dashes_only < 1 {
                    "-".to_string()
                } else {
                    format!("{}:", "-".repeat(dashes_only - 1))
                }
            }
            Alignment::Center => {
                if dashes_only < 2 {
                    ":-:".to_string()
                } else {
                    format!(":{}:", "-".repeat(dashes_only - 2))
                }
            }
        };
        s.push(' ');
        s.push_str(&inner);
        s.push(' ');
        s.push('|');
    }
    s.push('\n');

    for row in &table.rows {
        s.push('|');
        for (col, cell) in row.iter().enumerate() {
            if col >= table.alignments.len() {
                break;
            }
            s.push_str(&pad_for(
                table.alignments[col],
                &escape_cell(&cell.content),
                widths[col],
            ));
            s.push('|');
        }
        // Pad short rows with empties so output stays rectangular.
        for (align, width) in table.alignments.iter().zip(widths.iter()).skip(row.len()) {
            s.push_str(&pad_for(*align, "", *width));
            s.push('|');
        }
        s.push('\n');
    }
    s
}

/// Replay the captured original lines, substituting only changed
/// cells. Cells whose content matches the original raw bytes are
/// emitted byte-for-byte unchanged.
///
/// For rows whose cell count no longer matches the original (e.g. a
/// column was added later), falls back to compact formatting for
/// just that row.
pub fn to_gfm_preserve(table: &MarkdownTable, _buffer: &str) -> String {
    let originals = match table.original_lines.as_ref() {
        Some(ls) => ls,
        // Should not happen for tables parsed via `parse_tables`, but
        // be defensive: fall back to pretty-printing.
        None => return to_gfm_pretty(table),
    };

    let mut out = String::new();
    for line in originals {
        let rebuilt = match line.kind {
            LineKind::Header => rebuild_line(line, &table.headers),
            // Separator is structural — we never edit it via cells.
            LineKind::Separator => line.raw.clone(),
            LineKind::Body { row_index } => {
                if let Some(row) = table.rows.get(row_index) {
                    rebuild_line(line, row)
                } else {
                    // Row was removed; skip.
                    continue;
                }
            }
        };
        out.push_str(&rebuilt);
        out.push('\n');
    }
    out
}

/// Insert an empty body row at `at_index`. Triggers a structural
/// re-serialize: PreserveOriginal tables get promoted to Pretty just
/// long enough to emit the new row (we don't have an `OriginalLine`
/// for it), and `original_lines` is re-captured from the freshly
/// written source so subsequent cell edits stay surgical.
pub fn insert_empty_row(
    table: &mut MarkdownTable,
    at_index: usize,
    buffer: &mut String,
) -> Result<EditDelta> {
    let cols = table.alignments.len();
    if cols == 0 {
        return Err(TableError::InvalidStructure(
            "table has no columns".to_string(),
        ));
    }
    if at_index > table.rows.len() {
        return Err(TableError::CellOutOfRange {
            row: at_index as i32,
            col: 0,
        });
    }

    let blank_row: Vec<Cell> = (0..cols)
        .map(|_| Cell {
            // Source ranges are filler — refresh_internal_ranges (called
            // below) will re-derive them from the new source.
            source_range: 0..0,
            content: String::new(),
            leading_ws: 1,
            trailing_ws: 1,
        })
        .collect();
    table.rows.insert(at_index, blank_row);

    // Pick a temporary style for the re-serialise. PreserveOriginal
    // can't carry a brand-new row through `to_gfm_preserve` (no
    // OriginalLine to splice), so swap to Pretty for the emit.
    let original_style = table.style;
    let emit_style = match original_style {
        TableStyle::PreserveOriginal => TableStyle::Pretty,
        other => other,
    };
    table.style = emit_style;
    let new_text = to_gfm(table, buffer);
    table.style = original_style;

    let old_range = table.source_range.clone();
    let old_len = old_range.end - old_range.start;
    let byte_delta = new_text.len() as isize - old_len as isize;

    buffer.replace_range(old_range.clone(), &new_text);
    table.source_range = old_range.start..(old_range.start + new_text.len());

    // Refresh per-cell source ranges from the new bytes.
    refresh_internal_ranges(table, buffer);

    // Re-capture original_lines from the new source so PreserveOriginal
    // works for *future* per-cell edits including in the new row.
    if matches!(original_style, TableStyle::PreserveOriginal) {
        let table_src = &buffer[table.source_range.clone()];
        table.original_lines = Some(super::parse::capture_original_lines(table_src));
    }

    Ok(EditDelta {
        byte_delta,
        patched_range: old_range,
        new_range: table.source_range.clone(),
    })
}

/// Edit a single cell. Dispatches on table style; returns enough
/// information for the caller to shift downstream tables.
pub fn update_cell(
    table: &mut MarkdownTable,
    row: i32,
    col: usize,
    new_content: &str,
    buffer: &mut String,
) -> Result<EditDelta> {
    // Validate cell first; we want to return a clean error before
    // touching the buffer.
    table.cell(row, col)?;

    match table.style {
        TableStyle::PreserveOriginal => update_cell_in_place(table, row, col, new_content, buffer),
        TableStyle::Pretty | TableStyle::Compact => {
            update_cell_via_reserialize(table, row, col, new_content, buffer)
        }
    }
}

/// PreserveOriginal: patch only the cell's content range. Preserves
/// the user's intra-cell whitespace and surrounding column padding
/// when the new content fits the existing width; widens the cell if
/// the new content is longer.
fn update_cell_in_place(
    table: &mut MarkdownTable,
    row: i32,
    col: usize,
    new_content: &str,
    buffer: &mut String,
) -> Result<EditDelta> {
    let escaped = escape_cell(new_content);

    // Pull what we need from the cell, then drop the borrow before we
    // call shift_after (which needs &mut table).
    let (old_range, padded, new_len, byte_delta) = {
        let cell = table.cell_mut(row, col)?;
        let old_range = cell.source_range.clone();
        let old_len = old_range.end - old_range.start;

        let inner_target = old_len
            .saturating_sub(cell.leading_ws as usize)
            .saturating_sub(cell.trailing_ws as usize)
            .max(visual_width(&escaped));
        let lead = " ".repeat(cell.leading_ws.max(1) as usize);
        let trail = " ".repeat(cell.trailing_ws.max(1) as usize);
        let padded = format!(
            "{lead}{escaped:<inner_target$}{trail}",
            lead = lead,
            escaped = escaped,
            trail = trail,
            inner_target = inner_target
        );
        let new_len = padded.len();
        let byte_delta = new_len as isize - old_len as isize;

        cell.content = new_content.to_string();
        cell.leading_ws = cell.leading_ws.max(1);
        cell.trailing_ws = cell.trailing_ws.max(1);
        // Note: we do NOT manually mutate `cell.source_range` here.
        // `shift_after` handles it below — its `>=` semantics on the
        // cell's end correctly grows the range from `old_range.end` to
        // `old_range.end + byte_delta`. Setting it manually AND letting
        // shift_after run would double-apply the delta.

        (old_range, padded, new_len, byte_delta)
    };

    buffer.replace_range(old_range.clone(), &padded);
    table.shift_after(old_range.end, byte_delta);

    let new_range = table.cell(row, col)?.source_range.clone();

    // Keep original_lines in sync so subsequent edits on this table
    // stay accurate.
    if let Some(lines) = &mut table.original_lines {
        refresh_original_line(lines, row, col, &padded);
    }

    let _ = new_len; // explicit: used to compute byte_delta above
    Ok(EditDelta {
        byte_delta,
        patched_range: old_range,
        new_range,
    })
}

/// Pretty / Compact: re-serialise the whole table block. Cleaner output
/// but touches every byte of the table.
fn update_cell_via_reserialize(
    table: &mut MarkdownTable,
    row: i32,
    col: usize,
    new_content: &str,
    buffer: &mut String,
) -> Result<EditDelta> {
    table.cell_mut(row, col)?.content = new_content.to_string();

    let new_text = match table.style {
        TableStyle::Pretty => to_gfm_pretty(table),
        TableStyle::Compact => to_gfm_compact(table),
        TableStyle::PreserveOriginal => unreachable!(),
    };

    let old_range = table.source_range.clone();
    let old_len = old_range.end - old_range.start;
    let byte_delta = new_text.len() as isize - old_len as isize;

    buffer.replace_range(old_range.clone(), &new_text);
    table.source_range = old_range.start..(old_range.start + new_text.len());

    // After full re-serialise, in-table source_range values are stale.
    // Re-derive them by re-scanning the freshly written text.
    refresh_internal_ranges(table, buffer);

    Ok(EditDelta {
        byte_delta,
        patched_range: old_range,
        new_range: table.source_range.clone(),
    })
}

// --- internals ----------------------------------------------------------

fn column_widths(table: &MarkdownTable) -> Vec<usize> {
    let n = table.alignments.len();
    let mut widths = vec![3usize; n]; // minimum 3 dashes for the separator
    for (col, cell) in table.headers.iter().enumerate() {
        if col < n {
            widths[col] = widths[col].max(visual_width(&escape_cell(&cell.content)));
        }
    }
    for row in &table.rows {
        for (col, cell) in row.iter().enumerate() {
            if col < n {
                widths[col] = widths[col].max(visual_width(&escape_cell(&cell.content)));
            }
        }
    }
    widths
}

/// Best-effort visual width. Without a unicode-width dep we use
/// chars().count() which is correct for ASCII / Latin / CJK at the
/// 1-cell granularity that table column alignment uses anyway.
fn visual_width(s: &str) -> usize {
    s.chars().count()
}

fn pad_for(align: Alignment, content: &str, width: usize) -> String {
    let text_w = visual_width(content);
    if text_w >= width {
        return format!(" {content} ");
    }
    let extra = width - text_w;
    match align {
        Alignment::Right => format!(" {pad}{content} ", pad = " ".repeat(extra)),
        Alignment::Center => {
            let left = extra / 2;
            let right = extra - left;
            format!(
                " {l}{content}{r} ",
                l = " ".repeat(left),
                r = " ".repeat(right)
            )
        }
        _ => format!(" {content}{pad} ", pad = " ".repeat(extra)),
    }
}

fn push_row<F>(out: &mut String, cells: &[Cell], mut fmt: F)
where
    F: FnMut(&Cell) -> String,
{
    out.push('|');
    for cell in cells {
        out.push_str(&fmt(cell));
        out.push('|');
    }
    out.push('\n');
}

fn push_separator<F>(out: &mut String, aligns: &[Alignment], mut fmt: F)
where
    F: FnMut(&Alignment) -> String,
{
    out.push('|');
    for a in aligns {
        out.push(' ');
        out.push_str(&fmt(a));
        out.push(' ');
        out.push('|');
    }
    out.push('\n');
}

/// Escape cell-hostile characters so GFM round-trips correctly.
/// Backslash must be escaped first to avoid double-escaping.
pub fn escape_cell(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "<br>")
}

/// Rebuild a single line from the original with current cell contents
/// substituted into the cell spans. Falls back to compact formatting
/// if the cell count no longer matches the original line.
fn rebuild_line(line: &OriginalLine, cells: &[Cell]) -> String {
    if line.cell_spans.len() != cells.len() {
        // Structural mismatch — emit a compact line for this row only.
        return compact_line(cells);
    }
    let mut rebuilt = line.raw.clone();
    // Walk right-to-left so earlier offsets stay valid.
    for (col, span) in line.cell_spans.iter().enumerate().rev() {
        let original_text = &line.raw[span.clone()];
        let lead = original_text
            .bytes()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        let trail = original_text
            .bytes()
            .rev()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        let escaped = escape_cell(&cells[col].content);

        let inner_target = original_text
            .trim()
            .chars()
            .count()
            .max(visual_width(&escaped));

        let padded = format!(
            "{lead}{escaped:<inner_target$}{trail}",
            lead = " ".repeat(lead.max(1)),
            escaped = escaped,
            trail = " ".repeat(trail.max(1)),
            inner_target = inner_target
        );
        rebuilt.replace_range(span.clone(), &padded);
    }
    rebuilt
}

fn compact_line(cells: &[Cell]) -> String {
    let mut s = String::from("|");
    for c in cells {
        s.push(' ');
        s.push_str(&escape_cell(&c.content));
        s.push(' ');
        s.push('|');
    }
    s
}

fn refresh_original_line(lines: &mut [OriginalLine], row: i32, col: usize, padded: &str) {
    let target_index = match row {
        -1 => lines.iter().position(|l| l.kind == LineKind::Header),
        r => lines
            .iter()
            .position(|l| matches!(l.kind, LineKind::Body { row_index } if row_index as i32 == r)),
    };
    let Some(idx) = target_index else {
        return;
    };
    let line = &mut lines[idx];
    if col >= line.cell_spans.len() {
        return;
    }

    let span = line.cell_spans[col].clone();
    let old_len = span.end - span.start;
    line.raw.replace_range(span.clone(), padded);

    let new_len = padded.len();
    let delta = new_len as isize - old_len as isize;
    // Shift spans of cells to the right of this one within the line.
    for sp in line.cell_spans.iter_mut().skip(col + 1) {
        sp.start = (sp.start as isize + delta) as usize;
        sp.end = (sp.end as isize + delta) as usize;
    }
    line.cell_spans[col] = span.start..(span.start + new_len);
}

/// After a full re-serialise, re-scan the table block to refresh each
/// cell's `source_range`. Cheap (a single linear pass).
fn refresh_internal_ranges(table: &mut MarkdownTable, buffer: &str) {
    let table_src = &buffer[table.source_range.clone()];
    let mut row_cursor: usize = 0;
    let mut byte_cursor = table.source_range.start;
    let mut seen_separator = false;

    for line in table_src.lines() {
        let line_byte_start = byte_cursor;
        byte_cursor += line.len() + 1; // +1 for the newline

        if line.trim().is_empty() {
            continue;
        }
        if super::parse::is_separator_line(line) {
            seen_separator = true;
            continue;
        }
        let cells = split_cells(line);
        let target = if !seen_separator {
            Some(&mut table.headers)
        } else {
            let r = table.rows.get_mut(row_cursor);
            row_cursor += 1;
            r
        };
        if let Some(target) = target {
            for (col, span) in cells.iter().enumerate() {
                if col >= target.len() {
                    break;
                }
                target[col].source_range =
                    (line_byte_start + span.start)..(line_byte_start + span.end);
            }
        }
    }

    // Pretty/Compact tables retain Pretty/Compact style; they don't keep
    // original_lines. Drop any stale capture.
    if matches!(table.style, TableStyle::Pretty | TableStyle::Compact) {
        table.original_lines = None;
    } else if let Some(lines) = &mut table.original_lines {
        *lines = capture_original_lines(table_src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::parse::parse_tables;

    fn round_trip_preserve(src: &str) -> String {
        let tables = parse_tables(src);
        let t = &tables[0];
        to_gfm_preserve(t, src)
    }

    #[test]
    fn preserve_round_trips_compact_unchanged() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = round_trip_preserve(src);
        // Trailing newline differences are acceptable; compare trimmed.
        assert_eq!(out.trim_end(), src.trim_end());
    }

    #[test]
    fn preserve_round_trips_padded_unchanged() {
        let src = "| name  | score |\n|-------|------:|\n| Alice |    42 |\n";
        let out = round_trip_preserve(src);
        assert_eq!(out.trim_end(), src.trim_end());
    }

    #[test]
    fn pretty_emits_aligned_columns() {
        let mut tables = parse_tables("| a | name |\n|---|------|\n| x | longer |\n");
        let t = &mut tables[0];
        t.style = TableStyle::Pretty;
        let out = to_gfm_pretty(t);
        // Header and body row should agree on column widths.
        let lines: Vec<&str> = out.lines().collect();
        let header_pipes: Vec<usize> = lines[0].match_indices('|').map(|(i, _)| i).collect();
        let body_pipes: Vec<usize> = lines[2].match_indices('|').map(|(i, _)| i).collect();
        assert_eq!(header_pipes, body_pipes);
    }

    #[test]
    fn compact_uses_single_space_padding() {
        let mut tables = parse_tables("| name | score |\n|------|-------|\n| Alice | 42 |\n");
        let t = &mut tables[0];
        t.style = TableStyle::Compact;
        let out = to_gfm_compact(t);
        assert!(out.starts_with("| name |"));
        assert!(out.contains("| Alice |"));
    }

    #[test]
    fn update_cell_preserve_keeps_unchanged_bytes_byte_identical() {
        let original =
            "| name  | score |\n|-------|------:|\n| Alice |    42 |\n| Bob   |    99 |\n";
        let mut buffer = String::from(original);
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];

        // Edit body row 0 col 0: Alice → Alex (4 chars, fits existing width).
        let delta = t.update_cell(0, 0, "Alex", &mut buffer).unwrap();
        assert_eq!(delta.byte_delta, 0); // no width change

        // Header and unrelated body row should be byte-identical to original.
        assert!(buffer.contains("| name  | score |"));
        assert!(buffer.contains("| Bob   |    99 |"));
        // Edited cell shows "Alex" with preserved padding.
        assert!(buffer.contains("| Alex  |"));
    }

    #[test]
    fn update_cell_preserve_grows_width_when_content_longer() {
        let original = "| name  |\n|-------|\n| Alice |\n";
        let mut buffer = String::from(original);
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];

        let delta = t.update_cell(0, 0, "Alexandra", &mut buffer).unwrap();
        assert!(delta.byte_delta > 0);
        assert!(buffer.contains("Alexandra"));
        // Cell source range must reflect the new length.
        let cell = t.cell(0, 0).unwrap();
        assert_eq!(&buffer[cell.source_range.clone()].trim(), &"Alexandra");
    }

    #[test]
    fn update_cell_pretty_re_aligns_columns() {
        let mut buffer = String::from("| a | name |\n|---|------|\n| x | y |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.style = TableStyle::Pretty;
        t.original_lines = None;

        t.update_cell(0, 1, "longer", &mut buffer).unwrap();
        let lines: Vec<&str> = buffer.lines().collect();
        assert!(lines[2].contains("longer"));
        // Header line should be at least as long as body line.
        assert!(lines[0].len() >= lines[2].len() - 1);
    }

    #[test]
    fn update_cell_escapes_pipes() {
        let mut buffer = String::from("| a |\n|---|\n| x |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];

        t.update_cell(0, 0, "foo | bar", &mut buffer).unwrap();
        assert!(buffer.contains("foo \\| bar"));
    }

    #[test]
    fn update_cell_handles_multiline_via_br() {
        let mut buffer = String::from("| a |\n|---|\n| x |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];

        t.update_cell(0, 0, "line1\nline2", &mut buffer).unwrap();
        assert!(buffer.contains("line1<br>line2"));
    }

    #[test]
    fn update_cell_out_of_range_errors_without_buffer_change() {
        let mut buffer = String::from("| a |\n|---|\n| x |\n");
        let before = buffer.clone();
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];

        let err = t.update_cell(99, 0, "x", &mut buffer).unwrap_err();
        assert!(matches!(err, TableError::CellOutOfRange { .. }));
        assert_eq!(buffer, before);
    }

    #[test]
    fn shift_after_runs_on_in_place_edit() {
        let mut buffer = String::from("| a |\n|---|\n| short |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let original_end = t.source_range.end;

        t.update_cell(0, 0, "much longer text", &mut buffer)
            .unwrap();
        assert!(t.source_range.end > original_end);
    }

    #[test]
    fn escape_cell_idempotent_for_safe_content() {
        assert_eq!(escape_cell("foo bar"), "foo bar");
        assert_eq!(escape_cell("**bold**"), "**bold**");
    }

    #[test]
    fn escape_cell_handles_backslash_before_pipe() {
        // \ -> \\ first, then | -> \| → original literal backslash stays
        // distinguishable from an escape sequence.
        assert_eq!(escape_cell("a\\|b"), "a\\\\\\|b");
    }

    #[test]
    fn insert_empty_row_appends_at_end() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let before_rows = t.rows.len();
        let delta = t.insert_empty_row(before_rows, &mut buffer).unwrap();
        assert_eq!(t.rows.len(), before_rows + 1);
        assert!(delta.byte_delta > 0);
        // Buffer now has a new blank row line.
        assert!(buffer.matches('\n').count() >= 4);
    }

    #[test]
    fn insert_empty_row_at_middle_position() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n| 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.insert_empty_row(1, &mut buffer).unwrap();
        assert_eq!(t.rows.len(), 3);
        // The inserted row is at index 1 → empty.
        assert!(t.rows[1].iter().all(|c| c.content.is_empty()));
        // Surrounding rows are intact.
        assert_eq!(t.rows[0][0].content, "1");
        assert_eq!(t.rows[2][0].content, "2");
    }

    #[test]
    fn insert_empty_row_in_preserve_table_recaptures_original_lines() {
        let original = "| name  | score |\n|-------|------:|\n| Alice |    42 |\n";
        let mut buffer = String::from(original);
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        assert_eq!(t.style, TableStyle::PreserveOriginal);

        t.insert_empty_row(1, &mut buffer).unwrap();
        assert_eq!(t.rows.len(), 2);
        // Style stays PreserveOriginal even though we re-serialised
        // through Pretty for the emit.
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        // original_lines re-captured against the new source.
        assert!(t.original_lines.is_some());
        // Subsequent cell edit should still work in PreserveOriginal mode.
        t.update_cell(1, 0, "Bob", &mut buffer).unwrap();
        assert!(buffer.contains("Bob"));
    }

    #[test]
    fn insert_empty_row_out_of_range_errors() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let err = t.insert_empty_row(99, &mut buffer).unwrap_err();
        assert!(matches!(err, TableError::CellOutOfRange { .. }));
    }
}
