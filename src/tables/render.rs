//! Post-process comrak's table HTML to add the data attributes the
//! frontend uses for click-to-edit cells, and ship the JS that wires
//! the click → edit → commit flow.
//!
//! The strategy is: comrak owns rendering, this module owns the
//! addressing layer. We walk the HTML string byte-by-byte, count
//! `<table>` opens to match against the parsed [`MarkdownTable`]
//! list (in document order), and inject:
//!
//! - `data-table-id="N"` — stable id of the table this cell belongs to
//! - `data-row="-1"` for header cells, `0..` for body rows
//! - `data-col="0.."` — column index within the row
//! - `data-raw="..."` — percent-encoded raw markdown source of the
//!   cell (so JS can show the user the source on click without a
//!   round-trip to Rust)
//! - `class="rmd-cell"` — what the JS click handler targets
//!
//! Defensive when the HTML contains tables the parser didn't see
//! (e.g., raw `<table>` HTML blocks): cells in unknown tables get
//! no attributes and are skipped over without breaking the scan.

use super::model::{Alignment, MarkdownTable, SortDirection};

/// Inject `data-*` attributes on every `<th>`/`<td>` belonging to a
/// table that the parser recognised. Other HTML passes through
/// byte-identical.
pub fn inject_table_attrs(html: &str, tables: &[MarkdownTable]) -> String {
    if tables.is_empty() || !html.contains("<table") {
        return html.to_string();
    }

    let mut out = String::with_capacity(html.len() + tables.len() * 256);

    let mut i = 0usize;
    let mut table_idx = 0usize;
    let mut in_table = false;
    let mut in_head = false;
    let mut in_row = false;
    let mut row_idx: i32 = -1;
    let mut col_idx: usize = 0;

    while i < html.len() {
        let bytes = html.as_bytes();
        if bytes[i] == b'<' {
            let tag_end = match html[i..].find('>') {
                Some(p) => i + p + 1,
                None => {
                    // Malformed HTML — emit the rest verbatim.
                    out.push_str(&html[i..]);
                    return out;
                }
            };
            let tag = &html[i..tag_end];
            let name = peek_tag_name(tag);

            // Track structural state. We do this *before* deciding
            // whether to inject — opening `<tr>` resets col_idx, etc.
            match name.as_str() {
                "table" => in_table = true,
                "/table" => {
                    in_table = false;
                    in_head = false;
                    in_row = false;
                    table_idx += 1;
                    row_idx = -1;
                    col_idx = 0;
                }
                "thead" => {
                    in_head = true;
                    row_idx = -1;
                }
                "/thead" => in_head = false,
                "tbody" => {
                    row_idx = -1;
                }
                "/tbody" => {}
                "tr" => {
                    in_row = true;
                    col_idx = 0;
                    if !in_head {
                        row_idx += 1;
                    }
                }
                "/tr" => in_row = false,
                _ => {}
            }

            let is_cell_open =
                (name == "th" || name == "td") && in_table && in_row && table_idx < tables.len();

            if is_cell_open {
                let table = &tables[table_idx];
                let raw = if in_head {
                    table.headers.get(col_idx).map(|c| c.content.as_str())
                } else {
                    table
                        .rows
                        .get(row_idx as usize)
                        .and_then(|r| r.get(col_idx))
                        .map(|c| c.content.as_str())
                };
                if let Some(raw_md) = raw {
                    let raw_encoded = percent_encode(raw_md);
                    let row_attr = if in_head { -1 } else { row_idx };
                    let align_attr = match table.alignments.get(col_idx).copied() {
                        Some(Alignment::Left) => "left",
                        Some(Alignment::Center) => "center",
                        Some(Alignment::Right) => "right",
                        _ => "none",
                    };
                    // data-sort-dir is set ONLY on the header cell of the
                    // currently-sorted column, so the JS toolbar reads it
                    // off the active cell to show the right tri-state.
                    let sort_attr = if in_head {
                        match table.sort_indicator {
                            Some((sc, SortDirection::Ascending)) if sc == col_idx => {
                                " data-sort-dir=\"asc\""
                            }
                            Some((sc, SortDirection::Descending)) if sc == col_idx => {
                                " data-sort-dir=\"desc\""
                            }
                            _ => "",
                        }
                    } else {
                        ""
                    };
                    let attrs = format!(
                        r#" class="rmd-cell" data-table-id="{}" data-row="{}" data-col="{}" data-align="{}"{} data-raw="{}""#,
                        table.id, row_attr, col_idx, align_attr, sort_attr, raw_encoded
                    );
                    // Inject just before the closing `>`. Defensive against
                    // self-closing `<th/>` even though comrak doesn't emit them.
                    let close = tag.rfind('>').unwrap();
                    let pre_close = if tag[..close].ends_with('/') {
                        close - 1
                    } else {
                        close
                    };
                    out.push_str(&tag[..pre_close]);
                    out.push_str(&attrs);
                    out.push_str(&tag[pre_close..]);
                    col_idx += 1;
                    i = tag_end;
                    continue;
                }
            }
            // Still advance col_idx for unknown/extra cells so the
            // ordering stays consistent if a future cell falls back
            // into the parsed range.
            if (name == "th" || name == "td") && in_table && in_row {
                col_idx += 1;
            }

            out.push_str(tag);
            i = tag_end;
        } else {
            // Chunk-copy until the next '<' to stay unicode-safe.
            let next = html[i..].find('<').map(|p| i + p).unwrap_or(html.len());
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    out
}

/// Extract the tag name (or `/name` for end tags) from a `<...>` slice,
/// lowercased. Returns the empty string for unparseable tags.
fn peek_tag_name(tag: &str) -> String {
    let inner = tag.trim_start_matches('<').trim_end_matches('>').trim();
    let mut chars = inner.chars();
    let mut name = String::new();
    if let Some(c) = chars.next() {
        if c == '/' {
            name.push('/');
            for c in chars {
                if c.is_ascii_alphanumeric() {
                    name.push(c.to_ascii_lowercase());
                } else {
                    break;
                }
            }
        } else if c.is_ascii_alphabetic() {
            name.push(c.to_ascii_lowercase());
            for c in chars {
                if c.is_ascii_alphanumeric() {
                    name.push(c.to_ascii_lowercase());
                } else {
                    break;
                }
            }
        }
    }
    name
}

/// RFC 3986 unreserved-character percent encoding. Compatible with
/// `decodeURIComponent` in JavaScript on the frontend.
pub fn percent_encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{:02X}", b);
        }
    }
    out
}

