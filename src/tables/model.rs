//! Core data structures for the markdown table subsystem.
//!
//! Everything here is parser- and renderer-agnostic; [`parse`](super::parse)
//! and [`serialize`](super::serialize) plug into these types.

use std::fmt;
use std::ops::Range;

/// Stable identifier for a table in a document. Survives byte-level edits;
/// only the document-level table registry assigns new IDs (on insert,
/// delete, or wholesale replace).
pub type TableId = u64;

/// A parsed markdown table.
///
/// Cells store **raw markdown source** verbatim — formatting markers like
/// `**bold**` or `[link](url)` are kept as-is for lossless round-trip.
/// Rendering of inline content happens at display time, not at parse time.
#[derive(Clone, Debug)]
pub struct MarkdownTable {
    /// Stable identifier — what the DOM addresses cells by.
    pub id: TableId,

    /// Byte range in the buffer covering the entire table block,
    /// including the trailing newline when present. Shifts on every
    /// edit; never store this in the DOM.
    pub source_range: Range<usize>,

    /// Per-column alignment from the separator row.
    pub alignments: Vec<Alignment>,

    /// Header cells (always present in GFM).
    pub headers: Vec<Cell>,

    /// Body rows in document order.
    pub rows: Vec<Vec<Cell>>,

    /// How this table should be re-serialized after edits.
    pub style: TableStyle,

    /// Optional per-column widths (HTML-comment extension; degrades
    /// gracefully in viewers that don't recognise the comment).
    /// `None` per column = auto-size.
    pub column_widths: Vec<Option<u32>>,

    /// Captured raw line bytes, populated only when
    /// `style == TableStyle::PreserveOriginal`. Lets [`update_cell`]
    /// patch a single cell's content in place without disturbing
    /// user-formatted whitespace.
    ///
    /// [`update_cell`]: MarkdownTable::update_cell
    pub original_lines: Option<Vec<OriginalLine>>,

    /// Transient: which column is currently sorted and in what
    /// direction. Hydrated by callers from per-document sort state
    /// (typically a `HashMap<TableId, ...>` on the host application)
    /// after `parse_tables`; consumed by the render post-processor
    /// to emit `data-sort-dir` on the active header cell so the
    /// frontend can show the tri-state indicator on the toolbar.
    pub sort_indicator: Option<(usize, SortDirection)>,
}

/// A single table cell.
///
/// `source_range` points at the cell's **content** — between the
/// surrounding `|`s, inclusive of any leading/trailing whitespace.
/// The pipe characters themselves are *not* part of this range.
#[derive(Clone, Debug)]
pub struct Cell {
    pub source_range: Range<usize>,

    /// Trimmed inner content (raw markdown). For an empty cell this is
    /// the empty string.
    pub content: String,

    /// Leading whitespace bytes inside the cell. Used for
    /// padding-aware minimum-diff edits.
    pub leading_ws: u8,

    /// Trailing whitespace bytes inside the cell.
    pub trailing_ws: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    None,
    Left,
    Center,
    Right,
}

/// Round-trip strategy for re-serialising a table after edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableStyle {
    /// `| a | b |` — single-space padding, no column alignment.
    /// Always reformats on edit. Fully handled by the serializer, but no
    /// UI path currently selects it (there's no "compact" command yet);
    /// kept as a complete member of the style enum.
    #[allow(dead_code)]
    Compact,
    /// `| a   | b   |` — pretty-printed with column widths recomputed.
    /// Default for *newly created* tables.
    Pretty,
    /// Preserve user's exact whitespace. Cell edits patch in place;
    /// structural edits (insert/delete row or column) fall back to
    /// pretty-printing the affected rows. Default for *parsed* tables.
    PreserveOriginal,
}

