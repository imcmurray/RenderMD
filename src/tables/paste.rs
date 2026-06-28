//! Smart-paste: detect when pasted clipboard content should become a
//! markdown table and convert it.
//!
//! Detection covers four cases in priority order:
//!
//! 1. **HTML table** — from Excel, Sheets, Numbers, browser copy.
//! 2. **GFM table** — already a markdown table; just normalise.
//! 3. **TSV** — tab-separated values (Excel's plain-text format).
//! 4. **CSV** — comma-separated, parsed via the `csv` crate so quoted
//!    commas are handled correctly.
//!
//! Anything else returns [`TablePaste::None`] so the editor's normal
//! paste path runs.

use super::model::*;
use super::serialize::{to_gfm_compact, to_gfm_pretty};

/// Result of inspecting clipboard content. Each variant carries the
/// detected shape (`rows × cols`) so the UI can show a useful toast
/// before committing.
#[derive(Clone, Debug)]
pub enum TablePaste {
    /// HTML containing at least one `<table>`. Convert via
    /// [`html_table_to_gfm`].
    Html {
        html: String,
        rows: usize,
        cols: usize,
    },
    /// Already a GFM markdown table.
    Gfm {
        text: String,
        rows: usize,
        cols: usize,
    },
    /// Tab-separated values.
    Tsv {
        text: String,
        rows: usize,
        cols: usize,
    },
    /// Comma-separated values (RFC 4180).
    Csv {
        text: String,
        rows: usize,
        cols: usize,
    },
    /// Not table-shaped — fall back to normal text paste.
    None,
}

impl TablePaste {
    /// `Some((rows, cols))` for table variants, `None` otherwise.
    pub fn shape(&self) -> Option<(usize, usize)> {
        match self {
            TablePaste::Html { rows, cols, .. }
            | TablePaste::Gfm { rows, cols, .. }
            | TablePaste::Tsv { rows, cols, .. }
            | TablePaste::Csv { rows, cols, .. } => Some((*rows, *cols)),
            TablePaste::None => None,
        }
    }
}

/// Look at clipboard text + HTML and pick the best interpretation.
/// Pass `None` for missing formats.
pub fn detect_table_paste(text: Option<&str>, html: Option<&str>) -> TablePaste {
    if let Some(h) = html {
        if let Some(t) = extract_html_table(h) {
            return t;
        }
    }
    let Some(text) = text else {
        return TablePaste::None;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return TablePaste::None;
    }

    if let Some(t) = looks_like_gfm(trimmed) {
        return t;
    }
    if let Some(t) = looks_like_tsv(trimmed) {
        return t;
    }
    if let Some(t) = looks_like_csv(trimmed) {
        return t;
    }
    TablePaste::None
}

/// Convert a detected paste to clean GFM markdown text. Returns `None`
/// for [`TablePaste::None`] or conversion failures.
pub fn paste_to_gfm(paste: &TablePaste, style: TableStyle) -> Option<String> {
    match paste {
        TablePaste::Html { html, .. } => html_table_to_gfm(html, style),
        TablePaste::Gfm { text, .. } => Some(normalize_gfm(text, style)),
        TablePaste::Tsv { text, .. } => tsv_to_gfm(text, style),
        TablePaste::Csv { text, .. } => csv_to_gfm(text, style),
        TablePaste::None => None,
    }
}

// --- detection ----------------------------------------------------------

fn looks_like_gfm(text: &str) -> Option<TablePaste> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next()?;
    if !header.trim_start().starts_with('|') {
        return None;
    }
    let separator = lines.next()?.trim();
    if !separator.contains('-') {
        return None;
    }
    if !separator
        .bytes()
        .all(|c| matches!(c, b'|' | b'-' | b':' | b' '))
    {
        return None;
    }
    let cols = header.matches('|').count().saturating_sub(1).max(1);
    let rows = text.lines().filter(|l| !l.trim().is_empty()).count();
    Some(TablePaste::Gfm {
        text: text.to_string(),
        rows,
        cols,
    })
}

fn looks_like_tsv(text: &str) -> Option<TablePaste> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return None;
    }
    let cols = lines[0].matches('\t').count() + 1;
    if cols < 2 {
        return None;
    }
    if !lines.iter().all(|l| l.matches('\t').count() + 1 == cols) {
        return None;
    }
    Some(TablePaste::Tsv {
        text: text.to_string(),
        rows: lines.len(),
        cols,
    })
}