/// CSS + JS shipped with every preview page that has at least one
/// table. Wires up click → contenteditable → blur/Enter/Tab/Esc →
/// tab-delimited message to Rust. Idempotent on repeated injection
/// (the IIFE bails when there's no `tableEdit` handler).
///
/// Exposes `window.rmdFocusCell(tableId, row, col)` so Rust can
/// inject a one-shot script after `refresh_preview` to programmat-
/// ically focus a target cell — used by the Tab/Enter navigation
/// flow to keep the edit experience continuous across re-renders.
pub const TABLE_EDIT_JS: &str = r#"
<script>
(function() {
  var msg = (window.webkit && window.webkit.messageHandlers) || null;
  if (!msg || !msg.tableEdit) return;

  var active = null;
  var originalRaw = "";
  // Direction (next | prev | up | down) to navigate to after the
  // current edit commits. Set by Tab / Shift+Tab / Enter / Shift+Enter
  // and consumed by commitEdit, which fires the tableNavigate message.
  var pendingNavigation = null;

  document.addEventListener("click", function(e) {
    if (!e.target.closest) return;
    var cell = e.target.closest(".rmd-cell");
    if (!cell) return;
    if (active === cell) return;
    if (active) commitEdit(active);
    beginEdit(cell);
  });

  function beginEdit(td) {
    var raw = td.getAttribute("data-raw") || "";
    try { raw = decodeURIComponent(raw); } catch (e) {}
    active = td;
    originalRaw = raw;
    td.dataset.renderedHtml = td.innerHTML;
    td.textContent = raw;
    td.contentEditable = "true";
    td.classList.add("rmd-cell-editing");
    td.focus();
    selectAllInside(td);
  }

  function selectAllInside(el) {
    var range = document.createRange();
    range.selectNodeContents(el);
    var sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  }

  function commitEdit(td) {
    if (!td || td !== active) return;
    var newContent = (td.textContent || "").replace(/ /g, " ");
    var navDirection = pendingNavigation;
    pendingNavigation = null;
    td.contentEditable = "false";
    td.classList.remove("rmd-cell-editing");

    var tableId = td.getAttribute("data-table-id");
    var rowAttr = td.getAttribute("data-row");
    var colAttr = td.getAttribute("data-col");

    if (newContent !== originalRaw) {
      // Content changed → send tableEdit. Rust applies + refreshes.
      msg.tableEdit.postMessage(
        tableId + "\t" + rowAttr + "\t" + colAttr + "\t" + newContent
      );
    } else {
      // No change — restore rendered HTML without a round trip.
      td.innerHTML = td.dataset.renderedHtml || td.innerHTML;
      delete td.dataset.renderedHtml;
    }
    active = null;

    if (navDirection && msg.tableNavigate) {
      // Sequential message — Rust processes after tableEdit (if any)
      // since both run on the main thread in order.
      msg.tableNavigate.postMessage(
        tableId + "\t" + rowAttr + "\t" + colAttr + "\t" + navDirection
      );
    }
  }

  function cancelEdit(td) {
    if (!td || td !== active) return;
    pendingNavigation = null;
    td.contentEditable = "false";
    td.classList.remove("rmd-cell-editing");
    td.innerHTML = td.dataset.renderedHtml || td.textContent;
    delete td.dataset.renderedHtml;
    active = null;
  }

  document.addEventListener("keydown", function(e) {
    if (!active) return;
    if (e.key === "Escape") {
      e.preventDefault();
      cancelEdit(active);
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      // Ctrl/Cmd+Enter → soft break (becomes <br> at commit time).
      document.execCommand("insertText", false, "\n");
      e.preventDefault();
    } else if (e.key === "Enter") {
      e.preventDefault();
      pendingNavigation = e.shiftKey ? "up" : "down";
      commitEdit(active);
    } else if (e.key === "Tab") {
      e.preventDefault();
      pendingNavigation = e.shiftKey ? "prev" : "next";
      commitEdit(active);
    }
  });

  document.addEventListener("focusout", function() {
    if (!active) return;
    var stale = active;
    // Defer so that clicking another cell can hand off cleanly.
    setTimeout(function() {
      if (active === stale && document.activeElement !== stale) {
        commitEdit(stale);
      }
    }, 0);
  });

  // -- Floating toolbar for structural ops -----------------------
  // Shown only while a cell is being edited. Mousedown is suppressed
  // on the buttons so clicking one doesn't blur the cell first (which
  // would commit + hide the toolbar before our handler runs).
  var toolbar = null;
  function buildToolbar() {
    if (toolbar) return toolbar;
    toolbar = document.createElement("div");
    toolbar.className = "rmd-table-toolbar";
    toolbar.style.display = "none";
    var structural = [
      ["row-above", "↱ Row", "Insert row above (Ctrl+Shift+↑)"],
      ["row-below", "↵ Row", "Insert row below (Ctrl+Shift+↓)"],
      ["col-left", "↰ Col", "Insert column left (Ctrl+Shift+←)"],
      ["col-right", "↳ Col", "Insert column right (Ctrl+Shift+→)"],
      ["row-delete", "− Row", "Delete row (Ctrl+Shift+-)"],
      ["col-delete", "− Col", "Delete column (Ctrl+Alt+-)"],
    ];
    structural.forEach(function(s) {
      addToolbarButton(s[0], s[1], s[2], false);
    });
    // Separator between structural and alignment clusters; only
    // visible when the alignment buttons are shown (header cell).
    var sep = document.createElement("div");
    sep.className = "rmd-table-toolbar-sep";
    toolbar.appendChild(sep);
    var align = [
      ["align-left", "L", "Left-align column"],
      ["align-center", "C", "Center-align column"],
      ["align-right", "R", "Right-align column"],
    ];
    align.forEach(function(s) {
      addToolbarButton(s[0], s[1], s[2], true);
    });
    // Sort button — also header-only. Tri-state cycle: off → asc →
    // desc → off. Label/active state is refreshed on every
    // showToolbarForCell based on the active cell's data-sort-dir.
    addToolbarButton(
      "sort",
      "↕ Sort",
      "Sort by this column (cycles asc / desc / off)",
      true,
      "sort"
    );
    // Reformat is always visible (independent of header/body), so
    // give it its own thin divider so it doesn't crowd the L/C/R
    // cluster when those are hidden for body cells.
    var sep2 = document.createElement("div");
    sep2.className = "rmd-table-toolbar-sep rmd-table-toolbar-sep-reformat";
    toolbar.appendChild(sep2);
    addToolbarButton(
      "reformat",
      "↔ Format",
      "Reformat table to pretty alignment (Ctrl+Shift+Alt+T)",
      false
    );
    document.body.appendChild(toolbar);
    return toolbar;
  }
  function addToolbarButton(action, label, title, isHeaderOnly, kind) {
    var btn = document.createElement("button");
    btn.type = "button";
    var classes = ["rmd-table-toolbar-btn"];
    if (isHeaderOnly) classes.push("rmd-table-toolbar-align");
    if (kind === "sort") classes.push("rmd-table-toolbar-sort");
    btn.className = classes.join(" ");
    btn.setAttribute("data-action", action);
    btn.textContent = label;
    btn.title = title;
    // Stop mousedown from stealing focus from the editable cell —
    // otherwise the cell blurs (and commits) before our click runs.
    btn.addEventListener("mousedown", function(e) { e.preventDefault(); });
    btn.addEventListener("click", function(e) {
      e.preventDefault();
      e.stopPropagation();
      if (!active) return;
      var resolvedOp = action;
      if (kind === "sort") {
        // Tri-state cycle: read the cell's current state and pick
        // the next one. Backend handles the actual reordering and
        // the original-order snapshot.
        var current = active.getAttribute("data-sort-dir") || "off";
        var next = current === "off" ? "asc"
                 : current === "asc" ? "desc"
                 : "off";
        resolvedOp = "sort-" + next;
      } else if (action.indexOf("align-") === 0
                 && btn.classList.contains("rmd-table-toolbar-active")) {
        // Clicking the already-active alignment reverts to none.
        resolvedOp = "align-none";
      }
      dispatchStructureOp(resolvedOp);
    });
    toolbar.appendChild(btn);
  }
  function showToolbarForCell(cell) {
    var tb = buildToolbar();
    tb.style.display = "flex";

    // Alignment buttons + separator only show when a HEADER cell is
    // active. The model's alignment lives per-column; we wouldn't
    // know which column to retarget if a body cell were the source.
    var isHeader = cell.getAttribute("data-row") === "-1";
    tb.querySelectorAll(".rmd-table-toolbar-align, .rmd-table-toolbar-sep")
      .forEach(function(el) {
        el.style.display = isHeader ? "" : "none";
      });

    // Highlight the alignment button matching the column's current
    // setting (or none, if the column has no explicit alignment).
    if (isHeader) {
      var currentAlign = cell.getAttribute("data-align") || "none";
      tb.querySelectorAll(".rmd-table-toolbar-align").forEach(function(b) {
        var action = b.getAttribute("data-action") || "";
        if (action.indexOf("align-") !== 0) return; // skip sort
        var op = action.slice("align-".length);
        b.classList.toggle("rmd-table-toolbar-active", op === currentAlign);
      });
      // Tri-state sort indicator: label + active class follow the
      // active cell's data-sort-dir, which the post-processor sets
      // only on the currently-sorted header column.
      var sortBtn = tb.querySelector(".rmd-table-toolbar-sort");
      if (sortBtn) {
        var dir = cell.getAttribute("data-sort-dir") || "off";
        sortBtn.textContent =
          dir === "asc" ? "↑ Sort"
            : dir === "desc" ? "↓ Sort"
            : "↕ Sort";
        sortBtn.classList.toggle("rmd-table-toolbar-active", dir !== "off");
      }
    }

    // Measure after the show/hide so the rect is final.
    var cellRect = cell.getBoundingClientRect();
    var tbRect = tb.getBoundingClientRect();
    var top = Math.max(8, cellRect.top - tbRect.height - 6);
    var left = Math.min(
      Math.max(8, cellRect.right - tbRect.width),
      window.innerWidth - tbRect.width - 8
    );
    tb.style.top = top + "px";
    tb.style.left = left + "px";
  }
  function hideToolbar() {
    if (toolbar) toolbar.style.display = "none";
  }
  function dispatchStructureOp(op) {
    if (!active || !msg.tableStructure) return;
    var tableId = active.getAttribute("data-table-id");
    var rowAttr = active.getAttribute("data-row");
    var colAttr = active.getAttribute("data-col");
    // If there's a pending edit, commit it first so the structural op
    // runs on the latest content. The webkit handler queue preserves
    // order across these two messages.
    var newContent = (active.textContent || "").replace(/ /g, " ");
    if (newContent !== originalRaw) {
      msg.tableEdit.postMessage(
        tableId + "\t" + rowAttr + "\t" + colAttr + "\t" + newContent
      );
    }
    msg.tableStructure.postMessage(
      tableId + "\t" + rowAttr + "\t" + colAttr + "\t" + op
    );
    active.contentEditable = "false";
    active.classList.remove("rmd-cell-editing");
    active = null;
    pendingNavigation = null;
    hideToolbar();
  }

  // Show the toolbar on every beginEdit; hide on commit/cancel.
  var _origBeginEdit = beginEdit;
  beginEdit = function(td) {
    _origBeginEdit(td);
    showToolbarForCell(td);
  };
  var _origCommitEdit = commitEdit;
  commitEdit = function(td) {
    _origCommitEdit(td);
    hideToolbar();
  };
  var _origCancelEdit = cancelEdit;
  cancelEdit = function(td) {
    _origCancelEdit(td);
    hideToolbar();
  };

  // Keyboard shortcuts (active while a cell is being edited).
  document.addEventListener("keydown", function(e) {
    if (!active) return;
    if (!(e.ctrlKey || e.metaKey)) return;
    if (e.shiftKey && e.key === "ArrowDown") {
      e.preventDefault();
      dispatchStructureOp("row-below");
    } else if (e.shiftKey && e.key === "ArrowUp") {
      e.preventDefault();
      dispatchStructureOp("row-above");
    } else if (e.shiftKey && e.key === "ArrowRight") {
      e.preventDefault();
      dispatchStructureOp("col-right");
    } else if (e.shiftKey && e.key === "ArrowLeft") {
      e.preventDefault();
      dispatchStructureOp("col-left");
    } else if (e.shiftKey && (e.key === "-" || e.key === "_")) {
      e.preventDefault();
      dispatchStructureOp("row-delete");
    } else if (e.altKey && (e.key === "-" || e.key === "_")) {
      e.preventDefault();
      dispatchStructureOp("col-delete");
    } else if (e.shiftKey && e.altKey && (e.key === "t" || e.key === "T")) {
      e.preventDefault();
      dispatchStructureOp("reformat");
    }
  });

  // Programmatic focus for one-shot scripts injected by Rust after
  // a Tab/Enter-driven navigation or a structural op. Re-uses the
  // click → beginEdit path so we don't duplicate setup logic.
  window.rmdFocusCell = function(tableId, row, col) {
    var sel = '.rmd-cell[data-table-id="' + tableId + '"]'
            + '[data-row="' + row + '"][data-col="' + col + '"]';
    var cell = document.querySelector(sel);
    if (!cell) return;
    cell.click();
  };
})();
</script>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::parse::parse_tables;

    #[test]
    fn percent_encode_handles_simple_text() {
        assert_eq!(percent_encode("Alice"), "Alice");
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("**bold**"), "%2A%2Abold%2A%2A");
    }

    #[test]
    fn percent_encode_handles_pipe_and_backslash() {
        assert_eq!(percent_encode("foo | bar"), "foo%20%7C%20bar");
        assert_eq!(percent_encode("a\\|b"), "a%5C%7Cb");
    }

    #[test]
    fn inject_no_tables_passes_through_unchanged() {
        let html = "<p>hello <strong>world</strong></p>";
        assert_eq!(inject_table_attrs(html, &[]), html);
    }

    #[test]
    fn inject_adds_data_attrs_to_simple_table() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let tables = parse_tables(src);
        let html = "<table>\n<thead>\n<tr><th>a</th><th>b</th></tr>\n</thead>\n<tbody>\n<tr><td>1</td><td>2</td></tr>\n</tbody>\n</table>\n";
        let out = inject_table_attrs(html, &tables);
        assert!(out.contains(r#"data-table-id="1""#), "got: {out}");
        assert!(out.contains(r#"data-row="-1""#));
        assert!(out.contains(r#"data-row="0""#));
        assert!(out.contains(r#"data-col="0""#));
        assert!(out.contains(r#"data-col="1""#));
        assert!(out.contains(r#"class="rmd-cell""#));
        assert!(out.contains(r#"data-raw="a""#));
        assert!(out.contains(r#"data-raw="1""#));
    }

    #[test]
    fn inject_preserves_cell_inner_html() {
        let src = "| **bold** | x |\n|---|---|\n| a | b |\n";
        let tables = parse_tables(src);
        let html = "<table><thead><tr><th><strong>bold</strong></th><th>x</th></tr></thead><tbody><tr><td>a</td><td>b</td></tr></tbody></table>";
        let out = inject_table_attrs(html, &tables);
        // Inner content untouched.
        assert!(out.contains("<strong>bold</strong>"));
        // Raw markdown source preserved in data-raw.
        assert!(out.contains("%2A%2Abold%2A%2A"));
    }

    #[test]
    fn inject_handles_multiple_tables_with_separate_ids() {
        let src = "| a |\n|---|\n| 1 |\n\n| b |\n|---|\n| 2 |\n";
        let tables = parse_tables(src);
        let html = "<table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table>\n<table><thead><tr><th>b</th></tr></thead><tbody><tr><td>2</td></tr></tbody></table>";
        let out = inject_table_attrs(html, &tables);
        assert!(out.contains(r#"data-table-id="1""#));
        assert!(out.contains(r#"data-table-id="2""#));
        // Each table has its own row indexing.
        let first_table = out.split("</table>").next().unwrap();
        assert!(first_table.contains(r#"data-col="0""#));
    }

    #[test]
    fn inject_uses_percent_encoding_compatible_with_js() {
        let src = "| **a** |\n|---|\n| 1 |\n";
        let tables = parse_tables(src);
        let html =
            "<table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table>";
        let out = inject_table_attrs(html, &tables);
        // The encoded `**a**` is %2A%2Aa%2A%2A — decodes via decodeURIComponent.
        assert!(out.contains("%2A%2Aa%2A%2A"));
    }

    #[test]
    fn inject_does_not_break_when_html_has_more_tables_than_parsed() {
        // Defensive: extra `<table>` HTML the parser didn't see gets
        // no injection but doesn't break the scan for following text.
        let src = "| a |\n|---|\n| 1 |\n";
        let tables = parse_tables(src);
        assert_eq!(tables.len(), 1);
        let html = "<table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table>\n<table><tr><td>raw</td></tr></table>";
        let out = inject_table_attrs(html, &tables);
        // First table gets attrs.
        assert!(out.contains(r#"data-table-id="1""#));
        // The trailing raw table is left alone; no panic, no malformed
        // output, no data-table-id="2".
        assert!(!out.contains(r#"data-table-id="2""#));
        assert!(out.contains("<td>raw</td>"));
    }

    #[test]
    fn injection_emits_data_align_from_separator_alignment() {
        let src = "| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n";
        let tables = parse_tables(src);
        let html = "<table><thead><tr><th>a</th><th>b</th><th>c</th></tr></thead><tbody><tr><td>1</td><td>2</td><td>3</td></tr></tbody></table>";
        let out = inject_table_attrs(html, &tables);
        assert!(out.contains(r#"data-align="left""#), "got: {out}");
        assert!(out.contains(r#"data-align="center""#));
        assert!(out.contains(r#"data-align="right""#));
    }

    #[test]
    fn injection_defaults_data_align_to_none() {
        let src = "| a |\n|---|\n| 1 |\n";
        let tables = parse_tables(src);
        let html =
            "<table><thead><tr><th>a</th></tr></thead><tbody><tr><td>1</td></tr></tbody></table>";
        let out = inject_table_attrs(html, &tables);
        assert!(out.contains(r#"data-align="none""#));
    }

    #[test]
    fn injection_round_trips_through_comrak_style_output() {
        // Smoke: pipe-encoded cells stay correctly encoded.
        let src = "| name | note |\n|------|------|\n| Alice | foo \\| bar |\n";
        let tables = parse_tables(src);
        let html = "<table><thead><tr><th>name</th><th>note</th></tr></thead><tbody><tr><td>Alice</td><td>foo | bar</td></tr></tbody></table>";
        let out = inject_table_attrs(html, &tables);
        // The cell's data-raw should contain the (escaped) form as
        // stored in the parsed model — pulldown-cmark keeps the
        // escaped backslash in the source content.
        // Cell content from the parser is `foo \| bar` (raw).
        assert!(out.contains("data-raw=\"foo%20%5C%7C%20bar\""));
    }
}