/// One line of original table source, kept verbatim for the
/// PreserveOriginal round-trip strategy.
#[derive(Clone, Debug)]
pub struct OriginalLine {
    /// Raw line bytes (no trailing newline).
    pub raw: String,
    /// Byte range *within `raw`* for each cell's content (between pipes).
    pub cell_spans: Vec<Range<usize>>,
    /// Which structural part of the table this line is.
    pub kind: LineKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Header,
    Separator,
    Body { row_index: usize },
}

/// Information returned by an edit. The caller applies `byte_delta` to
/// every other table's `source_range` (downstream tables shift forward
/// or backward by exactly this many bytes).
#[derive(Clone, Debug)]
pub struct EditDelta {
    /// `new_len - old_len` for the edited cell.
    pub byte_delta: isize,
    /// The buffer range that was replaced.
    pub patched_range: Range<usize>,
    /// The new buffer range covering the same logical cell.
    pub new_range: Range<usize>,
}

/// Sort direction for [`sort_rows`] and the `Sort` toolbar control.
///
/// The toolbar cycles `None → Ascending → Descending → None`. Backend
/// behaviour:
///
///   - `Ascending` / `Descending`: re-orders rows in place. The caller
///     keeps a snapshot of the pre-sort rows so a subsequent `None`
///     click can restore the original order.
///   - `None`: signals the caller should restore from its snapshot.
///     `sort_rows` itself just returns the rows unchanged in this case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    None,
    Ascending,
    Descending,
}

/// Sort `rows` by the content of `col`, returning a freshly ordered
/// `Vec`. Smart numeric/text detection per column:
///
///   - If every non-empty cell in `col` parses as `f64`, the column
///     is treated as numeric and sorted by numeric value (so `"10"`
///     sorts after `"2"`).
///   - Otherwise: case-insensitive lexicographic comparison.
///
/// Empty cells go to the end regardless of direction. Sort is stable
/// (preserves the relative order of equal values). `SortDirection::None`
/// returns `rows` unchanged.
pub fn sort_rows(rows: Vec<Vec<Cell>>, col: usize, direction: SortDirection) -> Vec<Vec<Cell>> {
    if matches!(direction, SortDirection::None) {
        return rows;
    }
    let numeric = is_numeric_column(&rows, col);
    let mut out = rows;
    out.sort_by(|a, b| compare_rows(a, b, col, numeric, direction));
    out
}

/// `true` when at least one non-empty cell exists in `col` *and* every
/// non-empty cell parses as `f64`. Empty cells are skipped — they
/// don't drag the column into "text" mode.
pub fn is_numeric_column(rows: &[Vec<Cell>], col: usize) -> bool {
    let mut any = false;
    for row in rows {
        let content = row.get(col).map(|c| c.content.as_str()).unwrap_or("");
        if content.trim().is_empty() {
            continue;
        }
        if content.trim().parse::<f64>().is_err() {
            return false;
        }
        any = true;
    }
    any
}

fn compare_rows(
    a: &[Cell],
    b: &[Cell],
    col: usize,
    numeric: bool,
    direction: SortDirection,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let a_content = a.get(col).map(|c| c.content.as_str()).unwrap_or("").trim();
    let b_content = b.get(col).map(|c| c.content.as_str()).unwrap_or("").trim();

    // Empties go to the end regardless of direction.
    match (a_content.is_empty(), b_content.is_empty()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }

    let base = if numeric {
        let an: f64 = a_content.parse().unwrap_or(0.0);
        let bn: f64 = b_content.parse().unwrap_or(0.0);
        an.partial_cmp(&bn).unwrap_or(Ordering::Equal)
    } else {
        a_content.to_lowercase().cmp(&b_content.to_lowercase())
    };
    match direction {
        SortDirection::Descending => base.reverse(),
        _ => base,
    }
}

/// Direction for [`MarkdownTable::navigate`].
///
/// Maps directly to the keys the user pressed inside an editable cell:
///   - `Tab` → [`NavDirection::Next`]
///   - `Shift+Tab` → [`NavDirection::Prev`]
///   - `Enter` → [`NavDirection::Down`]
///   - `Shift+Enter` → [`NavDirection::Up`]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDirection {
    Next,
    Prev,
    Down,
    Up,
}