fn looks_like_csv(text: &str) -> Option<TablePaste> {
    if !text.contains(',') {
        return None;
    }
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false) // require consistent column count
        .from_reader(text.as_bytes());

    let mut record_count = 0usize;
    let mut col_count = 0usize;
    for result in rdr.records() {
        let rec = match result {
            Ok(r) => r,
            Err(_) => return None,
        };
        if record_count == 0 {
            col_count = rec.len();
        } else if rec.len() != col_count {
            return None;
        }
        record_count += 1;
    }
    if record_count < 2 || col_count < 2 {
        return None;
    }
    Some(TablePaste::Csv {
        text: text.to_string(),
        rows: record_count,
        cols: col_count,
    })
}

/// Find the first `<table>...</table>` block and shape it. Permissive:
/// counts the first `<tr>`'s cells and the number of `<tr>` elements.
/// The actual conversion is in [`html_table_to_gfm`].
fn extract_html_table(html: &str) -> Option<TablePaste> {
    let lc = html.to_lowercase();
    let start = lc.find("<table")?;
    let end_tag = lc[start..].find("</table>")?;
    let end = start + end_tag + "</table>".len();
    let block = &html[start..end];

    let rows = count_tags(block, "<tr");
    if rows < 1 {
        return None;
    }
    let first_tr_start = lc[start..].find("<tr")? + start;
    let first_tr_end_rel = lc[first_tr_start..].find("</tr>")?;
    let first_tr = &html[first_tr_start..first_tr_start + first_tr_end_rel];
    let cols = count_tags(first_tr, "<td").max(count_tags(first_tr, "<th"));
    if cols < 1 {
        return None;
    }

    Some(TablePaste::Html {
        html: block.to_string(),
        rows,
        cols,
    })
}

fn count_tags(haystack: &str, needle: &str) -> usize {
    haystack.to_lowercase().matches(needle).count()
}

// --- conversion ---------------------------------------------------------

fn tsv_to_gfm(text: &str, style: TableStyle) -> Option<String> {
    let rows: Vec<Vec<String>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').map(|c| c.to_string()).collect())
        .collect();
    rows_to_gfm(rows, style)
}

fn csv_to_gfm(text: &str, style: TableStyle) -> Option<String> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(text.as_bytes());
    let rows: std::result::Result<Vec<Vec<String>>, csv::Error> = rdr
        .records()
        .map(|r| r.map(|rec| rec.iter().map(String::from).collect()))
        .collect();
    rows_to_gfm(rows.ok()?, style)
}

/// Convert an HTML `<table>` block to GFM. Light-touch parsing — walks
/// the bytes finding `<tr>`/`<td>`/`<th>` boundaries and extracts cell
/// text. Preserves a handful of inline tags as markdown (`<b>`, `<i>`,
/// `<code>`, `<a>`, `<br>`); everything else is flattened to text.
pub fn html_table_to_gfm(html: &str, style: TableStyle) -> Option<String> {
    let rows = parse_html_table_rows(html)?;
    if rows.is_empty() {
        return None;
    }
    rows_to_gfm(rows, style)
}

fn parse_html_table_rows(html: &str) -> Option<Vec<Vec<String>>> {
    let lc = html.to_lowercase();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut search_from = 0usize;

    while let Some(tr_pos) = lc[search_from..].find("<tr") {
        let tr_start = search_from + tr_pos;
        let after_open = lc[tr_start..].find('>')? + tr_start + 1;
        let close_rel = lc[after_open..].find("</tr>")?;
        let tr_inner = &html[after_open..after_open + close_rel];
        rows.push(parse_html_row_cells(tr_inner));
        search_from = after_open + close_rel + "</tr>".len();
    }
    if rows.is_empty() {
        return None;
    }
    Some(rows)
}

fn parse_html_row_cells(tr_inner: &str) -> Vec<String> {
    let lc = tr_inner.to_lowercase();
    let mut cells: Vec<String> = Vec::new();
    let mut search_from = 0usize;
    while let Some(pos) = lc[search_from..].find("<t") {
        let tag_start = search_from + pos;
        // Accept <td or <th; skip anything else (e.g. <textarea —
        // unlikely inside a table but be defensive).
        let after_two = match lc.get(tag_start + 2..tag_start + 3) {
            Some(s) => s,
            None => break,
        };
        if after_two != "d" && after_two != "h" {
            search_from = tag_start + 2;
            continue;
        }
        let close_tag = if after_two == "d" { "</td>" } else { "</th>" };
        let after_open = match lc[tag_start..].find('>') {
            Some(p) => p + tag_start + 1,
            None => break,
        };
        let close_rel = match lc[after_open..].find(close_tag) {
            Some(p) => p,
            None => break,
        };
        let cell_inner = &tr_inner[after_open..after_open + close_rel];
        cells.push(html_cell_to_markdown(cell_inner));
        search_from = after_open + close_rel + close_tag.len();
    }
    cells
}

