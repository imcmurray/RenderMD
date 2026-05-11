//! Core data structures for the markdown table subsystem.
//!
//! Everything here is parser- and renderer-agnostic; [`parse`](super::parse)
//! and [`serialize`](super::serialize) plug into these types.

use std::collections::HashMap;
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

    /// Cached formulas keyed by `(row, col)` where `row == -1` is the
    /// header. The source of truth is HTML-comment sidecar metadata
    /// adjacent to the table, so the markdown stays portable.
    pub formulas: HashMap<(i32, usize), String>,

    /// Captured raw line bytes, populated only when
    /// `style == TableStyle::PreserveOriginal`. Lets [`update_cell`]
    /// patch a single cell's content in place without disturbing
    /// user-formatted whitespace.
    ///
    /// [`update_cell`]: MarkdownTable::update_cell
    pub original_lines: Option<Vec<OriginalLine>>,
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
    /// Always reformats on edit.
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

/// Errors produced by the table subsystem.
#[derive(Clone, Debug)]
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
    /// Total cell count (header + all body cells).
    pub fn cell_count(&self) -> usize {
        self.headers.len() + self.rows.iter().map(Vec::len).sum::<usize>()
    }

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
            formulas: HashMap::new(),
            original_lines: None,
        }
    }

    #[test]
    fn cell_count_counts_header_and_body() {
        let t = sample_table();
        assert_eq!(t.cell_count(), 4);
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
}