/// Result of a navigation query — `(row, col)` the caller should focus
/// next, plus whether the model already had room for that cell or
/// whether the caller needs to insert an empty row first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavTarget {
    pub row: i32,
    pub col: usize,
    /// `true` when the user navigated past the end of the body — the
    /// caller should run `insert_empty_row(row)` before focusing.
    pub created_row: bool,
}

/// Errors produced by the table subsystem.
///
/// Some variants aren't returned by any current code path (CSV/HTML import
/// and table-lookup-by-id aren't wired into the editor yet) but are kept so
/// the error type is complete and stable for callers that match on it.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum TableError {
    CellOutOfRange { row: i32, col: usize },
    TableNotFound(TableId),
    NotPreserveMode,
    InvalidCsv(String),
    InvalidHtml(String),
    InvalidStructure(String),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::CellOutOfRange { row, col } => {
                write!(f, "cell ({row}, {col}) is out of range")
            }
            TableError::TableNotFound(id) => write!(f, "table {id} not found"),
            TableError::NotPreserveMode => write!(f, "operation requires PreserveOriginal mode"),
            TableError::InvalidCsv(msg) => write!(f, "invalid CSV: {msg}"),
            TableError::InvalidHtml(msg) => write!(f, "invalid HTML: {msg}"),
            TableError::InvalidStructure(msg) => write!(f, "invalid table structure: {msg}"),
        }
    }
}

impl std::error::Error for TableError {}

pub type Result<T> = std::result::Result<T, TableError>;

impl MarkdownTable {
    /// Borrow a cell. `row == -1` is the header.
    pub fn cell(&self, row: i32, col: usize) -> Result<&Cell> {
        if row == -1 {
            self.headers.get(col)
        } else {
            self.rows.get(row as usize).and_then(|r| r.get(col))
        }
        .ok_or(TableError::CellOutOfRange { row, col })
    }

    /// Mutably borrow a cell. `row == -1` is the header.
    pub fn cell_mut(&mut self, row: i32, col: usize) -> Result<&mut Cell> {
        if row == -1 {
            self.headers.get_mut(col)
        } else {
            self.rows.get_mut(row as usize).and_then(|r| r.get_mut(col))
        }
        .ok_or(TableError::CellOutOfRange { row, col })
    }

    /// Shift every cell's `source_range` (and the table's own range)
    /// that starts at or after `after` by `delta` bytes. Called after
    /// a cell edit propagates within the table.
    pub fn shift_after(&mut self, after: usize, delta: isize) {
        let apply = |r: &mut Range<usize>| {
            if r.start >= after {
                r.start = (r.start as isize + delta) as usize;
            }
            if r.end >= after {
                r.end = (r.end as isize + delta) as usize;
            }
        };
        for c in &mut self.headers {
            apply(&mut c.source_range);
        }
        for row in &mut self.rows {
            for c in row {
                apply(&mut c.source_range);
            }
        }
        if self.source_range.end >= after {
            self.source_range.end = (self.source_range.end as isize + delta) as usize;
        }
    }

    /// Edit a single cell. See [`crate::tables::serialize`] for the
    /// implementation. Returns the patch info for document-level
    /// shift accounting.
    pub fn update_cell(
        &mut self,
        row: i32,
        col: usize,
        new_content: &str,
        buffer: &mut String,
    ) -> Result<EditDelta> {
        crate::tables::serialize::update_cell(self, row, col, new_content, buffer)
    }

    /// Insert an empty body row at the given 0-based index. Triggers
    /// a structural re-serialize (Pretty for PreserveOriginal tables,
    /// since we don't have an `OriginalLine` to splice). See
    /// [`crate::tables::serialize`] for the implementation.
    pub fn insert_empty_row(&mut self, at_index: usize, buffer: &mut String) -> Result<EditDelta> {
        crate::tables::serialize::insert_empty_row(self, at_index, buffer)
    }