/// Extract markdown text from a `<td>` / `<th>` inner HTML. Preserves
/// a small whitelist of inline tags; flattens everything else.
fn html_cell_to_markdown(cell_inner: &str) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    let bytes = cell_inner.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // Find the end of the tag.
            let Some(tag_end_rel) = cell_inner[i..].find('>') else {
                break;
            };
            let raw_tag = &cell_inner[i..i + tag_end_rel + 1];
            let lower_tag = raw_tag.to_lowercase();
            let (name, is_close) = tag_name(&lower_tag);
            i += tag_end_rel + 1;
            match (name.as_str(), is_close) {
                ("br", _) => out.push_str("<br>"),
                ("b", false) | ("strong", false) => out.push_str("**"),
                ("b", true) | ("strong", true) => out.push_str("**"),
                ("i", false) | ("em", false) => out.push('*'),
                ("i", true) | ("em", true) => out.push('*'),
                ("code", false) => out.push('`'),
                ("code", true) => out.push('`'),
                ("a", false) => {
                    // Buffer text until </a>, then emit [text](href).
                    let href = extract_attr(raw_tag, "href").unwrap_or_default();
                    let close_pos = cell_inner[i..].to_lowercase().find("</a>");
                    if let Some(rel) = close_pos {
                        let inner = &cell_inner[i..i + rel];
                        let plain = strip_inline_tags(inner);
                        out.push_str(&format!("[{}]({})", plain, href));
                        i += rel + "</a>".len();
                    }
                }
                _ => { /* drop unknown tags */ }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    decode_entities(&out).trim().to_string()
}

fn tag_name(raw: &str) -> (String, bool) {
    let inner = raw.trim_start_matches('<').trim_end_matches('>');
    let inner = inner.trim_start_matches('/');
    let name: String = inner
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    (name, raw.starts_with("</"))
}

fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lc = tag.to_lowercase();
    let needle = format!("{name}=\"");
    if let Some(p) = lc.find(&needle) {
        let start = p + needle.len();
        let end_rel = lc[start..].find('"')?;
        return Some(tag[start..start + end_rel].to_string());
    }
    let needle = format!("{name}='");
    if let Some(p) = lc.find(&needle) {
        let start = p + needle.len();
        let end_rel = lc[start..].find('\'')?;
        return Some(tag[start..start + end_rel].to_string());
    }
    None
}

fn strip_inline_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Normalise a GFM table that's already in the clipboard: re-parse and
/// re-serialise to the requested style. Pretty / Compact reformat the
/// table; PreserveOriginal returns the input unchanged.
fn normalize_gfm(text: &str, style: TableStyle) -> String {
    if matches!(style, TableStyle::PreserveOriginal) {
        return text.trim_end().to_string() + "\n";
    }
    let mut tables = super::parse::parse_tables(text);
    let Some(t) = tables.first_mut() else {
        return text.to_string();
    };
    t.style = style;
    match style {
        TableStyle::Pretty => to_gfm_pretty(t),
        TableStyle::Compact => to_gfm_compact(t),
        TableStyle::PreserveOriginal => unreachable!(),
    }
}

