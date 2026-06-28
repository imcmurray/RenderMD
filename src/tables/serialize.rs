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

use super::model::*;
use super::parse::{capture_original_lines, split_cells};

/// Serialise a table to GFM source per its style.
///
/// If any column has a persisted width, an `<!-- rmd-cols: ... -->`
/// comment is prepended on its own line. This is the persistence
/// side of the column-resize feature; viewers that don't recognise
/// the comment treat it as a regular HTML comment and ignore it.
pub fn to_gfm(table: &MarkdownTable, buffer: &str) -> String {
    let body = match table.style {
        TableStyle::Compact => to_gfm_compact(table),
        TableStyle::Pretty => to_gfm_pretty(table),
        TableStyle::PreserveOriginal => to_gfm_preserve(table, buffer),
    };
    match build_column_widths_comment(&table.column_widths) {
        Some(comment) => format!("{comment}\n{body}"),
        None => body,
    }
}

/// Build the persistence comment for `column_widths` if any column
/// has an explicit width. `Some(180)` → `"180"`, `None` → empty. The
/// format mirrors what [`crate::tables::parse::extract_column_widths_comment`]
/// expects so a round-trip is lossless.
fn build_column_widths_comment(widths: &[Option<u32>]) -> Option<String> {
    if widths.iter().all(|w| w.is_none()) {
        return None;
    }
    let parts: Vec<String> = widths
        .iter()
        .map(|w| match w {
            Some(n) => n.to_string(),
            None => String::new(),
        })
        .collect();
    Some(format!("<!-- rmd-cols: {} -->", parts.join(",")))
}

/// Replace the table's persisted column widths and trigger a
/// structural re-serialize. Pass `widths.len() == table.alignments.len()`
/// or you get back an `InvalidStructure` error.
///
/// Passing all-`None` widths drops the persistence comment from the
/// next emit — that's how the "clear all widths" command is built.
pub fn set_column_widths(
    table: &mut MarkdownTable,
    widths: Vec<Option<u32>>,
    buffer: &mut String,
) -> Result<EditDelta> {
    if widths.len() != table.alignments.len() {
        return Err(TableError::InvalidStructure(format!(
            "column_widths length {} doesn't match table columns {}",
            widths.len(),
            table.alignments.len()
        )));
    }
    table.column_widths = widths;
    re_serialize_structurally(table, buffer)
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
    let blank_row: Vec<Cell> = (0..cols).map(|_| blank_cell()).collect();
    table.rows.insert(at_index, blank_row);
    re_serialize_structurally(table, buffer)
}

/// Remove the body row at `row_index`. Leaves the header intact; an
/// empty `rows` is allowed (header-only table). Returns the patch
/// info so the caller can shift downstream tables.
pub fn delete_row(
    table: &mut MarkdownTable,
    row_index: usize,
    buffer: &mut String,
) -> Result<EditDelta> {
    if row_index >= table.rows.len() {
        return Err(TableError::CellOutOfRange {
            row: row_index as i32,
            col: 0,
        });
    }
    table.rows.remove(row_index);
    re_serialize_structurally(table, buffer)
}

/// Insert an empty column at `at_index`. Affects header, alignments,
/// column_widths, and every body row (short rows are padded with
/// blanks as needed before the insert).
pub fn insert_column(
    table: &mut MarkdownTable,
    at_index: usize,
    alignment: Alignment,
    buffer: &mut String,
) -> Result<EditDelta> {
    let cols = table.alignments.len();
    if at_index > cols {
        return Err(TableError::CellOutOfRange {
            row: -1,
            col: at_index,
        });
    }
    table.alignments.insert(at_index, alignment);
    table.headers.insert(at_index, blank_cell());
    for row in &mut table.rows {
        // Pad row up to at_index if it's shorter than the new
        // table width, then splice in the empty cell.
        while row.len() < at_index {
            row.push(blank_cell());
        }
        row.insert(at_index, blank_cell());
    }
    table.column_widths.insert(at_index, None);
    re_serialize_structurally(table, buffer)
}