    /// Delete the body row at `row_index`. Header is never removed
    /// this way (caller should refuse `row == -1` upstream).
    pub fn delete_row(&mut self, row_index: usize, buffer: &mut String) -> Result<EditDelta> {
        crate::tables::serialize::delete_row(self, row_index, buffer)
    }

    /// Insert an empty column at `at_index` with the given alignment.
    /// Header, body rows, alignments, and column_widths all get the
    /// new column.
    pub fn insert_column(
        &mut self,
        at_index: usize,
        alignment: Alignment,
        buffer: &mut String,
    ) -> Result<EditDelta> {
        crate::tables::serialize::insert_column(self, at_index, alignment, buffer)
    }

    /// Delete the column at `col_index`. Refuses to remove the last
    /// remaining column.
    pub fn delete_column(&mut self, col_index: usize, buffer: &mut String) -> Result<EditDelta> {
        crate::tables::serialize::delete_column(self, col_index, buffer)
    }

    /// Set a column's alignment. Idempotent — calling with the
    /// column's current alignment returns an empty-range EditDelta
    /// so the caller can skip the buffer patch.
    pub fn set_column_alignment(
        &mut self,
        col: usize,
        alignment: Alignment,
        buffer: &mut String,
    ) -> Result<EditDelta> {
        crate::tables::serialize::set_column_alignment(self, col, alignment, buffer)
    }

    /// Force-reformat the table with pretty column alignment,
    /// recomputing widths from current cell contents. Leaves the
    /// table in PreserveOriginal mode so subsequent per-cell edits
    /// stay surgical against the now-pretty source. See
    /// [`crate::tables::serialize::reformat_pretty`] for details.
    pub fn reformat_pretty(&mut self, buffer: &mut String) -> Result<EditDelta> {
        crate::tables::serialize::reformat_pretty(self, buffer)
    }

    /// Replace the table's body rows wholesale (used by the sort
    /// flow, which computes the new order outside the model and
    /// commits it here as one structural operation).
    pub fn replace_rows(
        &mut self,
        new_rows: Vec<Vec<Cell>>,
        buffer: &mut String,
    ) -> Result<EditDelta> {
        crate::tables::serialize::replace_rows(self, new_rows, buffer)
    }

    /// Set the column widths used by the resize handles. Triggers a
    /// structural re-serialize so the persistence comment is
    /// written/refreshed in front of the table. Pass all-`None`
    /// widths to drop the comment entirely.
    pub fn set_column_widths(
        &mut self,
        widths: Vec<Option<u32>>,
        buffer: &mut String,
    ) -> Result<EditDelta> {
        crate::tables::serialize::set_column_widths(self, widths, buffer)
    }

