//! Markdown table subsystem.
//!
//! Parses GFM pipe-tables into a stable, round-trippable in-memory
//! representation so the editor can offer click-to-edit cells and
//! smart-paste without disturbing the on-disk file's formatting.
//!
//! # Design
//!
//! - **Stable IDs, ephemeral source ranges.** Tables and cells carry
//!   stable [`TableId`] values that survive byte-level edits. Source
//!   ranges are computed from the buffer and shift on every change;
//!   only the backend tracks them.
//! - **PreserveOriginal by default.** Parsed tables keep their original
//!   line bytes so cell edits patch surgically — column padding the user
//!   hand-tuned isn't disturbed. Newly created tables use
//!   [`TableStyle::Pretty`] for column-aligned output.
//! - **Cells store raw markdown.** Inline formatting (`**bold**`,
//!   `[link](x)`, etc.) lives in [`Cell::content`] verbatim; rendering
//!   happens at display time.
//! - **No coupling to the buffer abstraction.** Functions take
//!   `&str` / `&mut String` so the same code works against a `Rope`,
//!   `GtkTextBuffer` snapshot, or plain `String`.
//!
//! # Integration sketch
//!
//! ```ignore
//! let mut tables = parse::parse_tables(&buffer);
//! // ...user clicks cell (row, col) of `table_id`...
//! let delta = tables[idx].update_cell(row, col, &new_content, &mut buffer)?;
//! // Then shift other tables in the document by `delta.byte_delta`.
//! ```
//!
//! See `parse::parse_tables`, `MarkdownTable::update_cell`, and the
//! `paste` module for the three load-bearing entry points.

// Scaffolding for the table editor — wired into webview commands in a
// follow-up. Allowing dead_code / unused_imports until the integration
// layer (click-to-edit, smart paste) consumes these re-exports.
#![allow(dead_code, unused_imports)]

pub mod model;
pub mod parse;
pub mod paste;
pub mod render;
pub mod serialize;

pub use model::{
    Alignment, Cell, EditDelta, LineKind, MarkdownTable, OriginalLine, Result, TableError, TableId,
    TableStyle,
};
pub use parse::parse_tables;
pub use paste::{detect_table_paste, paste_to_gfm, TablePaste};