/// Replace the table's body rows wholesale and trigger a structural
/// re-serialize. Used by the sort flow to commit a freshly reordered
/// rows vector; same PreserveOriginal-aware handling as the other
/// structural ops.
pub fn replace_rows(
    table: &mut MarkdownTable,
    new_rows: Vec<Vec<Cell>>,
    buffer: &mut String,
) -> Result<EditDelta> {
    table.rows = new_rows;
    re_serialize_structurally(table, buffer)
}

/// Force-reformat the table to pretty-aligned columns, recomputing
/// widths from the current cell contents. Works on tables of any
/// style — PreserveOriginal, Pretty, or Compact — and **leaves the
/// table in PreserveOriginal mode against the newly-pretty source**
/// so subsequent per-cell edits stay surgical.
///
/// The two-step "emit-as-Pretty, then track-as-PreserveOriginal"
/// pattern is intentional: it gives the user the visual benefit of
/// Pretty formatting (column-width alignment, normalised separator
/// colons) for this one operation, while keeping editing cheap for
/// the long tail of small edits that follow.
///
/// Idempotent: tables that already emit byte-identical Pretty
/// output return an `EditDelta` whose patched range covers the full
/// table but whose `byte_delta` is 0; the caller can choose to apply
/// it (harmless) or short-circuit on zero-byte-delta if it wants to
/// avoid the buffer event.
pub fn reformat_pretty(table: &mut MarkdownTable, buffer: &mut String) -> Result<EditDelta> {
    if table.alignments.is_empty() {
        return Err(TableError::InvalidStructure(
            "table has no columns".to_string(),
        ));
    }

    // Force the emit through to_gfm_pretty regardless of prior style.
    let new_text = to_gfm_pretty(table);

    let old_range = table.source_range.clone();
    let old_len = old_range.end - old_range.start;
    let byte_delta = new_text.len() as isize - old_len as isize;

    buffer.replace_range(old_range.clone(), &new_text);
    table.source_range = old_range.start..(old_range.start + new_text.len());

    refresh_internal_ranges(table, buffer);

    // Track future edits as PreserveOriginal against the new bytes.
    table.style = TableStyle::PreserveOriginal;
    let table_src = &buffer[table.source_range.clone()];
    table.original_lines = Some(super::parse::capture_original_lines(table_src));

    Ok(EditDelta {
        byte_delta,
        patched_range: old_range,
        new_range: table.source_range.clone(),
    })
}

/// Set the column's alignment, rewriting the separator row so the
/// new colons land in the right place. A no-op (same alignment as
/// the current value) returns an `EditDelta` with an empty
/// `patched_range` so the caller can short-circuit without
/// touching the buffer.
pub fn set_column_alignment(
    table: &mut MarkdownTable,
    col: usize,
    alignment: Alignment,
    buffer: &mut String,
) -> Result<EditDelta> {
    if col >= table.alignments.len() {
        return Err(TableError::CellOutOfRange { row: -1, col });
    }
    if table.alignments[col] == alignment {
        // No-op: caller will see is_empty()==true on patched_range
        // and skip the buffer patch and refresh.
        let pin = table.source_range.start..table.source_range.start;
        return Ok(EditDelta {
            byte_delta: 0,
            patched_range: pin.clone(),
            new_range: pin,
        });
    }
    table.alignments[col] = alignment;
    re_serialize_structurally(table, buffer)
}

/// Remove the column at `col_index` from header, alignments,
/// column_widths, and every body row that has a cell at that index.
/// Refuses to remove the last column (would leave an invalid table).
pub fn delete_column(
    table: &mut MarkdownTable,
    col_index: usize,
    buffer: &mut String,
) -> Result<EditDelta> {
    let cols = table.alignments.len();
    if cols <= 1 {
        return Err(TableError::InvalidStructure(
            "can't delete the only column".to_string(),
        ));
    }
    if col_index >= cols {
        return Err(TableError::CellOutOfRange {
            row: -1,
            col: col_index,
        });
    }
    table.alignments.remove(col_index);
    table.headers.remove(col_index);
    for row in &mut table.rows {
        if col_index < row.len() {
            row.remove(col_index);
        }
    }
    if col_index < table.column_widths.len() {
        table.column_widths.remove(col_index);
    }
    re_serialize_structurally(table, buffer)
}