    /// Compute the navigation target for a Tab / Shift+Tab / Enter
    /// / Shift+Enter press inside the cell at `(row, col)`.
    ///
    /// Pure logic — does not mutate the table. If
    /// `result.created_row == true`, the caller is responsible for
    /// running [`insert_empty_row`] at `result.row` before focusing.
    ///
    /// Convention: `row == -1` is the header. Behaviour:
    ///
    /// - **Next**: right one cell; wrap from header end to first body
    ///   row; from end of last body row, signal `created_row` to
    ///   append a new empty row.
    /// - **Prev**: left one cell; wrap from start of a body row to
    ///   end of previous row; from start of first body row to end of
    ///   header. No-op at `(-1, 0)`.
    /// - **Down**: same column, next row; signals `created_row` past
    ///   the last body row.
    /// - **Up**: same column, previous row; from first body row to
    ///   header; no-op at the header.
    ///
    /// [`insert_empty_row`]: MarkdownTable::insert_empty_row
    pub fn navigate(&self, row: i32, col: usize, dir: NavDirection) -> NavTarget {
        let cols = self.alignments.len();
        let body_rows = self.rows.len() as i32;
        // Empty table guard — should be impossible for a parsed GFM
        // table, but be defensive.
        if cols == 0 {
            return NavTarget {
                row,
                col,
                created_row: false,
            };
        }
        let last_col = cols - 1;
        match dir {
            NavDirection::Next => {
                if col + 1 < cols {
                    NavTarget {
                        row,
                        col: col + 1,
                        created_row: false,
                    }
                } else if row == -1 {
                    // End of header → first body cell. Append row if
                    // body is empty.
                    NavTarget {
                        row: 0,
                        col: 0,
                        created_row: body_rows == 0,
                    }
                } else if row + 1 < body_rows {
                    NavTarget {
                        row: row + 1,
                        col: 0,
                        created_row: false,
                    }
                } else {
                    // Tab at the very last cell → spreadsheet-style
                    // "append a row" magic.
                    NavTarget {
                        row: body_rows,
                        col: 0,
                        created_row: true,
                    }
                }
            }
            NavDirection::Prev => {
                if col > 0 {
                    NavTarget {
                        row,
                        col: col - 1,
                        created_row: false,
                    }
                } else if row > 0 {
                    NavTarget {
                        row: row - 1,
                        col: last_col,
                        created_row: false,
                    }
                } else if row == 0 {
                    NavTarget {
                        row: -1,
                        col: last_col,
                        created_row: false,
                    }
                } else {
                    // At (-1, 0) — top-left of header; nowhere to go.
                    NavTarget {
                        row,
                        col,
                        created_row: false,
                    }
                }
            }
            NavDirection::Down => {
                if row == -1 {
                    NavTarget {
                        row: 0,
                        col,
                        created_row: body_rows == 0,
                    }
                } else if row + 1 < body_rows {
                    NavTarget {
                        row: row + 1,
                        col,
                        created_row: false,
                    }
                } else {
                    NavTarget {
                        row: body_rows,
                        col,
                        created_row: true,
                    }
                }
            }
            NavDirection::Up => {
                if row == -1 {
                    NavTarget {
                        row,
                        col,
                        created_row: false,
                    }
                } else if row == 0 {
                    NavTarget {
                        row: -1,
                        col,
                        created_row: false,
                    }
                } else {
                    NavTarget {
                        row: row - 1,
                        col,
                        created_row: false,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_table() -> MarkdownTable {
        MarkdownTable {
            id: 1,
            source_range: 0..40,
            alignments: vec![Alignment::None, Alignment::None],
            headers: vec![
                Cell {
                    source_range: 1..7,
                    content: "a".into(),
                    leading_ws: 1,
                    trailing_ws: 1,
                },
                Cell {
                    source_range: 8..14,
                    content: "b".into(),
                    leading_ws: 1,
                    trailing_ws: 1,
                },
            ],
            rows: vec![vec![
                Cell {
                    source_range: 25..31,
                    content: "1".into(),
                    leading_ws: 1,
                    trailing_ws: 1,
                },
                Cell {
                    source_range: 32..38,
                    content: "2".into(),
                    leading_ws: 1,
                    trailing_ws: 1,
                },
            ]],
            style: TableStyle::Compact,
            column_widths: vec![None, None],
            original_lines: None,
            sort_indicator: None,
        }
    }

    #[test]
    fn cell_accessor_handles_header_and_body() {
        let t = sample_table();
        assert_eq!(t.cell(-1, 0).unwrap().content, "a");
        assert_eq!(t.cell(0, 1).unwrap().content, "2");
        assert!(matches!(
            t.cell(99, 0),
            Err(TableError::CellOutOfRange { .. })
        ));
    }

    #[test]
    fn shift_after_moves_only_affected_ranges() {
        let mut t = sample_table();
        t.shift_after(20, 5);
        assert_eq!(t.headers[0].source_range, 1..7); // untouched
        assert_eq!(t.rows[0][0].source_range, 30..36); // +5
        assert_eq!(t.source_range, 0..45); // table end shifted
    }

    #[test]
    fn navigate_next_advances_within_row() {
        let t = sample_table();
        let n = t.navigate(0, 0, NavDirection::Next);
        assert_eq!(
            n,
            NavTarget {
                row: 0,
                col: 1,
                created_row: false
            }
        );
    }

    #[test]
    fn navigate_next_wraps_header_to_body() {
        let t = sample_table();
        let n = t.navigate(-1, 1, NavDirection::Next);
        assert_eq!(
            n,
            NavTarget {
                row: 0,
                col: 0,
                created_row: false
            }
        );
    }

    #[test]
    fn navigate_next_at_last_cell_signals_new_row() {
        let t = sample_table();
        let n = t.navigate(0, 1, NavDirection::Next);
        assert_eq!(
            n,
            NavTarget {
                row: 1,
                col: 0,
                created_row: true
            }
        );
    }

    #[test]
    fn navigate_prev_wraps_to_previous_row_end() {
        let t = sample_table();
        let n = t.navigate(0, 0, NavDirection::Prev);
        assert_eq!(
            n,
            NavTarget {
                row: -1,
                col: 1,
                created_row: false
            }
        );
    }

    #[test]
    fn navigate_prev_no_op_at_top_left_of_header() {
        let t = sample_table();
        let n = t.navigate(-1, 0, NavDirection::Prev);
        assert_eq!(
            n,
            NavTarget {
                row: -1,
                col: 0,
                created_row: false
            }
        );
    }

    #[test]
    fn navigate_down_appends_row_at_end() {
        let t = sample_table();
        let n = t.navigate(0, 1, NavDirection::Down);
        assert_eq!(
            n,
            NavTarget {
                row: 1,
                col: 1,
                created_row: true
            }
        );
    }

    #[test]
    fn navigate_down_from_header_lands_in_body() {
        let t = sample_table();
        let n = t.navigate(-1, 0, NavDirection::Down);
        assert_eq!(
            n,
            NavTarget {
                row: 0,
                col: 0,
                created_row: false
            }
        );
    }

    #[test]
    fn navigate_up_from_first_body_lands_in_header() {
        let t = sample_table();
        let n = t.navigate(0, 1, NavDirection::Up);
        assert_eq!(
            n,
            NavTarget {
                row: -1,
                col: 1,
                created_row: false
            }
        );
    }

    #[test]
    fn navigate_handles_empty_body() {
        // Header-only table.
        let t = MarkdownTable {
            rows: vec![],
            ..sample_table()
        };
        let n = t.navigate(-1, 1, NavDirection::Next);
        assert_eq!(
            n,
            NavTarget {
                row: 0,
                col: 0,
                created_row: true
            }
        );
    }

    fn rows_with(values: &[&[&str]]) -> Vec<Vec<Cell>> {
        values
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| Cell {
                        source_range: 0..0,
                        content: (*c).to_string(),
                        leading_ws: 1,
                        trailing_ws: 1,
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn sort_rows_numeric_column_uses_numeric_order() {
        let rows = rows_with(&[&["x", "2"], &["y", "10"], &["z", "1"]]);
        let sorted = sort_rows(rows, 1, SortDirection::Ascending);
        assert_eq!(sorted[0][1].content, "1");
        assert_eq!(sorted[1][1].content, "2");
        assert_eq!(sorted[2][1].content, "10");
    }

    #[test]
    fn sort_rows_text_column_uses_case_insensitive_order() {
        let rows = rows_with(&[&["Banana"], &["apple"], &["Cherry"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Ascending);
        assert_eq!(sorted[0][0].content, "apple");
        assert_eq!(sorted[1][0].content, "Banana");
        assert_eq!(sorted[2][0].content, "Cherry");
    }

    #[test]
    fn sort_rows_descending() {
        let rows = rows_with(&[&["a"], &["c"], &["b"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Descending);
        assert_eq!(sorted[0][0].content, "c");
        assert_eq!(sorted[1][0].content, "b");
        assert_eq!(sorted[2][0].content, "a");
    }

    #[test]
    fn sort_rows_none_is_identity() {
        let rows = rows_with(&[&["b"], &["a"], &["c"]]);
        let sorted = sort_rows(rows.clone(), 0, SortDirection::None);
        for (orig, after) in rows.iter().zip(sorted.iter()) {
            assert_eq!(orig[0].content, after[0].content);
        }
    }

    #[test]
    fn sort_rows_empty_cells_go_last() {
        let rows = rows_with(&[&["b"], &[""], &["a"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Ascending);
        assert_eq!(sorted[0][0].content, "a");
        assert_eq!(sorted[1][0].content, "b");
        assert_eq!(sorted[2][0].content, "");
        // And in descending, empties also go last (not first).
        let rows = rows_with(&[&["b"], &[""], &["a"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Descending);
        assert_eq!(sorted[0][0].content, "b");
        assert_eq!(sorted[1][0].content, "a");
        assert_eq!(sorted[2][0].content, "");
    }

    #[test]
    fn sort_rows_mixed_numeric_and_text_uses_text_order() {
        // "abc" doesn't parse as f64 → whole column treated as text.
        // Then "10" < "2" alphabetically.
        let rows = rows_with(&[&["abc"], &["2"], &["10"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Ascending);
        assert_eq!(sorted[0][0].content, "10");
        assert_eq!(sorted[1][0].content, "2");
        assert_eq!(sorted[2][0].content, "abc");
    }

    #[test]
    fn sort_rows_is_stable_for_ties() {
        // Two rows have the same content in col 0; their relative
        // order must be preserved.
        let rows = rows_with(&[&["a", "first"], &["a", "second"], &["a", "third"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Ascending);
        assert_eq!(sorted[0][1].content, "first");
        assert_eq!(sorted[1][1].content, "second");
        assert_eq!(sorted[2][1].content, "third");
    }

    #[test]
    fn sort_rows_handles_negative_and_float_numerics() {
        let rows = rows_with(&[&["-3"], &["1.5"], &["0"], &["-0.5"]]);
        let sorted = sort_rows(rows, 0, SortDirection::Ascending);
        assert_eq!(sorted[0][0].content, "-3");
        assert_eq!(sorted[1][0].content, "-0.5");
        assert_eq!(sorted[2][0].content, "0");
        assert_eq!(sorted[3][0].content, "1.5");
    }

    #[test]
    fn is_numeric_column_skips_empty_cells() {
        let rows = rows_with(&[&["2"], &[""], &["10"]]);
        assert!(is_numeric_column(&rows, 0));
    }

    #[test]
    fn is_numeric_column_false_for_mixed() {
        let rows = rows_with(&[&["2"], &["abc"], &["10"]]);
        assert!(!is_numeric_column(&rows, 0));
    }

    #[test]
    fn is_numeric_column_false_for_all_empty() {
        let rows = rows_with(&[&[""], &[""], &[""]]);
        assert!(!is_numeric_column(&rows, 0));
    }

    #[test]
    fn navigate_single_column_table() {
        // Only one column → Tab/Enter behave identically.
        let t = MarkdownTable {
            alignments: vec![Alignment::None],
            headers: vec![Cell {
                source_range: 1..3,
                content: "a".into(),
                leading_ws: 1,
                trailing_ws: 1,
            }],
            rows: vec![vec![Cell {
                source_range: 6..8,
                content: "1".into(),
                leading_ws: 1,
                trailing_ws: 1,
            }]],
            ..sample_table()
        };
        let n = t.navigate(-1, 0, NavDirection::Next);
        assert_eq!(n.row, 0);
        assert_eq!(n.col, 0);
        let n = t.navigate(0, 0, NavDirection::Next);
        assert!(n.created_row);
    }
}