/// Build a [`MarkdownTable`] from `rows` (first row = header) and
/// serialise it to GFM source. Pads short rows with empties.
fn rows_to_gfm(rows: Vec<Vec<String>>, style: TableStyle) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let cols = rows.iter().map(|r| r.len()).max()?;
    if cols == 0 {
        return None;
    }

    let pad = |mut r: Vec<String>| -> Vec<String> {
        while r.len() < cols {
            r.push(String::new());
        }
        r
    };
    let mut rows: Vec<Vec<String>> = rows.into_iter().map(pad).collect();
    let header_row = rows.remove(0);

    let alignments = vec![Alignment::None; cols];

    let make_cell = |c: &str| Cell {
        source_range: 0..0,
        content: c.to_string(),
        leading_ws: 1,
        trailing_ws: 1,
    };

    let mut table = MarkdownTable {
        id: 0,
        source_range: 0..0,
        alignments,
        headers: header_row.iter().map(|c| make_cell(c)).collect(),
        rows: rows
            .iter()
            .map(|r| r.iter().map(|c| make_cell(c)).collect())
            .collect(),
        style,
        column_widths: vec![None; cols],
        original_lines: None,
        sort_indicator: None,
    };

    Some(match style {
        TableStyle::Pretty => to_gfm_pretty(&table),
        TableStyle::Compact => to_gfm_compact(&table),
        TableStyle::PreserveOriginal => {
            // PreserveOriginal needs original_lines we don't have here;
            // fall back to Pretty for fresh constructions.
            table.style = TableStyle::Pretty;
            to_gfm_pretty(&table)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_tsv() {
        let text = "name\tscore\nAlice\t42\nBob\t99\n";
        match detect_table_paste(Some(text), None) {
            TablePaste::Tsv { rows, cols, .. } => {
                assert_eq!(rows, 3);
                assert_eq!(cols, 2);
            }
            other => panic!("expected TSV, got {other:?}"),
        }
    }

    #[test]
    fn detects_csv() {
        let text = "name,score\nAlice,42\nBob,99\n";
        match detect_table_paste(Some(text), None) {
            TablePaste::Csv { rows, cols, .. } => {
                assert_eq!(rows, 3);
                assert_eq!(cols, 2);
            }
            other => panic!("expected CSV, got {other:?}"),
        }
    }

    #[test]
    fn detects_csv_with_quoted_commas() {
        let text = "name,bio\nAlice,\"a, the first\"\nBob,b\n";
        let detected = detect_table_paste(Some(text), None);
        assert!(matches!(detected, TablePaste::Csv { .. }));
        let md = paste_to_gfm(&detected, TableStyle::Pretty).unwrap();
        assert!(md.contains("a, the first"));
    }

    #[test]
    fn detects_gfm() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let detected = detect_table_paste(Some(text), None);
        assert!(matches!(detected, TablePaste::Gfm { .. }));
    }

    #[test]
    fn detects_html_table_from_excel_style_paste() {
        let html = r#"<html><body><table border="0"><tr><td>a</td><td>b</td></tr><tr><td>1</td><td>2</td></tr></table></body></html>"#;
        let detected = detect_table_paste(None, Some(html));
        match detected {
            TablePaste::Html { rows, cols, .. } => {
                assert_eq!(rows, 2);
                assert_eq!(cols, 2);
            }
            other => panic!("expected HTML, got {other:?}"),
        }
    }

    #[test]
    fn html_overrides_tsv_when_both_present() {
        let html = "<table><tr><td>X</td></tr></table>";
        let text = "ignored\ttext\nfallback\trow\n";
        let detected = detect_table_paste(Some(text), Some(html));
        assert!(matches!(detected, TablePaste::Html { .. }));
    }

    #[test]
    fn plain_prose_returns_none() {
        let text = "this is just a paragraph of text\nwith multiple lines\nbut no table\n";
        let detected = detect_table_paste(Some(text), None);
        assert!(matches!(detected, TablePaste::None));
    }

    #[test]
    fn ragged_tsv_returns_none() {
        let text = "a\tb\tc\n1\t2\n3\t4\t5\n";
        assert!(matches!(
            detect_table_paste(Some(text), None),
            TablePaste::None
        ));
    }

    #[test]
    fn tsv_to_gfm_produces_valid_table() {
        let text = "name\tscore\nAlice\t42\nBob\t99\n";
        let detected = detect_table_paste(Some(text), None);
        let md = paste_to_gfm(&detected, TableStyle::Pretty).unwrap();
        let parsed = super::super::parse::parse_tables(&md);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].rows.len(), 2);
        assert_eq!(parsed[0].alignments.len(), 2);
    }

    #[test]
    fn csv_with_pipe_in_cell_escapes_on_conversion() {
        let text = "name,note\nAlice,\"pipes | inside\"\n";
        let detected = detect_table_paste(Some(text), None);
        let md = paste_to_gfm(&detected, TableStyle::Pretty).unwrap();
        assert!(md.contains("pipes \\| inside"));
    }

    #[test]
    fn html_cell_inline_markup_preserved() {
        let html = "<table><tr><th>name</th></tr><tr><td><b>Alice</b></td></tr></table>";
        let detected = detect_table_paste(None, Some(html));
        let md = paste_to_gfm(&detected, TableStyle::Pretty).unwrap();
        assert!(md.contains("**Alice**"), "got: {md}");
    }

    #[test]
    fn html_cell_link_preserved() {
        let html = r#"<table><tr><th>name</th></tr><tr><td><a href="https://x.example">x</a></td></tr></table>"#;
        let detected = detect_table_paste(None, Some(html));
        let md = paste_to_gfm(&detected, TableStyle::Pretty).unwrap();
        assert!(md.contains("[x](https://x.example)"), "got: {md}");
    }

    #[test]
    fn pretty_paste_output_has_aligned_columns() {
        let text = "short\tlong-header\nx\tyyyyyyyyy\n";
        let detected = detect_table_paste(Some(text), None);
        let md = paste_to_gfm(&detected, TableStyle::Pretty).unwrap();
        let lines: Vec<&str> = md.lines().collect();
        let header_pipes: Vec<usize> = lines[0].match_indices('|').map(|(i, _)| i).collect();
        let body_pipes: Vec<usize> = lines[2].match_indices('|').map(|(i, _)| i).collect();
        assert_eq!(header_pipes, body_pipes, "{md}");
    }
}