fn blank_cell() -> Cell {
    Cell {
        // Source ranges are filler — refresh_internal_ranges (called
        // by re_serialize_structurally) will re-derive them from the
        // new source.
        source_range: 0..0,
        content: String::new(),
        leading_ws: 1,
        trailing_ws: 1,
    }
}

/// Shared finalizer for structural edits: re-emit the table block
/// (Pretty for emit even if the table is PreserveOriginal), splice
/// into the buffer, refresh internal source ranges, and re-capture
/// `original_lines` against the new source so future per-cell edits
/// in PreserveOriginal mode resume working surgically.
fn re_serialize_structurally(table: &mut MarkdownTable, buffer: &mut String) -> Result<EditDelta> {
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

    refresh_internal_ranges(table, buffer);

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
/// when the new content fits the existing width. When the new
/// content would *grow* the cell, falls through to a Pretty re-emit
/// so the rest of the column re-pads to match (otherwise the pipes
/// drift and the user sees a ragged table in the editor).
fn update_cell_in_place(
    table: &mut MarkdownTable,
    row: i32,
    col: usize,
    new_content: &str,
    buffer: &mut String,
) -> Result<EditDelta> {
    let escaped = escape_cell(new_content);

    // If the new content can't fit in this cell's current inner
    // width, promote to a Pretty re-emit. The column-padding-aware
    // emit keeps the whole table neat; sticking to in-place patching
    // would leave the edited cell wider than its column neighbours.
    {
        let cell = table.cell(row, col)?;
        let old_len = cell.source_range.end - cell.source_range.start;
        let inner_existing = old_len
            .saturating_sub(cell.leading_ws as usize)
            .saturating_sub(cell.trailing_ws as usize);
        if visual_width(&escaped) > inner_existing {
            return update_cell_via_pretty_promotion(table, row, col, new_content, buffer);
        }
    }

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

/// Cell edit on a PreserveOriginal table where the new content would
/// grow the cell beyond its existing inner width. Promotes to a
/// Pretty re-emit so the whole column re-pads, then re-captures
/// `original_lines` against the new source — leaving the table back
/// in PreserveOriginal mode for subsequent surgical edits.
fn update_cell_via_pretty_promotion(
    table: &mut MarkdownTable,
    row: i32,
    col: usize,
    new_content: &str,
    buffer: &mut String,
) -> Result<EditDelta> {
    table.cell_mut(row, col)?.content = new_content.to_string();

    // Force-emit through to_gfm_pretty regardless of prior style.
    let original_style = table.style;
    table.style = TableStyle::Pretty;
    let new_text = to_gfm(table, buffer);
    table.style = original_style;

    let old_range = table.source_range.clone();
    let old_len = old_range.end - old_range.start;
    let byte_delta = new_text.len() as isize - old_len as isize;

    buffer.replace_range(old_range.clone(), &new_text);
    table.source_range = old_range.start..(old_range.start + new_text.len());

    refresh_internal_ranges(table, buffer);

    // Keep PreserveOriginal mode alive against the freshly-pretty
    // source so the next narrow edit lands surgically again.
    if matches!(original_style, TableStyle::PreserveOriginal) {
        let table_src = &buffer[table.source_range.clone()];
        table.original_lines = Some(capture_original_lines(table_src));
    }

    Ok(EditDelta {
        byte_delta,
        patched_range: old_range,
        new_range: table.source_range.clone(),
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
        // The width-persistence comment lives inside source_range but
        // isn't structural — skip it so column source-ranges stay
        // bound to actual cells.
        if super::parse::is_rmd_cols_comment(line) {
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
    fn update_cell_preserve_grow_keeps_column_pipes_aligned() {
        // Before: pipes line up at fixed positions. After: a long
        // edit should re-pad neighbours so pipes still line up,
        // not leave the edited cell wider than its column siblings.
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.update_cell(0, 0, "much longer cell", &mut buffer)
            .unwrap();

        let lines: Vec<&str> = buffer.lines().collect();
        // All four lines should have pipes at the same column positions.
        let header_pipes: Vec<usize> = lines[0].match_indices('|').map(|(i, _)| i).collect();
        for line in &lines[1..] {
            let pipes: Vec<usize> = line.match_indices('|').map(|(i, _)| i).collect();
            assert_eq!(
                pipes, header_pipes,
                "pipe positions drifted on line: {line}"
            );
        }
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

    #[test]
    fn delete_row_removes_target() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n| 2 |\n| 3 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.delete_row(1, &mut buffer).unwrap();
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0][0].content, "1");
        assert_eq!(t.rows[1][0].content, "3");
    }

    #[test]
    fn delete_last_row_leaves_header_only_table() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.delete_row(0, &mut buffer).unwrap();
        assert_eq!(t.rows.len(), 0);
        assert!(buffer.contains("| a |") || buffer.contains("|a|") || buffer.contains("| a"));
    }

    #[test]
    fn delete_row_out_of_range_errors() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        assert!(matches!(
            t.delete_row(5, &mut buffer).unwrap_err(),
            TableError::CellOutOfRange { .. }
        ));
    }

    #[test]
    fn insert_column_at_end_adds_to_header_and_body() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.insert_column(2, Alignment::None, &mut buffer).unwrap();
        assert_eq!(t.alignments.len(), 3);
        assert_eq!(t.headers.len(), 3);
        assert_eq!(t.rows[0].len(), 3);
        assert!(t.headers[2].content.is_empty());
    }

    #[test]
    fn insert_column_at_start_shifts_existing_cells_right() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.insert_column(0, Alignment::None, &mut buffer).unwrap();
        assert_eq!(t.headers[0].content, "");
        assert_eq!(t.headers[1].content, "a");
        assert_eq!(t.rows[0][1].content, "1");
    }

    #[test]
    fn insert_column_with_alignment_preserved_in_separator() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.insert_column(1, Alignment::Right, &mut buffer).unwrap();
        // Right alignment renders the separator with a trailing colon.
        assert!(buffer.contains("-:"), "buffer: {buffer}");
    }

    #[test]
    fn delete_column_removes_from_header_and_body() {
        let mut buffer = String::from("| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.delete_column(1, &mut buffer).unwrap();
        assert_eq!(t.headers.len(), 2);
        assert_eq!(t.headers[0].content, "a");
        assert_eq!(t.headers[1].content, "c");
        assert_eq!(t.rows[0].len(), 2);
        assert_eq!(t.rows[0][1].content, "3");
    }

    #[test]
    fn delete_only_column_refused() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let err = t.delete_column(0, &mut buffer).unwrap_err();
        assert!(matches!(err, TableError::InvalidStructure(_)));
        // Table unchanged.
        assert_eq!(t.headers.len(), 1);
    }

    #[test]
    fn delete_column_out_of_range_errors() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let err = t.delete_column(99, &mut buffer).unwrap_err();
        assert!(matches!(err, TableError::CellOutOfRange { .. }));
    }

    #[test]
    fn structural_op_preserves_preserve_original_style() {
        let mut buffer = String::from("| name  | score |\n|-------|------:|\n| Alice |    42 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        t.insert_column(2, Alignment::None, &mut buffer).unwrap();
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        // original_lines re-captured against the new source.
        assert!(t.original_lines.is_some());
        // Per-cell edit still works in PreserveOriginal mode.
        t.update_cell(0, 0, "Bob", &mut buffer).unwrap();
        assert!(buffer.contains("Bob"));
    }

    #[test]
    fn reformat_pretty_aligns_columns_from_compact_source() {
        let original = "| a | name |\n|---|------|\n| x | longer cell |\n";
        let mut buffer = String::from(original);
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.reformat_pretty(&mut buffer).unwrap();
        // After reformat, header and body row should align pipe positions.
        let lines: Vec<&str> = buffer.lines().collect();
        let head_pipes: Vec<usize> = lines[0].match_indices('|').map(|(i, _)| i).collect();
        let body_pipes: Vec<usize> = lines[2].match_indices('|').map(|(i, _)| i).collect();
        assert_eq!(head_pipes, body_pipes);
    }

    #[test]
    fn reformat_pretty_leaves_style_in_preserve_mode() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        t.reformat_pretty(&mut buffer).unwrap();
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        // original_lines re-captured from the new (pretty) source.
        assert!(t.original_lines.is_some());
    }

    #[test]
    fn reformat_pretty_then_cell_edit_is_minimum_diff() {
        let mut buffer = String::from("| a | name |\n|---|------|\n| x | y |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.reformat_pretty(&mut buffer).unwrap();
        // After reformat, an in-place cell edit shouldn't disturb other cells.
        let before = buffer.clone();
        t.update_cell(0, 0, "z", &mut buffer).unwrap();
        // The unchanged cells (e.g., header) should be byte-identical.
        let header_line_before = before.lines().next().unwrap();
        let header_line_after = buffer.lines().next().unwrap();
        assert_eq!(header_line_before, header_line_after);
    }

    #[test]
    fn reformat_pretty_normalises_alignment_separator() {
        // Three-dash separator → pretty re-emit should still encode
        // alignment correctly even when widths grow.
        let mut buffer = String::from("| name | score |\n|:--|--:|\n| Alice | 42 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.reformat_pretty(&mut buffer).unwrap();
        // Left alignment: starting colon. Right alignment: trailing colon.
        assert!(buffer.contains(":-"), "buffer: {buffer}");
        assert!(buffer.contains("-:"), "buffer: {buffer}");
    }

    #[test]
    fn reformat_pretty_idempotent_on_already_pretty() {
        let mut buffer = String::from("| name  | score |\n|-------|-------|\n| Alice |    42 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let first = buffer.clone();
        t.reformat_pretty(&mut buffer).unwrap();
        let after_first = buffer.clone();
        // Second reformat should produce identical bytes.
        let mut tables2 = parse_tables(&buffer);
        tables2[0].reformat_pretty(&mut buffer).unwrap();
        assert_eq!(buffer, after_first);
        let _ = first;
    }

    #[test]
    fn reformat_pretty_empty_columns_errors() {
        // Build a degenerate table by hand — parser won't produce
        // alignments.len() == 0, but the function should refuse anyway.
        let mut table = MarkdownTable {
            id: 1,
            source_range: 0..0,
            alignments: vec![],
            headers: vec![],
            rows: vec![],
            style: TableStyle::PreserveOriginal,
            column_widths: vec![],
            original_lines: None,
            sort_indicator: None,
        };
        let mut buf = String::new();
        let err = table.reformat_pretty(&mut buf).unwrap_err();
        assert!(matches!(err, TableError::InvalidStructure(_)));
    }

    #[test]
    fn set_column_alignment_updates_alignments() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.set_column_alignment(1, Alignment::Center, &mut buffer)
            .unwrap();
        assert_eq!(t.alignments[1], Alignment::Center);
        assert!(buffer.contains(":-:") || buffer.contains(":---:"));
    }

    #[test]
    fn set_column_alignment_renders_left_separator() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.set_column_alignment(0, Alignment::Left, &mut buffer)
            .unwrap();
        assert!(buffer.contains(":-"), "buffer: {buffer}");
    }

    #[test]
    fn set_column_alignment_renders_right_separator() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.set_column_alignment(0, Alignment::Right, &mut buffer)
            .unwrap();
        assert!(buffer.contains("-:"), "buffer: {buffer}");
    }

    #[test]
    fn set_column_alignment_no_op_returns_empty_range() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let buffer_before = buffer.clone();
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        // Column already has Alignment::None.
        let delta = t
            .set_column_alignment(0, Alignment::None, &mut buffer)
            .unwrap();
        assert!(delta.patched_range.is_empty());
        assert_eq!(delta.byte_delta, 0);
        // Buffer untouched.
        assert_eq!(buffer, buffer_before);
    }

    #[test]
    fn set_column_alignment_out_of_range_errors() {
        let mut buffer = String::from("| a |\n|---|\n| 1 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let err = t
            .set_column_alignment(5, Alignment::Center, &mut buffer)
            .unwrap_err();
        assert!(matches!(err, TableError::CellOutOfRange { .. }));
    }

    #[test]
    fn set_column_alignment_preserves_preserve_original_style() {
        let original = "| name  | score |\n|-------|-------|\n| Alice |    42 |\n";
        let mut buffer = String::from(original);
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        t.set_column_alignment(1, Alignment::Right, &mut buffer)
            .unwrap();
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        assert!(t.original_lines.is_some());
        // Subsequent per-cell edits still patch in place.
        t.update_cell(0, 0, "Bob", &mut buffer).unwrap();
        assert!(buffer.contains("Bob"));
    }

    #[test]
    fn insert_column_pads_short_rows() {
        // Hand-crafted: parser never produces ragged rows but model
        // could after a sequence of edits. Defensive.
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        // Artificially make a row short to exercise the padding path.
        t.rows[0].pop();
        t.insert_column(2, Alignment::None, &mut buffer).unwrap();
        assert_eq!(t.rows[0].len(), 3);
    }

    #[test]
    fn set_column_widths_emits_comment() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.set_column_widths(vec![Some(180), None], &mut buffer)
            .unwrap();
        // Persistence comment now sits above the table.
        assert!(
            buffer.starts_with("<!-- rmd-cols: 180, -->\n"),
            "got: {buffer}"
        );
        // Re-parse should round-trip.
        let parsed = parse_tables(&buffer);
        assert_eq!(parsed[0].column_widths, vec![Some(180), None]);
    }

    #[test]
    fn set_column_widths_all_none_drops_comment() {
        // Pre-existing comment, then set widths to all-None — the
        // emit should not include the comment anymore.
        let mut buffer =
            String::from("<!-- rmd-cols: 100,200 -->\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        assert_eq!(tables[0].column_widths, vec![Some(100), Some(200)]);
        let t = &mut tables[0];
        t.set_column_widths(vec![None, None], &mut buffer).unwrap();
        assert!(
            !buffer.contains("rmd-cols"),
            "comment was not removed: {buffer}"
        );
    }

    #[test]
    fn set_column_widths_wrong_count_errors() {
        let mut buffer = String::from("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        let err = t
            .set_column_widths(vec![Some(100), Some(200), Some(300)], &mut buffer)
            .unwrap_err();
        assert!(matches!(err, TableError::InvalidStructure(_)));
    }

    #[test]
    fn set_column_widths_preserves_preserve_original_mode() {
        let original = "| name  | score |\n|-------|------:|\n| Alice |    42 |\n";
        let mut buffer = String::from(original);
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        t.set_column_widths(vec![Some(120), None], &mut buffer)
            .unwrap();
        assert_eq!(t.style, TableStyle::PreserveOriginal);
        // Cell edits still work surgically after widths are set.
        t.update_cell(0, 0, "Bob", &mut buffer).unwrap();
        assert!(buffer.contains("Bob"));
        assert!(buffer.contains("rmd-cols"));
    }

    #[test]
    fn column_widths_survive_insert_column() {
        let mut buffer =
            String::from("<!-- rmd-cols: 100,200 -->\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.insert_column(1, Alignment::None, &mut buffer).unwrap();
        // Existing widths preserved; the inserted column slot is None.
        assert_eq!(t.column_widths, vec![Some(100), None, Some(200)]);
        // Re-parse confirms the same values came back out.
        let parsed = parse_tables(&buffer);
        assert_eq!(parsed[0].column_widths, vec![Some(100), None, Some(200)]);
    }

    #[test]
    fn column_widths_survive_delete_column() {
        let mut buffer = String::from(
            "<!-- rmd-cols: 100,200,300 -->\n| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n",
        );
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.delete_column(1, &mut buffer).unwrap();
        assert_eq!(t.column_widths, vec![Some(100), Some(300)]);
        let parsed = parse_tables(&buffer);
        assert_eq!(parsed[0].column_widths, vec![Some(100), Some(300)]);
    }

    #[test]
    fn column_widths_survive_cell_edit_in_preserve_mode() {
        let mut buffer =
            String::from("<!-- rmd-cols: 100,200 -->\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        let mut tables = parse_tables(&buffer);
        let t = &mut tables[0];
        t.update_cell(0, 0, "z", &mut buffer).unwrap();
        // Comment still sits above the table after a surgical edit.
        assert!(buffer.starts_with("<!-- rmd-cols: 100,200 -->\n"));
    }

    #[test]
    fn build_column_widths_comment_format() {
        assert_eq!(
            build_column_widths_comment(&[Some(180), None, Some(120)]),
            Some("<!-- rmd-cols: 180,,120 -->".to_string())
        );
        assert_eq!(build_column_widths_comment(&[None, None]), None);
        assert_eq!(
            build_column_widths_comment(&[Some(50)]),
            Some("<!-- rmd-cols: 50 -->".to_string())
        );
    }
}
