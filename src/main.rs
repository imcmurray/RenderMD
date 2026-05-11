// RenderMD — a native GTK4 Markdown viewer/editor.
// Renders Markdown by default and toggles to a GtkSourceView 5 editor with
// F5 / Ctrl+Shift+E. Single-file Rust port of the original Python prototype.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use adw::prelude::*;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{markdown_to_html_with_plugins, ComrakOptions, ComrakPlugins};
use gtk::glib::{self, clone};
use gtk::{gdk, gio, pango};
use notify::Watcher;
use sourceview5::prelude::*;
use webkit6::prelude::*;

// Table subsystem — parsed/edited independently of the main comrak
// render path. See src/tables/mod.rs.
mod tables;

const APP_ID: &str = "io.github.rendermd.RenderMD";
const APP_NAME: &str = "RenderMD";

const MODE_PREVIEW: &str = "preview";
const MODE_EDIT: &str = "edit";

// ---- Preview CSS ------------------------------------------------------------
const PREVIEW_CSS_LIGHT: &str = r#"
:root {
  --bg: #ffffff;
  --fg: #1f2328;
  --muted: #59636e;
  --accent: #0969da;
  --border: #d1d9e0;
  --code-bg: #f6f8fa;
  --code-fg: #1f2328;
  --kbd-bg: #f6f8fa;
  --table-stripe: #f6f8fa;
  --quote-bg: #f6f8fa;
  --quote-bar: #d1d9e0;
  --alert-note: #0969da;
  --alert-tip: #1a7f37;
  --alert-important: #8250df;
  --alert-warning: #9a6700;
  --alert-caution: #cf222e;
  --change: #d4a017;
}
"#;

const PREVIEW_CSS_DARK: &str = r#"
:root {
  --bg: #1e1e2e;
  --fg: #e6edf3;
  --muted: #9da7b1;
  --accent: #79b8ff;
  --border: #30363d;
  --code-bg: #161b22;
  --code-fg: #e6edf3;
  --kbd-bg: #161b22;
  --table-stripe: #161b22;
  --quote-bg: #161b22;
  --quote-bar: #30363d;
  --alert-note: #79b8ff;
  --alert-tip: #3fb950;
  --alert-important: #a371f7;
  --alert-warning: #d29922;
  --alert-caution: #f85149;
  --change: #e3b341;
}
"#;

const PREVIEW_CSS_BASE: &str = r#"
* { box-sizing: border-box; }
html, body {
  margin: 0;
  padding: 0;
  background: var(--bg);
  color: var(--fg);
}
body {
  font-family: -apple-system, "Inter", "Cantarell", "Noto Sans",
               "Segoe UI", system-ui, sans-serif;
  font-size: 16px;
  line-height: 1.65;
  padding: 48px max(48px, 8vw) 96px;
  max-width: 980px;
  margin: 0 auto;
  word-wrap: break-word;
}
h1, h2, h3, h4, h5, h6 {
  font-weight: 600;
  line-height: 1.25;
  margin-top: 1.6em;
  margin-bottom: 0.6em;
}
h1 { font-size: 2.1em; padding-bottom: 0.3em; border-bottom: 1px solid var(--border); }
h2 { font-size: 1.55em; padding-bottom: 0.3em; border-bottom: 1px solid var(--border); }
h3 { font-size: 1.25em; }
h4 { font-size: 1.05em; }
h5 { font-size: 0.95em; }
h6 { font-size: 0.88em; color: var(--muted); }
p, ul, ol, blockquote, pre, table { margin: 0 0 1em 0; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
strong { font-weight: 600; }
hr {
  border: 0;
  border-top: 1px solid var(--border);
  margin: 2em 0;
}
ul, ol { padding-left: 2em; }
li + li { margin-top: 0.25em; }
li > p { margin: 0.5em 0; }
ul.contains-task-list { list-style: none; padding-left: 1em; }
ul.contains-task-list li.task-list-item { position: relative; padding-left: 0.25em; }
input.task-list-item-checkbox { margin-right: 0.5em; }

blockquote {
  margin: 1em 0;
  padding: 0.5em 1em;
  background: var(--quote-bg);
  border-left: 4px solid var(--quote-bar);
  color: var(--muted);
  border-radius: 4px;
}
blockquote > :last-child { margin-bottom: 0; }

code, kbd, samp, pre {
  font-family: "JetBrains Mono", "Fira Code", "Source Code Pro",
               "Cascadia Code", monospace;
  font-size: 0.92em;
}
:not(pre) > code {
  background: var(--code-bg);
  color: var(--code-fg);
  padding: 0.18em 0.42em;
  border-radius: 6px;
  border: 1px solid var(--border);
}
pre {
  background: var(--code-bg);
  color: var(--code-fg);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 14px 18px;
  overflow-x: auto;
  line-height: 1.55;
}
pre code { background: none; border: 0; padding: 0; }

kbd {
  background: var(--kbd-bg);
  border: 1px solid var(--border);
  border-bottom-width: 2px;
  border-radius: 6px;
  padding: 0.1em 0.5em;
  font-size: 0.85em;
}

table {
  border-collapse: collapse;
  width: 100%;
  display: block;
  overflow-x: auto;
}
table th, table td {
  border: 1px solid var(--border);
  padding: 8px 14px;
  text-align: left;
}
/* Column alignment from the GFM separator row. The data-align
   attribute is set by the post-processor; the attribute-selector
   specificity outranks the bare `table td` rule above so the
   left default doesn't fight column-specific alignment. */
table th[data-align="center"], table td[data-align="center"] { text-align: center; }
table th[data-align="right"], table td[data-align="right"] { text-align: right; }
table tr:nth-child(2n) { background: var(--table-stripe); }
table th { font-weight: 600; background: var(--table-stripe); }

img {
  max-width: 100%;
  height: auto;
  border-radius: 6px;
}

.alert {
  border-left: 4px solid;
  padding: 8px 16px;
  margin: 1em 0;
  border-radius: 0 6px 6px 0;
  background: var(--code-bg);
}
.alert-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-weight: 600;
  margin-bottom: 4px;
}
.alert-title svg { width: 16px; height: 16px; flex-shrink: 0; }
.alert > p:first-of-type { margin-top: 0; }
.alert > p:last-of-type { margin-bottom: 0; }
.alert-note { border-color: var(--alert-note); }
.alert-note .alert-title { color: var(--alert-note); }
.alert-tip { border-color: var(--alert-tip); }
.alert-tip .alert-title { color: var(--alert-tip); }
.alert-important { border-color: var(--alert-important); }
.alert-important .alert-title { color: var(--alert-important); }
.alert-warning { border-color: var(--alert-warning); }
.alert-warning .alert-title { color: var(--alert-warning); }
.alert-caution { border-color: var(--alert-caution); }
.alert-caution .alert-title { color: var(--alert-caution); }
.rmd-changed-marker { display: block; height: 0; margin: 0; padding: 0; }
.rmd-changed-marker + * {
  border-left: 3px solid var(--change);
  padding-left: 0.75em;
  margin-left: -1em;
  cursor: default;
}
.rmd-changed-marker + .rmd-showing-prev {
  background: rgba(227, 179, 65, 0.06);
}
.rmd-prev-empty { color: var(--muted); }
/* Swap content needs to wrap so long lines stay visible — the
 * mouseleave revert means the user can't scroll horizontally. */
.rmd-showing-prev,
.rmd-showing-prev pre,
.rmd-showing-prev code {
  white-space: pre-wrap;
  word-break: break-word;
  overflow-wrap: anywhere;
  overflow-x: hidden;
}
.rmd-img-wrapper {
  position: relative;
  display: inline-block;
  line-height: 0;
  max-width: 100%;
}
.rmd-img-wrapper img { display: block; max-width: 100%; height: auto; }
.rmd-img-wrapper:hover { outline: 1px dashed var(--accent); outline-offset: 1px; }
.rmd-img-handle {
  position: absolute;
  width: 12px;
  height: 12px;
  background: var(--bg);
  border: 2px solid var(--accent);
  border-radius: 2px;
  opacity: 0;
  transition: opacity 0.12s;
  z-index: 5;
}
.rmd-img-wrapper:hover .rmd-img-handle { opacity: 1; }
.rmd-img-handle-nw { top: -7px; left: -7px; cursor: nwse-resize; }
.rmd-img-handle-ne { top: -7px; right: -7px; cursor: nesw-resize; }
.rmd-img-handle-sw { bottom: -7px; left: -7px; cursor: nesw-resize; }
.rmd-img-handle-se { bottom: -7px; right: -7px; cursor: nwse-resize; }
.rmd-img-dragging { opacity: 0.55; }
.rmd-img-dragging .rmd-img-handle { display: none; }
.rmd-img-dragging:hover { outline: 2px solid var(--accent); }
.rmd-cell { cursor: text; transition: background 0.1s; }
.rmd-cell:hover { background: rgba(127, 127, 127, 0.08); }
.rmd-cell-editing {
  outline: 2px solid var(--accent);
  outline-offset: -2px;
  background: var(--bg);
  white-space: pre-wrap;
  word-break: break-word;
  cursor: text;
}
.rmd-cell-editing:focus { outline-color: var(--accent); }
.rmd-table-toolbar {
  position: fixed;
  z-index: 60;
  display: none;
  flex-direction: row;
  gap: 2px;
  padding: 4px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18);
  font-size: 0.8em;
  user-select: none;
}
.rmd-table-toolbar-btn {
  background: transparent;
  border: none;
  color: var(--fg);
  padding: 4px 8px;
  border-radius: 4px;
  cursor: pointer;
  font: inherit;
  white-space: nowrap;
}
.rmd-table-toolbar-btn:hover {
  background: rgba(127, 127, 127, 0.14);
  color: var(--accent);
}
.rmd-table-toolbar-btn.rmd-table-toolbar-active {
  background: var(--accent);
  color: var(--bg);
}
.rmd-table-toolbar-btn.rmd-table-toolbar-active:hover {
  background: var(--accent);
  color: var(--bg);
  filter: brightness(1.1);
}
.rmd-table-toolbar-sep {
  width: 1px;
  align-self: stretch;
  background: var(--border);
  margin: 2px 4px;
}
.rmd-table-fixed {
  table-layout: fixed;
}
.rmd-table-fixed th, .rmd-table-fixed td {
  overflow: hidden;
  word-break: break-word;
}
th.rmd-cell { position: relative; }
.rmd-th-resize-handle {
  position: absolute;
  top: 0;
  right: -3px;
  width: 6px;
  height: 100%;
  cursor: col-resize;
  z-index: 3;
  user-select: none;
}
.rmd-th-resize-handle:hover,
.rmd-th-resize-handle.rmd-resizing {
  background: var(--accent);
  opacity: 0.4;
}
.rmd-resizing-table, .rmd-resizing-table * {
  cursor: col-resize !important;
  user-select: none !important;
}
.rmd-history-rail {
  position: fixed;
  left: 8px;
  top: 24px;
  bottom: 24px;
  width: 22px;
  display: flex;
  flex-direction: column;
  align-items: center;
  z-index: 50;
  overflow-y: auto;
  scrollbar-width: thin;
}
.rmd-history-track {
  position: absolute;
  top: 8px;
  bottom: 8px;
  left: 50%;
  transform: translateX(-50%);
  width: 1px;
  background: var(--border);
  pointer-events: none;
}
.rmd-history-circle {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--muted);
  border: 1px solid var(--bg);
  margin: 5px 0;
  padding: 0;
  cursor: pointer;
  position: relative;
  z-index: 1;
  transition: transform 0.12s ease, background 0.12s ease, box-shadow 0.12s ease;
  flex-shrink: 0;
}
.rmd-history-circle:hover {
  transform: scale(1.5);
  background: var(--accent);
  box-shadow: 0 0 6px var(--accent);
}
.rmd-history-circle.rmd-history-active {
  background: var(--accent);
  box-shadow: 0 0 8px var(--accent);
}
.rmd-history-hint {
  position: fixed;
  left: 14px;
  top: 32px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--muted);
  opacity: 0.4;
  z-index: 50;
  cursor: pointer;
  transition: opacity 0.12s, transform 0.12s, background 0.12s;
}
.rmd-history-hint:hover {
  opacity: 0.9;
  transform: scale(1.6);
  background: var(--accent);
}
.rmd-minimap {
  position: fixed;
  right: 0;
  top: 8px;
  bottom: 8px;
  width: 14px;
  z-index: 60;
  pointer-events: auto;
}
.rmd-minimap-tick {
  position: absolute;
  left: 3px;
  right: 3px;
  height: 4px;
  background: var(--change);
  border-radius: 2px;
  cursor: pointer;
  opacity: 0.55;
  margin-top: -2px;
  transition: opacity 0.12s ease, transform 0.12s ease;
}
.rmd-minimap-tick:hover {
  opacity: 1;
  transform: scaleX(1.7);
}
.rmd-prev-banner {
  font-size: 0.68em;
  font-weight: 500;
  font-style: italic;
  color: var(--muted);
  margin: 0.5em 0 0 0;
  padding-top: 0.25em;
  border-top: 1px solid var(--border);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.rmd-diff {
  font-family: "JetBrains Mono", "Fira Code", "Cascadia Mono",
               "DejaVu Sans Mono", ui-monospace, monospace;
  font-size: 0.92em;
  line-height: 1.5;
  margin: 0.25em 0;
}
.rmd-diff-line {
  display: flex;
  white-space: pre-wrap;
  padding: 0 0.25em;
  border-radius: 2px;
}
.rmd-diff-mark {
  flex: 0 0 1.4em;
  text-align: center;
  font-weight: 600;
  user-select: none;
  opacity: 0.85;
}
.rmd-diff-text { flex: 1 1 auto; }
.rmd-diff-add { background: rgba(31, 136, 61, 0.14); color: var(--alert-tip); }
.rmd-diff-del { background: rgba(207, 34, 46, 0.14); color: var(--alert-caution); }
.rmd-diff-mod { background: rgba(227, 179, 65, 0.08); }
.rmd-diff-eq  { color: var(--muted); }
.rmd-diff-w-del {
  background: rgba(207, 34, 46, 0.22);
  color: var(--alert-caution);
  text-decoration: line-through;
  border-radius: 2px;
  padding: 0 2px;
}
.rmd-diff-w-add {
  background: rgba(31, 136, 61, 0.22);
  color: var(--alert-tip);
  text-decoration: none;
  border-radius: 2px;
  padding: 0 2px;
}
"#;

const HTML_TEMPLATE: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>{TITLE}</title>
<style>
{THEME_CSS}
{BASE_CSS}
</style>
<base href="{BASE_HREF}">
</head>
<body>
{BODY}
{MERMAID_SCRIPT}
{IMAGE_CLICK_JS}
</body>
</html>
"#;

// Image interactions in the rendered preview:
//   - Hover an image: 4 corner handles fade in.
//   - Drag a corner: live-resize, post `imageResize` (src\twidth\talt) on release.
//   - Mousedown on the image body: tracks for click vs drag.
//       * Released without moving: post `imageClick` to open the options dialog.
//       * Moved past a small threshold: drag-to-move; an insertion line
//         tracks the cursor's nearest gap between top-level blocks. On
//         release, post `imageMove` (src\ttargetIndex).
// All no-ops outside the WebView (no webkit handler), so this is safe to
// embed unconditionally — exported HTML files just see static images.
const IMAGE_CLICK_JS: &str = r#"<script>
(function() {
  var msg = (window.webkit && window.webkit.messageHandlers) || null;
  if (!msg || (!msg.imageClick && !msg.imageResize && !msg.imageMove)) return;
  function send(name, payload) {
    if (msg[name]) msg[name].postMessage(payload);
  }
  function setupImage(img) {
    if (img.dataset.rmdSetup) return;
    img.dataset.rmdSetup = "1";
    var wrap = document.createElement("span");
    wrap.className = "rmd-img-wrapper";
    img.parentNode.insertBefore(wrap, img);
    wrap.appendChild(img);
    ["nw","ne","sw","se"].forEach(function(corner) {
      var h = document.createElement("span");
      h.className = "rmd-img-handle rmd-img-handle-" + corner;
      wrap.appendChild(h);
      h.addEventListener("mousedown", function(e) {
        e.preventDefault();
        e.stopPropagation();
        startResize(e, img, corner);
      });
    });
    img.addEventListener("mousedown", function(e) {
      if (e.button !== 0) return;
      if (e.target !== img) return;
      e.preventDefault();
      startMaybeMove(e, img);
    });
    img.addEventListener("dragstart", function(e) { e.preventDefault(); });
  }
  function startResize(e, img, corner) {
    var startX = e.clientX;
    var startW = img.offsetWidth || img.naturalWidth || 200;
    var startH = img.offsetHeight || img.naturalHeight || 200;
    var aspect = startW / Math.max(1, startH);
    function onMove(ev) {
      var dx = ev.clientX - startX;
      var sign = (corner === "ne" || corner === "se") ? 1 : -1;
      var newW = Math.max(40, Math.round(startW + sign * dx));
      img.style.width = newW + "px";
      img.style.height = Math.round(newW / aspect) + "px";
    }
    function onUp() {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      var finalW = Math.round(img.offsetWidth);
      var src = img.getAttribute("src") || "";
      var alt = img.getAttribute("alt") || "";
      send("imageResize", src + "\t" + finalW + "\t" + alt);
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }
  // Drop targets are individual image wrappers in document order. This
  // way the user can drop "between" two images regardless of whether
  // they're each in their own paragraph (stacked) or side-by-side
  // inline within one paragraph.
  function dropTargets(dragWrapper) {
    return Array.from(document.querySelectorAll(".rmd-img-wrapper"))
      .filter(function(w) { return w !== dragWrapper; });
  }
  // "Before" a wrapper: cursor is above it (block layout) OR on the
  // same line and to the left of its midpoint (inline layout).
  function isBefore(x, y, rect) {
    if (y < rect.top) return true;
    if (y > rect.bottom) return false;
    return x < rect.left + rect.width / 2;
  }
  function dropIndexFor(wrapper, x, y) {
    var targets = dropTargets(wrapper);
    for (var i = 0; i < targets.length; i++) {
      if (isBefore(x, y, targets[i].getBoundingClientRect())) return i;
    }
    return targets.length;
  }
  // Make-room animation: as the ghost approaches a gap, blocks at and
  // after the candidate drop position smoothly translate downward by
  // the dragged image's height. The user sees the existing images
  // physically slide aside, previewing the post-drop layout.
  var spacedBlocks = [];
  function applySpacing(wrapper, idx, height) {
    var blocks = dropTargets(wrapper);
    var newSpaced = blocks.slice(idx);
    // Reset any block that's no longer in the "spaced" set.
    spacedBlocks.forEach(function(el) {
      if (newSpaced.indexOf(el) === -1) {
        el.style.transform = "";
      }
    });
    // Apply to the new set. transition is set once and left in place
    // for the duration of the drag.
    newSpaced.forEach(function(el) {
      if (el.dataset.rmdSpaced !== "1") {
        el.style.transition = "transform 0.18s ease";
        el.dataset.rmdSpaced = "1";
      }
      el.style.transform = "translateY(" + height + "px)";
    });
    spacedBlocks = newSpaced;
  }
  function clearSpacing() {
    spacedBlocks.forEach(function(el) {
      el.style.transform = "";
      el.style.transition = "";
      delete el.dataset.rmdSpaced;
    });
    spacedBlocks = [];
  }

  function startMaybeMove(e, img) {
    var wrapper = img.parentElement;
    var startX = e.clientX, startY = e.clientY;
    var rect = wrapper.getBoundingClientRect();
    var grabX = startX - rect.left;
    var grabY = startY - rect.top;
    var lockedWidth = Math.min(rect.width, 360);
    var lockedHeight = Math.round(
      (lockedWidth / Math.max(1, rect.width)) * rect.height
    );
    var moved = false;
    var lastIdx = -1;
    function onMove(ev) {
      var dx = ev.clientX - startX, dy = ev.clientY - startY;
      if (!moved && (Math.abs(dx) > 2 || Math.abs(dy) > 2)) {
        moved = true;
        wrapper.classList.add("rmd-img-dragging");
        // Float the wrapper at cursor position so the cursor smoothly
        // carries the image around.
        wrapper.style.position = "fixed";
        wrapper.style.zIndex = "1000";
        wrapper.style.pointerEvents = "none";
        wrapper.style.width = lockedWidth + "px";
        wrapper.style.height = "auto";
        wrapper.style.margin = "0";
      }
      if (moved) {
        wrapper.style.left = (ev.clientX - grabX) + "px";
        wrapper.style.top = (ev.clientY - grabY) + "px";
        var idx = dropIndexFor(wrapper, ev.clientX, ev.clientY);
        if (idx !== lastIdx) {
          applySpacing(wrapper, idx, lockedHeight);
          lastIdx = idx;
        }
      }
    }
    function onUp(ev) {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      clearSpacing();
      var src = img.getAttribute("src") || "";
      if (!moved) {
        var width = img.getAttribute("width") || "";
        var alt = img.getAttribute("alt") || "";
        send("imageClick", src + "\t" + width + "\t" + alt);
        return;
      }
      // Identify the target image by its src so Rust can locate it in
      // the buffer regardless of paragraph/inline structure. Empty
      // targetSrc means "append at end".
      var targets = dropTargets(wrapper);
      var targetSrc = "";
      if (lastIdx >= 0 && lastIdx < targets.length) {
        var tgtImg = targets[lastIdx].querySelector("img");
        targetSrc = (tgtImg && tgtImg.getAttribute("src")) || "";
      }
      send("imageMove", src + "\t" + targetSrc);
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }
  document.querySelectorAll("img").forEach(setupImage);
})();
</script>
"#;

// Mermaid.js UMD bundle, embedded so preview works offline. Only injected
// into the page when the document actually contains a mermaid fence.
const MERMAID_BUNDLE: &str = include_str!("../data/js/mermaid.min.js");

const MERMAID_INIT_JS: &str = r#"
(function() {
  // Source blocks come through as <pre class="mermaid">...</pre> (a CommonMark
  // type-1 HTML block, so blank lines inside the diagram survive parsing).
  // Mermaid expects <div class="mermaid"> with whitespace-trimmed content, so
  // convert here right before mermaid.run().
  document.querySelectorAll('pre.mermaid').forEach(function(pre) {
    var div = document.createElement('div');
    div.className = 'mermaid';
    div.textContent = pre.textContent.replace(/^\s*\n/, '').replace(/\s+$/, '');
    pre.replaceWith(div);
  });
  if (typeof mermaid === 'undefined') return;
  mermaid.initialize({ startOnLoad: false, theme: '{THEME}', securityLevel: 'loose' });
  mermaid.run();
})();
"#;

// ---- Settings ---------------------------------------------------------------
fn settings_dir() -> PathBuf {
    glib::user_config_dir().join("rendermd")
}

fn settings_file() -> PathBuf {
    settings_dir().join("settings.ini")
}

// ---- Markdown rendering -----------------------------------------------------

// :shortcode: -> emoji char. Curated list of the GitHub shortcodes that
// actually show up in issues, PRs, and READMEs — not exhaustive. The match
// arm compiles to a fast lookup; extend in place as users hit gaps.
fn lookup_emoji(name: &str) -> Option<&'static str> {
    Some(match name {
        // Reactions / approval
        "+1" | "thumbsup" => "👍",
        "-1" | "thumbsdown" => "👎",
        "tada" => "🎉",
        "rocket" => "🚀",
        "fire" => "🔥",
        "sparkles" => "✨",
        "100" => "💯",
        "ok_hand" => "👌",
        "clap" => "👏",
        "wave" => "👋",
        "pray" => "🙏",
        "muscle" => "💪",
        "raised_hands" => "🙌",
        "handshake" => "🤝",
        "heart" => "❤️",
        "broken_heart" => "💔",
        "heart_eyes" => "😍",
        "eyes" => "👀",
        "brain" => "🧠",
        // Status
        "white_check_mark" => "✅",
        "heavy_check_mark" => "✔️",
        "x" => "❌",
        "heavy_multiplication_x" => "✖️",
        "warning" => "⚠️",
        "exclamation" => "❗",
        "question" => "❓",
        "grey_exclamation" => "❕",
        "grey_question" => "❔",
        "bangbang" => "‼️",
        "interrobang" => "⁉️",
        "no_entry" => "⛔",
        "no_entry_sign" => "🚫",
        "construction" => "🚧",
        "stop_sign" => "🛑",
        // Faces
        "smile" => "😄",
        "smiley" => "😃",
        "grin" => "😁",
        "grinning" => "😀",
        "joy" => "😂",
        "rofl" => "🤣",
        "laughing" => "😆",
        "sweat_smile" => "😅",
        "wink" => "😉",
        "blush" => "😊",
        "innocent" => "😇",
        "thinking" => "🤔",
        "neutral_face" => "😐",
        "expressionless" => "😑",
        "no_mouth" => "😶",
        "smirk" => "😏",
        "unamused" => "😒",
        "roll_eyes" => "🙄",
        "grimacing" => "😬",
        "face_with_raised_eyebrow" => "🤨",
        "confused" => "😕",
        "worried" => "😟",
        "frowning" => "😦",
        "anguished" => "😧",
        "open_mouth" => "😮",
        "hushed" => "😯",
        "astonished" => "😲",
        "scream" => "😱",
        "tired_face" => "😫",
        "weary" => "😩",
        "sleepy" => "😪",
        "sleeping" => "😴",
        "yum" => "😋",
        "stuck_out_tongue" => "😛",
        "stuck_out_tongue_winking_eye" => "😜",
        "zany_face" => "🤪",
        "face_with_hand_over_mouth" => "🤭",
        "shushing_face" => "🤫",
        "face_with_monocle" => "🧐",
        "nerd_face" => "🤓",
        "sunglasses" => "😎",
        "star_struck" => "🤩",
        "partying_face" => "🥳",
        "cry" => "😢",
        "sob" => "😭",
        "rage" => "😡",
        "angry" => "😠",
        "triumph" => "😤",
        "imp" => "👿",
        "smiling_imp" => "😈",
        "skull" => "💀",
        "skull_and_crossbones" => "☠️",
        "alien" => "👽",
        "robot" => "🤖",
        "ghost" => "👻",
        // Tech / build
        "computer" => "💻",
        "desktop_computer" => "🖥️",
        "keyboard" => "⌨️",
        "mouse" => "🖱️",
        "iphone" => "📱",
        "phone" | "telephone" => "☎️",
        "package" => "📦",
        "memo" | "pencil" => "📝",
        "pencil2" => "✏️",
        "bulb" => "💡",
        "wrench" => "🔧",
        "hammer" => "🔨",
        "hammer_and_wrench" => "🛠️",
        "gear" => "⚙️",
        "nut_and_bolt" => "🔩",
        "bug" => "🐛",
        "lock" => "🔒",
        "unlock" => "🔓",
        "key" => "🔑",
        "shield" => "🛡️",
        "satellite" => "🛰️",
        "zap" => "⚡",
        "boom" => "💥",
        "bomb" => "💣",
        "link" => "🔗",
        "paperclip" => "📎",
        "books" => "📚",
        "book" => "📖",
        "page_facing_up" => "📄",
        "scroll" => "📜",
        "clipboard" => "📋",
        "calendar" => "📅",
        "stopwatch" => "⏱️",
        "alarm_clock" => "⏰",
        "hourglass" => "⌛",
        "hourglass_flowing_sand" => "⏳",
        // Visual cue
        "star" => "⭐",
        "star2" => "🌟",
        "trophy" => "🏆",
        "medal_sports" => "🏅",
        "first_place_medal" => "🥇",
        "second_place_medal" => "🥈",
        "third_place_medal" => "🥉",
        "crown" => "👑",
        "gem" => "💎",
        "moneybag" => "💰",
        "dollar" => "💵",
        // Arrows / pointers
        "arrow_up" => "⬆️",
        "arrow_down" => "⬇️",
        "arrow_left" => "⬅️",
        "arrow_right" => "➡️",
        "arrow_upper_right" => "↗️",
        "arrow_lower_right" => "↘️",
        "arrow_upper_left" => "↖️",
        "arrow_lower_left" => "↙️",
        "arrows_clockwise" => "🔃",
        "arrows_counterclockwise" => "🔄",
        "leftwards_arrow_with_hook" => "↩️",
        "arrow_right_hook" => "↪️",
        "point_up" => "☝️",
        "point_down" => "👇",
        "point_left" => "👈",
        "point_right" => "👉",
        // Misc that show up a lot
        "coffee" => "☕",
        "beer" => "🍺",
        "pizza" => "🍕",
        "poop" | "shit" | "hankey" => "💩",
        "tada_party" => "🥳",
        "balloon" => "🎈",
        "gift" => "🎁",
        "art" => "🎨",
        "rainbow" => "🌈",
        "sun" | "sunny" => "☀️",
        "cloud" => "☁️",
        "snowflake" => "❄️",
        "umbrella" => "☂️",
        "earth_americas" => "🌎",
        "earth_africa" => "🌍",
        "earth_asia" => "🌏",
        // Animals
        "dog" => "🐶",
        "cat" => "🐱",
        "fox_face" => "🦊",
        "lion" => "🦁",
        "monkey" => "🐒",
        "see_no_evil" => "🙈",
        "hear_no_evil" => "🙉",
        "speak_no_evil" => "🙊",
        "panda_face" => "🐼",
        "penguin" => "🐧",
        "owl" => "🦉",
        "snail" => "🐌",
        "ant" => "🐜",
        "honeybee" => "🐝",
        "butterfly" => "🦋",
        "octopus" => "🐙",
        // Plants / nature
        "seedling" => "🌱",
        "evergreen_tree" => "🌲",
        "deciduous_tree" => "🌳",
        "palm_tree" => "🌴",
        "cactus" => "🌵",
        "herb" => "🌿",
        "leaves" => "🍃",
        "rose" => "🌹",
        "cherry_blossom" => "🌸",
        "sunflower" => "🌻",
        // Food (common in changelogs/PRs)
        "apple" | "red_apple" => "🍎",
        "banana" => "🍌",
        "cake" => "🍰",
        "cookie" => "🍪",
        "doughnut" => "🍩",
        "icecream" => "🍨",
        "chocolate_bar" => "🍫",
        _ => return None,
    })
}

// Replace :shortcode: spans on a single line. Doesn't track inline-code (`...`)
// boundaries within the line — :foo: inside backticks may still be replaced.
// Acceptable v1 trade-off; markdown's own parser will still render the spans
// correctly because the resulting emoji char is valid inline-code content.
fn replace_shortcodes_in_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < n {
        if chars[i] == ':' {
            let mut j = i + 1;
            while j < n {
                let c = chars[j];
                if c == ':' || !(c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-') {
                    break;
                }
                j += 1;
            }
            if j < n && chars[j] == ':' && j > i + 1 {
                let name: String = chars[i + 1..j].iter().collect();
                if let Some(emoji) = lookup_emoji(&name) {
                    out.push_str(emoji);
                    i = j + 1;
                    continue;
                }
            }
            out.push(':');
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// Walk the document line-by-line, replace :shortcode: emoji on lines that
// aren't inside a fenced code block (any ``` fence — mermaid, language, or
// untagged). Runs before preprocess_mermaid_blocks so the input is still
// raw markdown source.
fn preprocess_emoji(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if in_fence {
            out.push_str(line);
            out.push('\n');
            if trimmed.starts_with("```") {
                in_fence = false;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            in_fence = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(&replace_shortcodes_in_line(line));
        out.push('\n');
    }
    out
}

// GitHub-style alerts: > [!NOTE]/[!TIP]/[!IMPORTANT]/[!WARNING]/[!CAUTION]
// followed by `> body...` lines. Comrak doesn't ship this extension, so we
// detect openers, collect blockquote continuations, render the body to HTML
// recursively (without the syntect plugin to keep alerts free of nested
// concerns), and emit a single-line <div class="alert alert-X">…</div>.
//
// Why single-line: <div> is a CommonMark "type 6" HTML block — terminates
// at the next blank line. If the rendered body had its own newlines (which
// it does: comrak emits paragraphs separated by \n), the block would close
// mid-alert. Replacing newlines with spaces in the rendered HTML keeps the
// whole alert as one logical line that comrak passes through verbatim.

// Octicons (MIT-licensed) — rough matches for what GitHub uses on alerts.
const ALERT_ICON_NOTE: &str = r##"<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M0 8a8 8 0 1 1 16 0A8 8 0 0 1 0 8Zm8-6.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13ZM6.5 7.75A.75.75 0 0 1 7.25 7h1a.75.75 0 0 1 .75.75v2.75h.25a.75.75 0 0 1 0 1.5h-2a.75.75 0 0 1 0-1.5h.25v-2h-.25a.75.75 0 0 1-.75-.75ZM8 6a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"/></svg>"##;

const ALERT_ICON_TIP: &str = r##"<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M8 1.5c-2.363 0-4 1.69-4 3.75 0 .984.424 1.625.984 2.304l.214.253c.223.264.47.556.673.848.284.411.537.896.621 1.49a.75.75 0 0 1-1.484.211c-.04-.282-.163-.547-.37-.847a8.456 8.456 0 0 0-.542-.68c-.084-.1-.173-.205-.268-.32C3.201 7.75 2.5 6.766 2.5 5.25 2.5 2.31 4.863 0 8 0s5.5 2.31 5.5 5.25c0 1.516-.701 2.5-1.328 3.259-.095.115-.184.22-.268.319-.207.245-.383.453-.541.681-.208.3-.33.565-.37.847a.751.751 0 0 1-1.485-.212c.084-.593.337-1.078.621-1.489.203-.292.45-.584.673-.848.075-.088.147-.173.213-.253.561-.679.985-1.32.985-2.304 0-2.06-1.637-3.75-4-3.75ZM5.75 12h4.5a.75.75 0 0 1 0 1.5h-4.5a.75.75 0 0 1 0-1.5ZM6 15.25a.75.75 0 0 1 .75-.75h2.5a.75.75 0 0 1 0 1.5h-2.5a.75.75 0 0 1-.75-.75Z"/></svg>"##;

const ALERT_ICON_IMPORTANT: &str = r##"<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M0 1.75C0 .784.784 0 1.75 0h12.5C15.216 0 16 .784 16 1.75v9.5A1.75 1.75 0 0 1 14.25 13H8.06l-2.573 2.573A1.458 1.458 0 0 1 3 14.543V13H1.75A1.75 1.75 0 0 1 0 11.25Zm1.75-.25a.25.25 0 0 0-.25.25v9.5c0 .138.112.25.25.25h2a.75.75 0 0 1 .75.75v2.19l2.72-2.72a.749.749 0 0 1 .53-.22h6.5a.25.25 0 0 0 .25-.25v-9.5a.25.25 0 0 0-.25-.25Zm7 2.25v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 9a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"/></svg>"##;

const ALERT_ICON_WARNING: &str = r##"<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575Zm1.763.707a.25.25 0 0 0-.44 0L1.698 13.132a.25.25 0 0 0 .22.368h12.164a.25.25 0 0 0 .22-.368Zm.53 3.996v2.5a.75.75 0 0 1-1.5 0v-2.5a.75.75 0 0 1 1.5 0ZM9 11a1 1 0 1 1-2 0 1 1 0 0 1 2 0Z"/></svg>"##;

const ALERT_ICON_CAUTION: &str = r##"<svg viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true"><path d="M4.47.22A.749.749 0 0 1 5 0h6c.199 0 .389.079.53.22l4.25 4.25c.141.14.22.331.22.53v6a.749.749 0 0 1-.22.53l-4.25 4.25A.749.749 0 0 1 11 16H5a.749.749 0 0 1-.53-.22L.22 11.53A.749.749 0 0 1 0 11V5c0-.199.079-.389.22-.53Zm.84 1.28L1.5 5.31v5.38l3.81 3.81h5.38l3.81-3.81V5.31L10.69 1.5ZM8 4a.75.75 0 0 1 .75.75v3.5a.75.75 0 0 1-1.5 0v-3.5A.75.75 0 0 1 8 4Zm0 8a1 1 0 1 1 0-2 1 1 0 0 1 0 2Z"/></svg>"##;

// Variants table: (token, lowercase variant for CSS class, label, icon).
const ALERT_VARIANTS: &[(&str, &str, &str, &str)] = &[
    ("[!NOTE]", "note", "Note", ALERT_ICON_NOTE),
    ("[!TIP]", "tip", "Tip", ALERT_ICON_TIP),
    (
        "[!IMPORTANT]",
        "important",
        "Important",
        ALERT_ICON_IMPORTANT,
    ),
    ("[!WARNING]", "warning", "Warning", ALERT_ICON_WARNING),
    ("[!CAUTION]", "caution", "Caution", ALERT_ICON_CAUTION),
];

fn detect_alert_opener(
    line: &str,
) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    let trimmed = line.trim();
    let after_gt = trimmed.strip_prefix('>')?.trim();
    ALERT_VARIANTS.iter().find(|v| after_gt == v.0)
}

fn strip_blockquote_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('>') {
        // Per CommonMark, a single optional space after `>` is part of the
        // marker and should be stripped.
        rest.strip_prefix(' ').unwrap_or(rest).to_string()
    } else {
        line.to_string()
    }
}

fn render_alert_body(body: &str) -> String {
    // Comrak with the same GFM extensions as the main pipeline minus the
    // syntect plugin (alerts almost never carry highlighted code blocks,
    // and skipping syntect keeps the body cheap to render).
    let mut options = ComrakOptions::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.parse.smart = true;
    options.render.unsafe_ = true;
    comrak::markdown_to_html(body, &options)
}

fn preprocess_alerts(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut iter = text.lines().peekable();

    while let Some(line) = iter.next() {
        if let Some(&(_, variant, label, icon)) = detect_alert_opener(line) {
            // Collect continuation lines (blockquote-prefixed).
            let mut body_lines: Vec<String> = Vec::new();
            while let Some(peek) = iter.peek() {
                if !peek.trim_start().starts_with('>') {
                    break;
                }
                body_lines.push(strip_blockquote_prefix(peek));
                iter.next();
            }
            let body_md = body_lines.join("\n");
            let body_html = render_alert_body(&body_md);
            // Flatten newlines so the resulting <div> survives CommonMark's
            // type-6 block parsing (which terminates on a blank line).
            let body_oneline = body_html.replace('\n', " ");

            out.push('\n');
            out.push_str(&format!(
                "<div class=\"alert alert-{}\"><div class=\"alert-title\">{} <span>{}</span></div>{}</div>",
                variant, icon, label, body_oneline
            ));
            out.push_str("\n\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

// Convert ```mermaid fences to <div class="mermaid">SOURCE</div> raw HTML
// before comrak runs. Doing this at the markdown level (rather than
// post-processing comrak's output) sidesteps the SyntectAdapter wrapping
// unknown languages in its own markup. Returns (preprocessed text, had-any).
fn preprocess_mermaid_blocks(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut had_mermaid = false;
    let mut in_block = false;
    let mut buf = String::new();
    for line in text.lines() {
        if in_block {
            if line.trim() == "```" {
                // <pre> is a CommonMark "type 1" HTML block — terminates only
                // on </pre>, so blank lines inside the diagram survive intact.
                // (A <div> wrapper would terminate at the first blank line and
                // slice multi-paragraph diagrams in half.)
                out.push_str("\n<pre class=\"mermaid\">\n");
                out.push_str(&html_escape(&buf));
                out.push_str("</pre>\n\n");
                in_block = false;
                had_mermaid = true;
                buf.clear();
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
        } else if line.trim() == "```mermaid" {
            in_block = true;
            buf.clear();
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    // Unclosed fence: restore the original lines verbatim so we don't drop content.
    if in_block {
        out.push_str("```mermaid\n");
        out.push_str(&buf);
    }
    (out, had_mermaid)
}

// One commit affecting the open file, returned by `git log` and used to
// populate the history rail in the preview. `sha` is the full hash;
// `short_sha` is what we display.
#[derive(Clone, Debug)]
struct Commit {
    sha: String,
    short_sha: String,
    iso_date: String,
    subject: String,
}

// State for "viewing a historical revision" mode. The buffer is left
// alone; `text` is what the preview renders. `parent_text` (when
// present) lets us show diff markers against the parent commit using
// the existing change-marker pipeline. `commit_unix_secs` populates
// the hover banner's "X ago" label.
#[derive(Clone, Debug)]
struct HistorySnapshot {
    sha: String,
    text: String,
    parent_text: Option<String>,
    commit_unix_secs: i64,
}

/// Per-table sort state held on the application (one entry per
/// currently-sorted table). The snapshot is the *pre-sort* row vector
/// so the tri-state cycle's "Off" can restore the original order.
/// Cleared by any edit that changes the row set (cell content change,
/// row/column insert/delete).
#[derive(Clone, Debug)]
struct TableSortSnapshot {
    original_rows: Vec<Vec<tables::model::Cell>>,
    col: usize,
    direction: tables::model::SortDirection,
}

// Run `git log --follow` for the file and return the commit list, newest
// first. Returns None if git isn't installed, the file isn't in a repo,
// the file isn't tracked, or git errored. Capped at 100 commits to keep
// the rail manageable on long-lived files; a "show more" affordance is
// future work.
fn fetch_git_history(file_path: &Path) -> Option<Vec<Commit>> {
    let parent = file_path.parent()?;
    let file_name = file_path.file_name()?.to_str()?;

    // Cheap check: are we inside a working tree at all?
    let in_repo = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(parent)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some();
    if !in_repo {
        return None;
    }

    let output = std::process::Command::new("git")
        .args([
            "log",
            "--follow",
            "-n",
            "100",
            "--pretty=format:%H%x09%h%x09%cI%x09%s",
            "--",
            file_name,
        ])
        .current_dir(parent)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let commits: Vec<Commit> = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '\t');
            Some(Commit {
                sha: parts.next()?.to_string(),
                short_sha: parts.next()?.to_string(),
                iso_date: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
            })
        })
        .collect();
    if commits.is_empty() {
        None
    } else {
        Some(commits)
    }
}

// Resolve the working tree's toplevel and the file's path relative to
// it. Needed for `git show <sha>:<relpath>`.
fn repo_relative(file_path: &Path) -> Option<(PathBuf, String)> {
    let parent = file_path.parent()?;
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(parent)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let toplevel = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let toplevel_path = PathBuf::from(toplevel);
    let rel = file_path.strip_prefix(&toplevel_path).ok()?;
    Some((toplevel_path, rel.to_string_lossy().to_string()))
}

// Read the file's content at a given commit. Returns None for renamed
// files (would need `git log --follow --name-only` to track) or other
// `git show` failures.
fn fetch_revision_text(file_path: &Path, sha: &str) -> Option<String> {
    let (toplevel, rel) = repo_relative(file_path)?;
    let arg = format!("{sha}:{rel}");
    let output = std::process::Command::new("git")
        .args(["show", &arg])
        .current_dir(&toplevel)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

// Get the parent SHA of a given commit. Returns None for the root
// commit (no parent) or on any git error.
fn fetch_parent_sha(file_path: &Path, sha: &str) -> Option<String> {
    let parent = file_path.parent()?;
    let parent_arg = format!("{sha}^");
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--quiet", &parent_arg])
        .current_dir(parent)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn iso_to_unix_secs(iso: &str) -> i64 {
    glib::DateTime::from_iso8601(iso, None)
        .map(|d| d.to_unix())
        .unwrap_or(0)
}

fn format_mtime(path: &Path) -> String {
    let modified = match fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return String::new(),
    };
    let secs = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return String::new(),
    };
    match glib::DateTime::from_unix_local(secs).and_then(|dt| dt.format("%Y-%m-%d %H:%M")) {
        Ok(s) => format!("Updated {}", s),
        Err(_) => String::new(),
    }
}

fn char_to_byte_offset(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

// Locate the markdown or HTML form of an image with the given src in the
// buffer text. Returns (char_offset, char_len) for the *whole* image
// expression so the caller can replace it cleanly. Naive: matches the
// first occurrence; if the same src appears more than once in the doc,
// only the first is touched. Good enough for typical use.
fn find_image_ref(text: &str, src: &str) -> Option<(usize, usize)> {
    // Markdown form: ![alt](src) or ![alt](src "title").
    let needle = format!("({src}");
    if let Some(byte_idx) = text.find(&needle) {
        // Confirm the byte just before is `]` and walk back to find `![`.
        let before = &text[..byte_idx];
        if before.ends_with(']') {
            if let Some(bracket_byte) = before.rfind("![") {
                // Find the closing `)` after byte_idx.
                let after_paren_start = byte_idx + 1;
                if let Some(rel_close) = text[after_paren_start..].find(')') {
                    let end_byte = after_paren_start + rel_close + 1;
                    let char_start = text[..bracket_byte].chars().count();
                    let char_end = text[..end_byte].chars().count();
                    return Some((char_start, char_end - char_start));
                }
            }
        }
    }
    // HTML form: <img ... src="src" ...> or src='src'.
    for delim in ['"', '\''] {
        let needle = format!("src={delim}{src}{delim}");
        if let Some(byte_idx) = text.find(&needle) {
            let before = &text[..byte_idx];
            if let Some(img_byte) = before.rfind("<img") {
                if let Some(rel_close) = text[img_byte..].find('>') {
                    let end_byte = img_byte + rel_close + 1;
                    let char_start = text[..img_byte].chars().count();
                    let char_end = text[..end_byte].chars().count();
                    return Some((char_start, char_end - char_start));
                }
            }
        }
    }
    None
}

// Build the new markup for an image, preferring markdown form when no
// width is set so the doc stays clean. Width forces the HTML form since
// CommonMark doesn't have an inline width syntax.
fn build_image_markup(src: &str, width: Option<&str>, alt: &str) -> String {
    match width {
        None => format!("![{alt}]({src})"),
        Some(w) => {
            let alt_attr = if alt.is_empty() {
                String::new()
            } else {
                format!(" alt=\"{}\"", html_escape(alt))
            };
            format!(
                "<img src=\"{}\"{} width=\"{}\">",
                html_escape(src),
                alt_attr,
                html_escape(w)
            )
        }
    }
}

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

// Build the HTML for the left-side commit timeline. Returns the rail
// markup plus a small <script> that wires click messages back to Rust.
// Empty string when there's no history. When `visible` is false but
// commits exist, returns just an unobtrusive hint dot so the user
// knows the option is there.
fn build_history_rail_html(commits: &[Commit], viewing_sha: Option<&str>, visible: bool) -> String {
    if commits.is_empty() {
        return String::new();
    }
    if !visible {
        // Click handler so the hint dot itself toggles history back on.
        return r##"<div class="rmd-history-hint" title="Git history available — click to show"></div>
<script>
(function() {
  var msg = (window.webkit && window.webkit.messageHandlers) || null;
  if (!msg || !msg.toggleHistory) return;
  var hint = document.querySelector(".rmd-history-hint");
  if (hint) hint.addEventListener("click", function() {
    msg.toggleHistory.postMessage("");
  });
})();
</script>"##
            .to_string();
    }

    let mut html = String::from(
        r#"<div class="rmd-history-rail" role="navigation" aria-label="Commit history">"#,
    );
    html.push_str(r#"<div class="rmd-history-track"></div>"#);
    for c in commits {
        let active = viewing_sha.map(|v| v == c.sha).unwrap_or(false);
        let date = c.iso_date.split('T').next().unwrap_or(&c.iso_date);
        let tooltip = format!("{} — {}\n{}", c.short_sha, date, c.subject);
        let cls = if active {
            "rmd-history-circle rmd-history-active"
        } else {
            "rmd-history-circle"
        };
        html.push_str(&format!(
            r#"<button type="button" class="{cls}" data-sha="{sha}" title="{title}"></button>"#,
            cls = cls,
            sha = html_escape(&c.sha),
            title = html_escape(&tooltip),
        ));
    }
    html.push_str("</div>");
    // Click handler — Phase 2 will hook this up to render the chosen
    // revision. For now, posts the SHA to the `commitClick` handler
    // so we can verify wiring end-to-end.
    html.push_str(
        r#"<script>
(function() {
  var msg = (window.webkit && window.webkit.messageHandlers) || null;
  if (!msg || !msg.commitClick) return;
  document.querySelectorAll(".rmd-history-circle").forEach(function(c) {
    c.addEventListener("click", function() {
      msg.commitClick.postMessage(c.getAttribute("data-sha") || "");
    });
  });
})();
</script>"#,
    );
    html
}

// Multiset line-subtraction diff: lines that appear more often in `new` than
// in `old` are flagged as changed. Imprecise around duplicate lines but
// adequate for prose change-marking and avoids pulling in a diff crate.
// Returned indices are 1-based against `new`.
fn compute_changed_lines(old: &str, new: &str) -> HashSet<usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for line in old.lines() {
        *counts.entry(line).or_default() += 1;
    }
    let mut changed = HashSet::new();
    for (i, line) in new.lines().enumerate() {
        match counts.get_mut(line) {
            Some(c) if *c > 0 => *c -= 1,
            _ => {
                changed.insert(i + 1);
            }
        }
    }
    changed
}

// Approximate top-level Markdown block detection: split on blank lines, but
// keep fenced code blocks (``` or ~~~) together. Good enough for marking
// changed regions in prose; a list with internal blank lines will be split
// per-item, which is finer-grained than the AST but still informative.
fn split_top_level_blocks(text: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut block_start: Option<usize> = None;
    let mut in_fence = false;
    let mut fence_marker: &str = "";
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if in_fence {
            if trimmed.starts_with(fence_marker) {
                in_fence = false;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if block_start.is_none() {
                block_start = Some(idx + 1);
            }
            in_fence = true;
            fence_marker = if trimmed.starts_with("```") {
                "```"
            } else {
                "~~~"
            };
        } else if line.trim().is_empty() {
            if let Some(start) = block_start.take() {
                blocks.push((start, idx));
            }
        } else if block_start.is_none() {
            block_start = Some(idx + 1);
        }
    }
    if let Some(start) = block_start.take() {
        blocks.push((start, lines.len()));
    }
    blocks
}

struct PendingChanges {
    changed_lines: HashSet<usize>,
    old_text: String,
    reload_ts: i64,
}

// Insert a `<div class="rmd-changed-marker">` HTML block before each
// top-level block whose line range overlaps `changes.changed_lines`. The
// CSS sibling selector then puts a left border on the next rendered
// element. Each marker also carries the corresponding *old* block text and
// the reload timestamp, which the bundled JS uses to populate a tooltip on
// hover.
fn inject_change_markers(text: &str, changes: &PendingChanges, dark: bool) -> String {
    if changes.changed_lines.is_empty() {
        return text.to_string();
    }
    let new_blocks = split_top_level_blocks(text);
    let new_lines: Vec<&str> = text.lines().collect();
    let old_lines: Vec<&str> = changes.old_text.lines().collect();

    let mut markers: HashMap<usize, String> = HashMap::new();
    for (start, end) in new_blocks {
        if !(start..=end).any(|l| changes.changed_lines.contains(&l)) {
            continue;
        }
        // Best-effort positional match: take the same 1-based line range
        // from old_text and new_text, clamped. Imprecise after big inserts
        // / deletes higher up, but a useful approximation.
        let slice = |lines: &[&str]| -> String {
            if lines.is_empty() || start > lines.len() {
                String::new()
            } else {
                let lo = start - 1;
                let hi = end.min(lines.len());
                lines[lo..hi].join("\n")
            }
        };
        let old_block = slice(&old_lines);
        let new_block = slice(&new_lines);
        // Fenced code blocks need a different code path: comrak escapes
        // anything inside <pre><code>, so our <del>/<ins>-annotated
        // markdown shows up as literal HTML tags. For these, build the
        // diff HTML ourselves and bypass comrak.
        let prev_html =
            if block_starts_with_fence(&new_block) || block_starts_with_fence(&old_block) {
                build_code_block_diff_html(&old_block, &new_block)
            } else {
                let annotated_md = build_annotated_diff_md(&old_block, &new_block);
                render_block_to_inline_html(&annotated_md, dark)
            };
        let escaped = glib::Uri::escape_string(&prev_html, None, false).to_string();
        markers.insert(
            start,
            format!(
                "<div class=\"rmd-changed-marker\" data-prev-html=\"{}\" data-age-ts=\"{}\"></div>\n\n",
                escaped, changes.reload_ts
            ),
        );
    }

    let mut out = String::with_capacity(text.len() + markers.len() * 96);
    for (idx, line) in new_lines.iter().enumerate() {
        if let Some(m) = markers.get(&(idx + 1)) {
            out.push_str(m);
        }
        out.push_str(line);
        out.push('\n');
    }
    // Append a one-shot script that wires native tooltips on the changed
    // blocks. Native browser tooltip delay (~700–1000 ms) gives the
    // "hover for a second" behaviour for free.
    out.push('\n');
    out.push_str(CHANGE_TOOLTIP_JS);
    out
}

const CHANGE_TOOLTIP_JS: &str = r#"<script>
(function() {
  function fmtAge(s) {
    if (s < 5) return "just now";
    if (s < 60) return s + " seconds ago";
    if (s < 3600) {
      var m = Math.floor(s / 60);
      return m + (m === 1 ? " minute ago" : " minutes ago");
    }
    if (s < 86400) {
      var h = Math.floor(s / 3600);
      return h + (h === 1 ? " hour ago" : " hours ago");
    }
    var d = Math.floor(s / 86400);
    return d + (d === 1 ? " day ago" : " days ago");
  }
  function unwrapToInner(html, expectedTag) {
    // Comrak wraps each block in its outer tag (e.g. <p>, <h2>, <ul>).
    // Since the target element ALREADY is that wrapper, strip the outer
    // tag so we don't end up with nested <p><p>... etc.
    var tmp = document.createElement("div");
    tmp.innerHTML = html;
    var first = tmp.firstElementChild;
    if (first && first.tagName === expectedTag) {
      return first.innerHTML;
    }
    return html;
  }
  function init() {
    document.querySelectorAll(".rmd-changed-marker").forEach(function(m) {
      var t = m.nextElementSibling;
      if (!t) return;
      var ts = parseInt(m.getAttribute("data-age-ts"), 10);
      var prevRaw = m.getAttribute("data-prev-html") || "";
      var prevHtml = "";
      try { prevHtml = decodeURIComponent(prevRaw); } catch (e) { prevHtml = ""; }
      var inner = prevHtml ? unwrapToInner(prevHtml, t.tagName) : "";
      var swapTimer = null;
      t.addEventListener("mouseenter", function() {
        if (t.classList.contains("rmd-showing-prev")) return;
        if (swapTimer) clearTimeout(swapTimer);
        swapTimer = setTimeout(function() {
          swapTimer = null;
          if (!("rmdOriginal" in t.dataset)) {
            t.dataset.rmdOriginal = t.innerHTML;
          }
          var ageS = Math.floor(Date.now() / 1000) - ts;
          var banner = '<div class="rmd-prev-banner">Edited externally ' + fmtAge(ageS) + '</div>';
          var content = inner || '<em class="rmd-prev-empty">(no prior content for this block)</em>';
          t.innerHTML = content + banner;
          t.classList.add("rmd-showing-prev");
        }, 1000);
      });
      t.addEventListener("mouseleave", function() {
        if (swapTimer) { clearTimeout(swapTimer); swapTimer = null; }
        if (t.classList.contains("rmd-showing-prev") && "rmdOriginal" in t.dataset) {
          t.innerHTML = t.dataset.rmdOriginal;
          delete t.dataset.rmdOriginal;
          t.classList.remove("rmd-showing-prev");
        }
      });
    });
    buildMinimap();
  }
  // Right-edge minimap of all changed blocks. Ticks are positioned
  // proportionally to where their target block sits in the document so
  // the user can see at a glance where edits landed in a long file,
  // and click any tick to scroll there.
  function buildMinimap() {
    var existing = document.querySelector(".rmd-minimap");
    if (existing) existing.remove();
    var markers = document.querySelectorAll(".rmd-changed-marker");
    if (!markers.length) return;
    var docHeight = document.documentElement.scrollHeight;
    if (docHeight <= window.innerHeight + 4) return;
    var minimap = document.createElement("div");
    minimap.className = "rmd-minimap";
    markers.forEach(function(m) {
      var target = m.nextElementSibling;
      if (!target) return;
      var rect = target.getBoundingClientRect();
      var topInDoc = rect.top + window.scrollY;
      var ratio = Math.max(0, Math.min(1, topInDoc / docHeight));
      var tick = document.createElement("div");
      tick.className = "rmd-minimap-tick";
      tick.style.top = (ratio * 100) + "%";
      tick.title = "Changed block — click to jump";
      tick.addEventListener("click", function() {
        target.scrollIntoView({ behavior: "smooth", block: "center" });
      });
      minimap.appendChild(tick);
    });
    document.body.appendChild(minimap);
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
  // Recompute on resize so tick positions stay accurate.
  var resizeTimer = null;
  window.addEventListener("resize", function() {
    if (resizeTimer) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(buildMinimap, 120);
  });
})();
</script>
"#;

// Greedy line-level diff plus intra-line word-level diff. Adjacent
// delete/add pairs that share enough vocabulary are rendered as a single
// "modified" line with only the changed words highlighted, instead of
// dumping the full old and new lines as two solid red/green blocks.
// Plain LCS-free; good enough for typical edits.

#[derive(Debug)]
enum LineOp<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Add(&'a str),
}

fn line_diff_ops<'a>(old_lines: &'a [&'a str], new_lines: &'a [&'a str]) -> Vec<LineOp<'a>> {
    let mut ops = Vec::new();
    let mut i = 0usize;
    for new_line in new_lines {
        let found = old_lines[i..].iter().position(|l| l == new_line);
        match found {
            Some(rel) => {
                for line in old_lines.iter().skip(i).take(rel) {
                    ops.push(LineOp::Delete(line));
                }
                ops.push(LineOp::Equal(new_line));
                i += rel + 1;
            }
            None => ops.push(LineOp::Add(new_line)),
        }
    }
    for line in old_lines.iter().skip(i) {
        ops.push(LineOp::Delete(line));
    }
    ops
}

// Tokenize as runs of alphanumerics, runs of whitespace, or single
// non-word chars. Keeps whitespace as its own token so we don't lose
// spacing when stitching the segments back together.
fn tokenize_for_diff(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut iter = s.char_indices().peekable();
    while let Some(&(start, c)) = iter.peek() {
        if c.is_alphanumeric() {
            iter.next();
            let mut end = start + c.len_utf8();
            while let Some(&(_, c2)) = iter.peek() {
                if !c2.is_alphanumeric() {
                    break;
                }
                end += c2.len_utf8();
                iter.next();
            }
            out.push(&s[start..end]);
        } else if c.is_whitespace() {
            iter.next();
            let mut end = start + c.len_utf8();
            while let Some(&(_, c2)) = iter.peek() {
                if !c2.is_whitespace() {
                    break;
                }
                end += c2.len_utf8();
                iter.next();
            }
            out.push(&s[start..end]);
        } else {
            iter.next();
            let end = start + c.len_utf8();
            out.push(&s[start..end]);
        }
    }
    out
}

// Multiset overlap divided by the larger token count. Whitespace and empty
// tokens don't contribute. Returns 1.0 for two empty inputs.
fn line_similarity(a: &str, b: &str) -> f64 {
    let keep = |t: &&str| !t.trim().is_empty();
    let a_tokens: Vec<&str> = tokenize_for_diff(a).into_iter().filter(keep).collect();
    let b_tokens: Vec<&str> = tokenize_for_diff(b).into_iter().filter(keep).collect();
    let bigger = a_tokens.len().max(b_tokens.len());
    if bigger == 0 {
        return 1.0;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in &a_tokens {
        *counts.entry(*t).or_default() += 1;
    }
    let mut common = 0usize;
    for t in &b_tokens {
        if let Some(c) = counts.get_mut(t) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    common as f64 / bigger as f64
}

// Builds an annotated *markdown* line for a modified pair: emits the new
// text with inline <del>/<ins> tags wrapping only the changed words, so
// after we feed it back to comrak it renders with the same typography as
// the surrounding paragraph / heading / list item.
fn build_annotated_word_diff_md(old: &str, new: &str) -> String {
    let old_tokens = tokenize_for_diff(old);
    let new_tokens = tokenize_for_diff(new);
    let mut out = String::new();
    let mut i = 0usize;
    // Buffer added tokens until the next match so deletions are emitted
    // before insertions — reads more naturally as "old → new".
    let mut pending_adds: Vec<&str> = Vec::new();
    let push_del = |out: &mut String, s: &str| {
        out.push_str("<del class=\"rmd-diff-w-del\">");
        out.push_str(s);
        out.push_str("</del>");
    };
    let push_ins = |out: &mut String, s: &str| {
        out.push_str("<ins class=\"rmd-diff-w-add\">");
        out.push_str(s);
        out.push_str("</ins>");
    };
    for new_t in &new_tokens {
        let found = old_tokens[i..].iter().position(|t| t == new_t);
        match found {
            Some(rel) => {
                for tok in old_tokens.iter().skip(i).take(rel) {
                    push_del(&mut out, tok);
                }
                for a in pending_adds.drain(..) {
                    push_ins(&mut out, a);
                }
                out.push_str(new_t);
                i += rel + 1;
            }
            None => pending_adds.push(new_t),
        }
    }
    for tok in old_tokens.iter().skip(i) {
        push_del(&mut out, tok);
    }
    for a in pending_adds.drain(..) {
        push_ins(&mut out, a);
    }
    out
}

fn block_starts_with_fence(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("").trim_start();
    first.starts_with("```") || first.starts_with("~~~")
}

// Inline HTML span for a chunk of word-diffed code. Same shape as the
// markdown-side word-diff but emits HTML directly so it survives being
// placed inside <pre><code>.
fn render_word_diff_html(old: &str, new: &str) -> String {
    let old_tokens = tokenize_for_diff(old);
    let new_tokens = tokenize_for_diff(new);
    let mut out = String::new();
    let mut i = 0usize;
    let mut pending_adds: Vec<&str> = Vec::new();
    let push_del = |out: &mut String, s: &str| {
        out.push_str(r#"<span class="rmd-diff-w-del">"#);
        out.push_str(&html_escape(s));
        out.push_str("</span>");
    };
    let push_ins = |out: &mut String, s: &str| {
        out.push_str(r#"<span class="rmd-diff-w-add">"#);
        out.push_str(&html_escape(s));
        out.push_str("</span>");
    };
    for new_t in &new_tokens {
        let found = old_tokens[i..].iter().position(|t| t == new_t);
        match found {
            Some(rel) => {
                for tok in old_tokens.iter().skip(i).take(rel) {
                    push_del(&mut out, tok);
                }
                for a in pending_adds.drain(..) {
                    push_ins(&mut out, a);
                }
                out.push_str(&html_escape(new_t));
                i += rel + 1;
            }
            None => pending_adds.push(new_t),
        }
    }
    for tok in old_tokens.iter().skip(i) {
        push_del(&mut out, tok);
    }
    for a in pending_adds.drain(..) {
        push_ins(&mut out, a);
    }
    out
}

// HTML diff for a fenced code block. Strips the fence lines, runs the
// same line + word diff machinery as build_annotated_diff_md, but emits
// <span> tags directly so they survive inside the <pre><code> wrapper.
fn build_code_block_diff_html(old: &str, new: &str) -> String {
    const MOD_THRESHOLD: f64 = 0.30;
    fn strip_fences(text: &str) -> Vec<&str> {
        let mut lines: Vec<&str> = text.lines().collect();
        if let Some(first) = lines.first() {
            let trimmed = first.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                lines.remove(0);
            }
        }
        if let Some(last) = lines.last() {
            let trimmed = last.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                lines.pop();
            }
        }
        lines
    }
    let old_lines = strip_fences(old);
    let new_lines = strip_fences(new);
    let ops = line_diff_ops(&old_lines, &new_lines);

    let mut body = String::new();
    let mut idx = 0usize;
    while idx < ops.len() {
        if let LineOp::Equal(line) = &ops[idx] {
            body.push_str(&html_escape(line));
            body.push('\n');
            idx += 1;
            continue;
        }
        let mut adds: Vec<&str> = Vec::new();
        let mut dels: Vec<&str> = Vec::new();
        while idx < ops.len() {
            match &ops[idx] {
                LineOp::Add(s) => adds.push(s),
                LineOp::Delete(s) => dels.push(s),
                LineOp::Equal(_) => break,
            }
            idx += 1;
        }
        let pair_count = adds.len().min(dels.len());
        for i in 0..pair_count {
            let old_line = dels[i];
            let new_line = adds[i];
            if line_similarity(old_line, new_line) >= MOD_THRESHOLD {
                body.push_str(&render_word_diff_html(old_line, new_line));
                body.push('\n');
            } else {
                body.push_str(r#"<span class="rmd-diff-w-del">"#);
                body.push_str(&html_escape(old_line));
                body.push_str("</span>\n");
                body.push_str(r#"<span class="rmd-diff-w-add">"#);
                body.push_str(&html_escape(new_line));
                body.push_str("</span>\n");
            }
        }
        for line in dels.iter().skip(pair_count) {
            body.push_str(r#"<span class="rmd-diff-w-del">"#);
            body.push_str(&html_escape(line));
            body.push_str("</span>\n");
        }
        for line in adds.iter().skip(pair_count) {
            body.push_str(r#"<span class="rmd-diff-w-add">"#);
            body.push_str(&html_escape(line));
            body.push_str("</span>\n");
        }
    }
    format!("<pre><code>{body}</code></pre>")
}

// Builds annotated markdown for a whole changed block. Equal lines pass
// through unchanged; modified pairs become an annotated word-diff line;
// pure adds/deletes get wrapped in <ins>/<del>. The result is plain
// markdown source that we feed back to comrak, so the rendered hover view
// preserves the original block's styling and just marks what changed.
fn build_annotated_diff_md(old: &str, new: &str) -> String {
    const MOD_THRESHOLD: f64 = 0.30;
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let ops = line_diff_ops(&old_lines, &new_lines);

    let mut out = String::new();
    let mut idx = 0usize;
    while idx < ops.len() {
        if let LineOp::Equal(line) = &ops[idx] {
            out.push_str(line);
            out.push('\n');
            idx += 1;
            continue;
        }
        let mut adds: Vec<&str> = Vec::new();
        let mut dels: Vec<&str> = Vec::new();
        while idx < ops.len() {
            match &ops[idx] {
                LineOp::Add(s) => adds.push(s),
                LineOp::Delete(s) => dels.push(s),
                LineOp::Equal(_) => break,
            }
            idx += 1;
        }
        let pair_count = adds.len().min(dels.len());
        for i in 0..pair_count {
            let old_line = dels[i];
            let new_line = adds[i];
            if line_similarity(old_line, new_line) >= MOD_THRESHOLD {
                out.push_str(&build_annotated_word_diff_md(old_line, new_line));
                out.push('\n');
            } else {
                out.push_str("<del class=\"rmd-diff-w-del\">");
                out.push_str(old_line);
                out.push_str("</del>\n");
                out.push_str("<ins class=\"rmd-diff-w-add\">");
                out.push_str(new_line);
                out.push_str("</ins>\n");
            }
        }
        for line in dels.iter().skip(pair_count) {
            out.push_str("<del class=\"rmd-diff-w-del\">");
            out.push_str(line);
            out.push_str("</del>\n");
        }
        for line in adds.iter().skip(pair_count) {
            out.push_str("<ins class=\"rmd-diff-w-add\">");
            out.push_str(line);
            out.push_str("</ins>\n");
        }
    }
    out
}

// Renders a markdown snippet to inline body HTML using the same comrak
// pipeline as the full-document renderer. Used by the hover-swap so the
// previous-version view keeps the original block's typography rather
// than dropping into a monospace diff view.
fn render_block_to_inline_html(text: &str, dark: bool) -> String {
    let mut options = ComrakOptions::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = false;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.extension.footnotes = true;
    options.extension.description_lists = true;
    options.extension.header_ids = Some(String::new());
    options.parse.smart = true;
    options.render.unsafe_ = true;
    options.render.github_pre_lang = true;

    let theme = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let adapter = SyntectAdapter::new(Some(theme));
    let mut plugins = ComrakPlugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&adapter);

    let with_emoji = preprocess_emoji(text);
    let with_alerts = preprocess_alerts(&with_emoji);
    let (preprocessed, _) = preprocess_mermaid_blocks(&with_alerts);
    markdown_to_html_with_plugins(&preprocessed, &options, &plugins)
}

fn render_markdown_to_html(text: &str, base_dir: Option<&Path>, dark: bool, title: &str) -> String {
    // Comrak gets us GFM-style features matching the Python python-markdown +
    // pymdown-extensions setup: tables, strikethrough, autolinks, task lists,
    // footnotes, smart quotes, superscript, description lists.
    let mut options = ComrakOptions::default();
    options.extension.strikethrough = true;
    options.extension.tagfilter = false;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.extension.footnotes = true;
    options.extension.description_lists = true;
    options.extension.header_ids = Some(String::new());
    options.parse.smart = true;
    options.render.unsafe_ = true;
    options.render.github_pre_lang = true;

    // Pick a syntect theme that flips with light/dark.
    let theme = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let adapter = SyntectAdapter::new(Some(theme));
    let mut plugins = ComrakPlugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&adapter);

    let with_emoji = preprocess_emoji(text);
    let with_alerts = preprocess_alerts(&with_emoji);
    let (preprocessed, had_mermaid) = preprocess_mermaid_blocks(&with_alerts);
    let body = markdown_to_html_with_plugins(&preprocessed, &options, &plugins);

    let mermaid_script = if had_mermaid {
        let theme = if dark { "dark" } else { "default" };
        let init = MERMAID_INIT_JS.replace("{THEME}", theme);
        let mut s = String::with_capacity(MERMAID_BUNDLE.len() + init.len() + 40);
        s.push_str("<script>");
        s.push_str(MERMAID_BUNDLE);
        s.push_str("</script>\n<script>");
        s.push_str(&init);
        s.push_str("</script>");
        s
    } else {
        String::new()
    };

    let base_href = match base_dir {
        Some(dir) => {
            let path_str = dir.to_string_lossy();
            // Keep '/' unescaped so the resulting URI is well-formed.
            let escaped = glib::Uri::escape_string(&path_str, Some("/"), false).to_string();
            format!("file://{}/", escaped)
        }
        None => String::new(),
    };

    let theme_css = if dark {
        PREVIEW_CSS_DARK
    } else {
        PREVIEW_CSS_LIGHT
    };
    let title_safe = if title.is_empty() {
        APP_NAME.to_string()
    } else {
        html_escape(title)
    };

    HTML_TEMPLATE
        .replace("{TITLE}", &title_safe)
        .replace("{THEME_CSS}", theme_css)
        .replace("{BASE_CSS}", PREVIEW_CSS_BASE)
        .replace("{BASE_HREF}", &base_href)
        .replace("{BODY}", &body)
        .replace("{MERMAID_SCRIPT}", &mermaid_script)
        .replace("{IMAGE_CLICK_JS}", IMAGE_CLICK_JS)
}

// ---- App state --------------------------------------------------------------
struct StateInner {
    window: adw::ApplicationWindow,
    webview: webkit6::WebView,
    buffer: sourceview5::Buffer,
    source_view: sourceview5::View,
    stack: gtk::Stack,
    toast_overlay: adw::ToastOverlay,
    toggle_btn: gtk::ToggleButton,
    toggle_icon: gtk::Image,
    toggle_label: gtk::Label,
    status_path: gtk::Label,
    status_mtime: gtk::Label,
    status_mode: gtk::Label,

    current_file: RefCell<Option<PathBuf>>,
    is_modified: Cell<bool>,
    mode: RefCell<String>,
    suppress_modify: Cell<bool>,
    // One-shot: set by reload_from_disk, taken by the next refresh_preview
    // to mark changed blocks with a left-border + hover tooltip showing
    // the prior content and how long ago the edit happened. Cleared on any
    // open/new so stale change marks can't leak across files.
    pending_changes: RefCell<Option<PendingChanges>>,

    // Git history rail state. `git_history` is the commit list for the
    // current file (None if the file isn't in a git repo).
    // `viewing_snapshot` holds the historical revision the preview is
    // rendering from, or None for the working copy. `history_visible`
    // toggles the rail on/off.
    git_history: RefCell<Option<Vec<Commit>>>,
    viewing_snapshot: RefCell<Option<HistorySnapshot>>,
    history_visible: Cell<bool>,

    // Set by `handle_table_navigate` and consumed by the next
    // `refresh_preview`: tells the WebView which cell to focus
    // (programmatically click) once the new HTML loads, so Tab/Enter
    // navigation between cells feels continuous to the user.
    pending_focus_cell: RefCell<Option<(tables::TableId, i32, usize)>>,

    // Per-table sort state — stores the snapshot of pre-sort rows
    // plus the current (col, direction). Consulted by:
    //   - `handle_table_sort` for the tri-state cycle (Off restores
    //     from the snapshot)
    //   - `refresh_preview` to hydrate `sort_indicator` on each
    //     parsed MarkdownTable so the post-processor emits
    //     `data-sort-dir` on the active header cell
    // Cleared when a structural / cell edit invalidates the snapshot.
    table_sort_snapshots: RefCell<HashMap<tables::TableId, TableSortSnapshot>>,
    toggle_handler: RefCell<Option<glib::SignalHandlerId>>,
    buffer_handler: RefCell<Option<glib::SignalHandlerId>>,

    // External-change watcher: notify::Watcher runs on its own thread; the
    // glib timeout pumps events onto the GTK main thread and coalesces
    // bursts. last_self_write is bumped before our own atomic save so the
    // resulting inotify event doesn't bounce back as an "external change."
    watcher: RefCell<Option<notify::RecommendedWatcher>>,
    watch_source_id: RefCell<Option<glib::SourceId>>,
    last_self_write: Cell<Instant>,
}

#[derive(Clone)]
struct State {
    inner: Rc<StateInner>,
}

impl State {
    fn new(app: &adw::Application) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(APP_NAME)
            .default_width(1100)
            .default_height(780)
            .build();

        let webview = webkit6::WebView::new();
        let buffer = sourceview5::Buffer::new(None);
        let source_view = sourceview5::View::with_buffer(&buffer);
        let stack = gtk::Stack::new();
        let toast_overlay = adw::ToastOverlay::new();
        let toggle_btn = gtk::ToggleButton::new();
        let toggle_icon = gtk::Image::from_icon_name("document-edit-symbolic");
        let toggle_label = gtk::Label::new(Some("Edit"));
        let status_path = gtk::Label::new(None);
        let status_mtime = gtk::Label::new(None);
        let status_mode = gtk::Label::new(None);

        let inner = StateInner {
            window,
            webview,
            buffer,
            source_view,
            stack,
            toast_overlay,
            toggle_btn,
            toggle_icon,
            toggle_label,
            status_path,
            status_mtime,
            status_mode,
            current_file: RefCell::new(None),
            is_modified: Cell::new(false),
            mode: RefCell::new(MODE_PREVIEW.to_string()),
            suppress_modify: Cell::new(false),
            pending_changes: RefCell::new(None),
            git_history: RefCell::new(None),
            viewing_snapshot: RefCell::new(None),
            history_visible: Cell::new(true),
            pending_focus_cell: RefCell::new(None),
            table_sort_snapshots: RefCell::new(HashMap::new()),
            toggle_handler: RefCell::new(None),
            buffer_handler: RefCell::new(None),
            watcher: RefCell::new(None),
            watch_source_id: RefCell::new(None),
            last_self_write: Cell::new(Instant::now()),
        };

        State {
            inner: Rc::new(inner),
        }
    }

    // -- UI --------------------------------------------------------------
    fn build_ui(&self) {
        let s = &self.inner;

        let toolbar_view = adw::ToolbarView::new();
        s.window.set_content(Some(&toolbar_view));

        // HeaderBar
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        // Left side: New / Open / Save
        let new_btn = gtk::Button::from_icon_name("document-new-symbolic");
        new_btn.set_tooltip_text(Some("New (Ctrl+N)"));
        new_btn.connect_clicked(clone!(
            #[strong(rename_to = state)]
            self,
            move |_| state.action_new()
        ));
        header.pack_start(&new_btn);

        let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
        open_btn.set_tooltip_text(Some("Open\u{2026} (Ctrl+O)"));
        open_btn.connect_clicked(clone!(
            #[strong(rename_to = state)]
            self,
            move |_| state.action_open()
        ));
        header.pack_start(&open_btn);

        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some("Save (Ctrl+S)"));
        save_btn.connect_clicked(clone!(
            #[strong(rename_to = state)]
            self,
            move |_| state.action_save()
        ));
        header.pack_start(&save_btn);

        // Right: hamburger menu + toggle
        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("open-menu-symbolic");
        menu_btn.set_menu_model(Some(&self.build_menu_model()));
        menu_btn.set_tooltip_text(Some("Menu"));
        header.pack_end(&menu_btn);

        // Toggle button: prominent, suggested-action with icon + label
        s.toggle_btn.add_css_class("suggested-action");
        s.toggle_btn
            .set_tooltip_text(Some("Switch between Preview and Edit (F5)"));
        let toggle_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toggle_box.append(&s.toggle_icon);
        toggle_box.append(&s.toggle_label);
        s.toggle_btn.set_child(Some(&toggle_box));
        let handler_id = s.toggle_btn.connect_toggled(clone!(
            #[strong(rename_to = state)]
            self,
            move |btn| {
                let target = if btn.is_active() {
                    MODE_EDIT
                } else {
                    MODE_PREVIEW
                };
                state.set_mode(target, false);
            }
        ));
        *s.toggle_handler.borrow_mut() = Some(handler_id);
        header.pack_end(&s.toggle_btn);

        // Stack for content
        s.stack
            .set_transition_type(gtk::StackTransitionType::Crossfade);
        s.stack.set_transition_duration(140);
        s.toast_overlay.set_child(Some(&s.stack));
        toolbar_view.set_content(Some(&s.toast_overlay));

        // --- Preview page ---
        if let Some(ws) = webkit6::prelude::WebViewExt::settings(&s.webview) {
            ws.set_enable_developer_extras(false);
            ws.set_javascript_can_access_clipboard(false);
            ws.set_enable_javascript(true);
        }
        let bg = if self.is_dark() { "#1e1e2e" } else { "#ffffff" };
        if let Ok(rgba) = gdk::RGBA::parse(bg) {
            s.webview.set_background_color(&rgba);
        }
        s.webview.connect_decide_policy(clone!(
            #[weak(rename_to = window)]
            s.window,
            #[upgrade_or]
            false,
            move |_, decision, decision_type| on_webview_policy(&window, decision, decision_type)
        ));
        // (function below takes references — matches the signal signature.)
        let preview_scroll = gtk::ScrolledWindow::new();
        preview_scroll.set_child(Some(&s.webview));
        preview_scroll.set_hexpand(true);
        preview_scroll.set_vexpand(true);
        s.stack.add_named(&preview_scroll, Some(MODE_PREVIEW));

        // --- Edit page (GtkSourceView 5) ---
        let lm = sourceview5::LanguageManager::default();
        if let Some(lang) = lm.language("markdown") {
            s.buffer.set_language(Some(&lang));
        }
        s.buffer.set_highlight_syntax(true);
        s.buffer.set_highlight_matching_brackets(true);
        self.apply_source_style_scheme();
        let buffer_handler_id = s.buffer.connect_changed(clone!(
            #[strong(rename_to = state)]
            self,
            move |_| state.on_buffer_changed()
        ));
        *s.buffer_handler.borrow_mut() = Some(buffer_handler_id);

        s.source_view.set_monospace(true);
        s.source_view.set_show_line_numbers(true);
        s.source_view.set_highlight_current_line(true);
        s.source_view.set_auto_indent(true);
        s.source_view.set_indent_on_tab(true);
        s.source_view
            .set_smart_home_end(sourceview5::SmartHomeEndType::Before);
        s.source_view.set_wrap_mode(gtk::WrapMode::WordChar);
        s.source_view.set_tab_width(4);
        s.source_view.set_insert_spaces_instead_of_tabs(true);
        s.source_view.set_top_margin(12);
        s.source_view.set_bottom_margin(12);
        s.source_view.set_left_margin(16);
        s.source_view.set_right_margin(16);

        let edit_scroll = gtk::ScrolledWindow::new();
        edit_scroll.set_child(Some(&s.source_view));
        edit_scroll.set_hexpand(true);
        edit_scroll.set_vexpand(true);
        s.stack.add_named(&edit_scroll, Some(MODE_EDIT));

        // Status bar
        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        status_bar.add_css_class("toolbar");
        status_bar.set_margin_start(10);
        status_bar.set_margin_end(10);
        status_bar.set_margin_top(4);
        status_bar.set_margin_bottom(4);

        s.status_path.set_xalign(0.0);
        s.status_path.set_hexpand(true);
        s.status_path.set_ellipsize(pango::EllipsizeMode::Middle);
        s.status_path.add_css_class("dim-label");
        s.status_mtime.add_css_class("dim-label");
        s.status_mode.add_css_class("dim-label");
        status_bar.append(&s.status_path);
        status_bar.append(&s.status_mtime);
        status_bar.append(&s.status_mode);
        toolbar_view.add_bottom_bar(&status_bar);

        // Track theme changes live
        let style_manager = adw::StyleManager::default();
        style_manager.connect_dark_notify(clone!(
            #[strong(rename_to = state)]
            self,
            move |_| state.on_theme_changed()
        ));

        // Confirm-on-close
        s.window.connect_close_request(clone!(
            #[strong(rename_to = state)]
            self,
            move |_| state.on_close_request()
        ));
    }

    fn build_menu_model(&self) -> gio::Menu {
        let menu = gio::Menu::new();

        let file_section = gio::Menu::new();
        file_section.append(Some("New"), Some("win.new"));
        file_section.append(Some("Open\u{2026}"), Some("win.open"));
        file_section.append(Some("Save"), Some("win.save"));
        file_section.append(Some("Save As\u{2026}"), Some("win.save-as"));
        menu.append_section(Some("File"), &file_section);

        let export_section = gio::Menu::new();
        export_section.append(Some("Export as HTML\u{2026}"), Some("win.export-html"));
        export_section.append(Some("Export as PDF\u{2026}"), Some("win.export-pdf"));
        menu.append_section(Some("Export"), &export_section);

        let edit_section = gio::Menu::new();
        edit_section.append(Some("Undo"), Some("win.undo"));
        edit_section.append(Some("Redo"), Some("win.redo"));
        menu.append_section(Some("Edit"), &edit_section);

        let view_section = gio::Menu::new();
        view_section.append(Some("Toggle Preview / Edit"), Some("win.toggle"));
        view_section.append(Some("Toggle Git History"), Some("win.toggle-history"));
        menu.append_section(Some("View"), &view_section);

        let help_section = gio::Menu::new();
        help_section.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        help_section.append(Some(&format!("About {}", APP_NAME)), Some("win.about"));
        menu.append_section(Some("Help"), &help_section);

        menu
    }

    #[allow(clippy::type_complexity)] // local action table; factoring out hurts readability
    fn wire_actions(&self, app: &adw::Application) {
        let s = &self.inner;

        let actions: Vec<(&str, Box<dyn Fn(&Self)>, &[&str])> = vec![
            (
                "new",
                Box::new(|st: &State| st.action_new()),
                &["<Primary>n"],
            ),
            (
                "open",
                Box::new(|st: &State| st.action_open()),
                &["<Primary>o"],
            ),
            (
                "save",
                Box::new(|st: &State| st.action_save()),
                &["<Primary>s"],
            ),
            (
                "save-as",
                Box::new(|st: &State| st.action_save_as()),
                &["<Primary><Shift>s"],
            ),
            (
                "export-html",
                Box::new(|st: &State| st.action_export_html()),
                &[],
            ),
            (
                "export-pdf",
                Box::new(|st: &State| st.action_export_pdf()),
                &[],
            ),
            (
                "toggle",
                Box::new(|st: &State| st.action_toggle()),
                &["F5", "<Primary><Shift>e"],
            ),
            ("undo", Box::new(|st: &State| st.do_undo()), &["<Primary>z"]),
            (
                "redo",
                Box::new(|st: &State| st.do_redo()),
                &["<Primary><Shift>z", "<Primary>y"],
            ),
            ("about", Box::new(|st: &State| st.action_about()), &[]),
            (
                "shortcuts",
                Box::new(|st: &State| st.action_shortcuts()),
                &["<Primary>question"],
            ),
            (
                "toggle-history",
                Box::new(|st: &State| st.action_toggle_history()),
                &["<Primary><Alt>h"],
            ),
        ];

        for (name, cb, accels) in actions {
            let action = gio::SimpleAction::new(name, None);
            let state_clone = self.clone();
            action.connect_activate(move |_, _| cb(&state_clone));
            s.window.add_action(&action);
            if !accels.is_empty() {
                app.set_accels_for_action(&format!("win.{}", name), accels);
            }
        }

        // Quit / close: special-case so we go through the close-request flow.
        let quit_action = gio::SimpleAction::new("quit", None);
        quit_action.connect_activate(clone!(
            #[strong(rename_to = state)]
            self,
            move |_, _| {
                state.inner.window.close();
            }
        ));
        s.window.add_action(&quit_action);
        app.set_accels_for_action("win.quit", &["<Primary>q", "<Primary>w"]);
    }

    // -- Image paste -----------------------------------------------------
    // Intercept Ctrl+V on the source view: if the clipboard has an image,
    // save it next to the .md file and insert a markdown reference. Falls
    // back to the standard text paste when there's no image (which is the
    // overwhelmingly common case).
    fn setup_image_paste(&self) {
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let st = self.clone();
        key.connect_key_pressed(move |_, key, _kc, modifiers| {
            // Strip lock modifiers so Caps/Num lock don't break the match.
            let m = modifiers
                & (gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK);
            if m == gdk::ModifierType::CONTROL_MASK && (key == gdk::Key::v || key == gdk::Key::V) {
                st.try_paste_image_or_text();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.inner.source_view.add_controller(key);
    }

    fn try_paste_image_or_text(&self) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let st = self.clone();
        let clipboard_for_fallback = clipboard.clone();
        clipboard.read_texture_async(None::<&gio::Cancellable>, move |result| match result {
            Ok(Some(texture)) => st.paste_image_to_buffer(&texture),
            _ => st.try_paste_table_or_text(clipboard_for_fallback),
        });
    }

    /// After an image read returns nothing, peek at the clipboard's
    /// text. If it looks like tabular data (TSV / CSV / existing GFM
    /// table), convert and insert as a clean GFM table — otherwise
    /// fall through to the buffer's normal text paste.
    ///
    /// Shift+Ctrl+V (the GTK "paste as plain text" shortcut) bypasses
    /// this handler entirely, so users always have an escape hatch.
    fn try_paste_table_or_text(&self, clipboard: gdk::Clipboard) {
        let st = self.clone();
        let cb_for_paste = clipboard.clone();
        clipboard.read_text_async(None::<&gio::Cancellable>, move |result| {
            let text = result.ok().flatten().map(|s| s.to_string());
            if let Some(text) = text.as_deref() {
                if st.try_paste_table(text) {
                    return;
                }
            }
            // Not table-shaped → fall back to GTK's text paste path,
            // which respects the user's editing context exactly as
            // before (cursor, selection, undo grouping).
            st.inner.buffer.paste_clipboard(&cb_for_paste, None, true);
        });
    }

    /// Detect, convert, and insert a clipboard payload as a GFM table.
    /// Returns true if smart-paste handled the content, false to let
    /// the caller fall back to normal text paste.
    fn try_paste_table(&self, text: &str) -> bool {
        let detected = tables::detect_table_paste(Some(text), None);
        if matches!(detected, tables::TablePaste::None) {
            return false;
        }
        let Some(md) = tables::paste_to_gfm(&detected, tables::TableStyle::Pretty) else {
            return false;
        };
        let (rows, cols) = match detected.shape() {
            Some(s) => s,
            None => return false,
        };
        let origin_label = match detected {
            tables::TablePaste::Tsv { .. } => "TSV",
            tables::TablePaste::Csv { .. } => "CSV",
            tables::TablePaste::Gfm { .. } => "Markdown",
            tables::TablePaste::Html { .. } => "HTML table",
            tables::TablePaste::None => return false,
        };
        self.insert_table_at_cursor(&md);
        // Body cells excluding header for the user-facing count.
        let body_rows = rows.saturating_sub(1);
        self.show_toast(&format!(
            "Pasted as table ({body_rows}×{cols} from {origin_label}) — Ctrl+Z to undo"
        ));
        true
    }

    /// Insert a freshly converted GFM table at the cursor with proper
    /// blank-line padding. Wraps the insert in a single user-action so
    /// Ctrl+Z reverts the entire paste in one step.
    fn insert_table_at_cursor(&self, md: &str) {
        let buf = &self.inner.buffer;
        let mark = buf.mark("insert").or_else(|| Some(buf.get_insert()));
        let Some(mark) = mark else {
            return;
        };
        let iter = buf.iter_at_mark(&mark);

        // Determine how much leading whitespace to add. Goal: the
        // table always lands on its own line with a blank line above
        // (so GFM parses it as a block) — but don't add extra blank
        // lines if the cursor is already at the start of a blank line.
        let at_line_start = iter.line_offset() == 0;
        let at_doc_start = iter.offset() == 0;
        let prev_line_blank = if at_line_start && !at_doc_start {
            // Look at the immediately-preceding line.
            let mut probe = iter;
            probe.backward_char(); // step into the previous newline
            probe.set_line_offset(0);
            let mut end_of_prev = probe;
            end_of_prev.forward_to_line_end();
            buf.text(&probe, &end_of_prev, false).trim().is_empty()
        } else {
            // Pretend doc start counts as "blank prev line" — no extra
            // padding needed when we're at the very top.
            at_doc_start
        };

        let mut prefix = String::new();
        if !at_line_start {
            prefix.push('\n'); // break out of the current line
            prefix.push('\n'); // blank separator
        } else if !prev_line_blank {
            prefix.push('\n'); // blank separator above
        }

        let mut insertion = prefix;
        insertion.push_str(md.trim_end_matches('\n'));
        insertion.push('\n');
        insertion.push('\n'); // trailing blank line so following content stays a block

        self.inner.suppress_modify.set(true);
        buf.begin_user_action();
        let mut iter = buf.iter_at_mark(&mark);
        buf.insert(&mut iter, &insertion);
        buf.end_user_action();
        self.inner.suppress_modify.set(false);

        if !self.inner.is_modified.get() {
            self.inner.is_modified.set(true);
            self.update_title();
        }
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    fn paste_image_to_buffer(&self, texture: &gdk::Texture) {
        let path = match self.inner.current_file.borrow().clone() {
            Some(p) => p,
            None => {
                self.show_toast("Save the document first to paste images");
                return;
            }
        };
        let parent = match path.parent() {
            Some(p) => p.to_path_buf(),
            None => return,
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "doc".to_string());
        let assets_dir_name = format!("{stem}-assets");
        let assets_dir = parent.join(&assets_dir_name);
        if let Err(e) = fs::create_dir_all(&assets_dir) {
            self.error_dialog("Couldn't save pasted image", &e.to_string());
            return;
        }
        let stamp = glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("%Y%m%d-%H%M%S").ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "image".to_string());
        let filename = format!("paste-{stamp}.png");
        let img_path = assets_dir.join(&filename);
        let png_bytes = texture.save_to_png_bytes();
        if let Err(e) = fs::write(&img_path, png_bytes.as_ref()) {
            self.error_dialog("Couldn't save pasted image", &e.to_string());
            return;
        }
        let rel_path = format!("{assets_dir_name}/{filename}");
        // URL-encode spaces / unusual chars so the path is safe inside
        // markdown's `()`.
        let encoded = glib::Uri::escape_string(&rel_path, Some("/-._~"), false).to_string();
        let md_ref = format!("![]({encoded})");
        self.inner.buffer.insert_at_cursor(&md_ref);
        self.show_toast(&format!("Image saved: {rel_path}"));
    }

    // -- Image click in preview ------------------------------------------
    // The IMAGE_CLICK_JS in the WebView posts a tab-delimited message
    // (src\twidth\talt) when the user clicks an image. Open a small dialog
    // to edit width / alt or remove the image; on apply we rewrite the
    // markdown source in the buffer.
    fn setup_image_click_handler(&self) {
        let manager = self.inner.webview.user_content_manager();
        let manager = match manager {
            Some(m) => m,
            None => return,
        };
        manager.register_script_message_handler("imageClick", None);
        manager.register_script_message_handler("imageResize", None);
        manager.register_script_message_handler("imageMove", None);
        manager.register_script_message_handler("commitClick", None);
        manager.register_script_message_handler("toggleHistory", None);
        manager.register_script_message_handler("scrollTo", None);
        manager.register_script_message_handler("tableEdit", None);
        manager.register_script_message_handler("tableNavigate", None);
        manager.register_script_message_handler("tableStructure", None);
        manager.register_script_message_handler("tableSort", None);
        manager.register_script_message_handler("tableResizeColumns", None);

        let st = self.clone();
        manager.connect_script_message_received(Some("imageClick"), move |_, value| {
            st.handle_image_click(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("imageResize"), move |_, value| {
            st.handle_image_resize(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("imageMove"), move |_, value| {
            st.handle_image_move(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("commitClick"), move |_, value| {
            st.handle_commit_click(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("toggleHistory"), move |_, _value| {
            st.action_toggle_history();
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("tableEdit"), move |_, value| {
            st.handle_table_edit(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("tableNavigate"), move |_, value| {
            st.handle_table_navigate(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("tableStructure"), move |_, value| {
            st.handle_table_structure(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("tableSort"), move |_, value| {
            st.handle_table_sort(&value.to_str());
        });
        let st = self.clone();
        manager.connect_script_message_received(Some("tableResizeColumns"), move |_, value| {
            st.handle_table_resize_columns(&value.to_str());
        });
    }

    /// Commit the column widths produced by the resize-handle drag.
    ///
    /// Payload is `table_id\twidths` where `widths` is a comma-
    /// separated list with empties for unset columns
    /// (e.g. `180,,120`). Stored as `<!-- rmd-cols: ... -->` in the
    /// markdown source by the serializer, and applied as a single
    /// undo step so Ctrl+Z reverses the whole drag.
    fn handle_table_resize_columns(&self, message: &str) {
        let mut parts = message.splitn(2, '\t');
        let table_id: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let widths_csv = parts.next().unwrap_or("");
        if table_id == 0 {
            return;
        }

        let widths: Vec<Option<u32>> = widths_csv
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

        let buffer_text = self.buffer_text();
        let mut tables_vec = tables::parse_tables(&buffer_text);
        let table = match tables_vec.iter_mut().find(|t| t.id == table_id) {
            Some(t) => t,
            None => {
                self.show_toast("Couldn't locate that table — refreshing");
                if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                    self.refresh_preview();
                }
                return;
            }
        };

        if widths.len() != table.alignments.len() {
            self.show_toast(&format!(
                "Resize ignored: got {} widths, table has {} columns",
                widths.len(),
                table.alignments.len()
            ));
            return;
        }
        if widths == table.column_widths {
            // No-op drag — nothing to persist.
            return;
        }

        let mut shadow = buffer_text.clone();
        let delta = match table.set_column_widths(widths, &mut shadow) {
            Ok(d) => d,
            Err(e) => {
                self.show_toast(&format!("Resize failed: {e}"));
                return;
            }
        };
        self.apply_buffer_patch(&buffer_text, &shadow, &delta);
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    /// Apply a sort operation triggered by the toolbar's tri-state
    /// sort button.
    ///
    /// Payload is `table_id\tcol\tdirection` where `direction` is
    /// `asc` | `desc` | `off`. (Row is unused — sort is per-column.)
    ///
    /// Snapshot lifecycle:
    ///   - First Asc/Desc click on a previously-unsorted table:
    ///     snapshot current rows into `table_sort_snapshots`, then
    ///     sort.
    ///   - Subsequent Asc/Desc clicks on the same table: re-sort
    ///     from the snapshot (so Desc isn't just "reverse of Asc" —
    ///     it's a Desc sort of the original, stable for ties).
    ///   - Off: restore from snapshot and remove the entry. If the
    ///     snapshot was invalidated by an intervening edit, Off is
    ///     a no-op with an informational toast.
    fn handle_table_sort(&self, message: &str) {
        let mut parts = message.splitn(3, '\t');
        let table_id: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let col: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let dir_str = parts.next().unwrap_or("");
        if table_id == 0 {
            return;
        }
        let direction = match dir_str {
            "asc" => tables::model::SortDirection::Ascending,
            "desc" => tables::model::SortDirection::Descending,
            "off" => tables::model::SortDirection::None,
            _ => return,
        };

        let buffer_text = self.buffer_text();
        let mut tables_vec = tables::parse_tables(&buffer_text);
        let table = match tables_vec.iter_mut().find(|t| t.id == table_id) {
            Some(t) => t,
            None => {
                self.show_toast("Couldn't locate that table — refreshing");
                if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                    self.refresh_preview();
                }
                return;
            }
        };
        if col >= table.alignments.len() {
            self.show_toast("Column out of range for sort");
            return;
        }

        let new_rows: Vec<Vec<tables::model::Cell>> = match direction {
            tables::model::SortDirection::None => {
                let mut snapshots = self.inner.table_sort_snapshots.borrow_mut();
                match snapshots.remove(&table_id) {
                    Some(snap) => {
                        self.show_toast(&format!(
                            "Sort cleared — column {} restored to original order",
                            snap.col + 1
                        ));
                        snap.original_rows
                    }
                    None => {
                        // No snapshot — either Off was clicked without a
                        // prior sort, or an intervening edit invalidated
                        // the snapshot.
                        self.show_toast(
                            "Original row order not tracked (cleared by a recent edit)",
                        );
                        return;
                    }
                }
            }
            tables::model::SortDirection::Ascending | tables::model::SortDirection::Descending => {
                // Use the existing snapshot's rows as the baseline if
                // we already have one (so Desc is always "desc of
                // original" rather than "reverse of asc"). Otherwise,
                // snapshot the current rows.
                let baseline = {
                    let snapshots = self.inner.table_sort_snapshots.borrow();
                    snapshots
                        .get(&table_id)
                        .map(|s| s.original_rows.clone())
                        .unwrap_or_else(|| table.rows.clone())
                };
                let sorted = tables::model::sort_rows(baseline.clone(), col, direction);
                self.inner.table_sort_snapshots.borrow_mut().insert(
                    table_id,
                    TableSortSnapshot {
                        original_rows: baseline,
                        col,
                        direction,
                    },
                );
                let dir_label = if matches!(direction, tables::model::SortDirection::Ascending) {
                    "ascending"
                } else {
                    "descending"
                };
                self.show_toast(&format!("Sorted by column {} ({})", col + 1, dir_label));
                sorted
            }
        };

        let mut shadow = buffer_text.clone();
        let delta = match table.replace_rows(new_rows, &mut shadow) {
            Ok(d) => d,
            Err(e) => {
                self.show_toast(&format!("Sort failed: {e}"));
                return;
            }
        };
        self.apply_buffer_patch(&buffer_text, &shadow, &delta);

        // Stay on the header cell the user clicked.
        *self.inner.pending_focus_cell.borrow_mut() = Some((table_id, -1, col));

        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    /// Apply a structural edit (insert/delete row, insert/delete
    /// column) to the table the click-to-edit cell belongs to.
    ///
    /// Payload is tab-delimited `table_id\trow\tcol\top` where `op`
    /// is one of `row-above` | `row-below` | `col-left` | `col-right`
    /// | `row-delete` | `col-delete`. The `(row, col)` is the cell
    /// that was focused when the user triggered the op.
    ///
    /// After the structural change, computes a sensible post-op
    /// focus cell and stashes it in `pending_focus_cell` so the next
    /// `refresh_preview` lands the user on a meaningful cell (e.g.
    /// after "Insert row below" they stay on the same row index;
    /// after "Delete row" they move to whatever is now in that
    /// slot, or the previous row when they deleted the last one).
    fn handle_table_structure(&self, message: &str) {
        let mut parts = message.splitn(4, '\t');
        let table_id: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let row: i32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let col: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let op = parts.next().unwrap_or("");
        if table_id == 0 || op.is_empty() {
            return;
        }

        let buffer_text = self.buffer_text();
        let mut tables_vec = tables::parse_tables(&buffer_text);
        let table = match tables_vec.iter_mut().find(|t| t.id == table_id) {
            Some(t) => t,
            None => {
                self.show_toast("Couldn't locate that table — refreshing");
                if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                    self.refresh_preview();
                }
                return;
            }
        };

        // `row == -1` indicates the header. For "row-below" the
        // insertion index is 0 (start of body); for "row-above"
        // we refuse — the header isn't a "row" the user can insert
        // above. row-delete on the header is also refused.
        let body_row = row.max(0) as usize;
        // Row/column structural ops change the row set; alignment +
        // reformat leave row order intact. Invalidate the sort
        // snapshot only for the former — otherwise "Off" should
        // still work after an alignment tweak.
        if matches!(
            op,
            "row-above" | "row-below" | "col-left" | "col-right" | "row-delete" | "col-delete"
        ) {
            self.inner
                .table_sort_snapshots
                .borrow_mut()
                .remove(&table_id);
        }
        let mut shadow = buffer_text.clone();
        let result = match op {
            "row-above" => {
                if row == -1 {
                    self.show_toast("Can't insert a row above the header");
                    return;
                }
                table.insert_empty_row(body_row, &mut shadow)
            }
            "row-below" => {
                let at = if row == -1 { 0 } else { body_row + 1 };
                table.insert_empty_row(at, &mut shadow)
            }
            "col-left" => table.insert_column(col, tables::model::Alignment::None, &mut shadow),
            "col-right" => {
                table.insert_column(col + 1, tables::model::Alignment::None, &mut shadow)
            }
            "row-delete" => {
                if row == -1 {
                    self.show_toast("Can't delete the header row");
                    return;
                }
                table.delete_row(body_row, &mut shadow)
            }
            "col-delete" => table.delete_column(col, &mut shadow),
            "align-left" => {
                table.set_column_alignment(col, tables::model::Alignment::Left, &mut shadow)
            }
            "align-center" => {
                table.set_column_alignment(col, tables::model::Alignment::Center, &mut shadow)
            }
            "align-right" => {
                table.set_column_alignment(col, tables::model::Alignment::Right, &mut shadow)
            }
            "align-none" => {
                table.set_column_alignment(col, tables::model::Alignment::None, &mut shadow)
            }
            "reformat" => table.reformat_pretty(&mut shadow),
            _ => return,
        };

        let delta = match result {
            Ok(d) => d,
            Err(e) => {
                self.show_toast(&format!("Operation failed: {e}"));
                return;
            }
        };

        // `set_column_alignment` signals a no-op (clicking the
        // already-active alignment) with an empty patched_range —
        // skip the buffer patch and the refresh entirely.
        if delta.patched_range.is_empty() && delta.byte_delta == 0 {
            return;
        }

        self.apply_buffer_patch(&buffer_text, &shadow, &delta);

        // Where should focus land after this op? Use the model's
        // *post-op* sizes (table has been mutated in place by the
        // op above).
        let n_cols = table.alignments.len();
        let n_body = table.rows.len() as i32;
        let focus: Option<(i32, usize)> = match op {
            "row-above" => Some((row + 1, col)),
            "row-below" => {
                // If we were on the header, land in the new first body row.
                let target = if row == -1 { 0 } else { row };
                Some((target, col))
            }
            "col-left" => Some((row, col + 1)),
            "col-right" => Some((row, col)),
            "row-delete" => {
                if n_body == 0 {
                    Some((-1, col)) // header-only table now
                } else {
                    let new_row = row.min(n_body - 1);
                    Some((new_row, col))
                }
            }
            "col-delete" => {
                if n_cols == 0 {
                    None
                } else {
                    let new_col = col.min(n_cols - 1);
                    Some((row, new_col))
                }
            }
            "align-left" | "align-center" | "align-right" | "align-none" => {
                // Alignment doesn't change cell positions — stay on the
                // header cell the user clicked.
                Some((row, col))
            }
            "reformat" => {
                // Cells keep their (row, col) addresses across a
                // reformat; stay on the same one. Also surface a
                // brief toast so the user sees what just happened.
                self.show_toast(&format!(
                    "Table reformatted to pretty style ({} column{})",
                    n_cols,
                    if n_cols == 1 { "" } else { "s" }
                ));
                Some((row, col))
            }
            _ => None,
        };
        if let Some((fr, fc)) = focus {
            *self.inner.pending_focus_cell.borrow_mut() = Some((table_id, fr, fc));
        }

        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    /// Splice a table-subsystem patch into the GtkTextBuffer as a
    /// single atomic edit. Used by both `handle_table_edit` and
    /// `handle_table_navigate` so undo treats each cell change /
    /// row insert as one Ctrl+Z step.
    fn apply_buffer_patch(&self, old_text: &str, new_text: &str, delta: &tables::EditDelta) {
        let char_start = old_text[..delta.patched_range.start].chars().count() as i32;
        let char_end = old_text[..delta.patched_range.end].chars().count() as i32;
        let replacement = &new_text[delta.new_range.clone()];

        let buf = &self.inner.buffer;
        self.inner.suppress_modify.set(true);
        buf.begin_user_action();
        let mut start = buf.iter_at_offset(char_start);
        let mut end = buf.iter_at_offset(char_end);
        buf.delete(&mut start, &mut end);
        let mut at = buf.iter_at_offset(char_start);
        buf.insert(&mut at, replacement);
        buf.end_user_action();
        self.inner.suppress_modify.set(false);

        if !self.inner.is_modified.get() {
            self.inner.is_modified.set(true);
            self.update_title();
        }
    }

    /// Process a Tab / Shift+Tab / Enter / Shift+Enter press inside an
    /// editable table cell.
    ///
    /// Payload is tab-delimited `table_id\trow\tcol\tdirection` where
    /// `direction` is one of `next` | `prev` | `down` | `up`.
    ///
    /// Flow:
    ///   1. Re-parse tables, find the requested table by id.
    ///   2. Ask the model where to go via `MarkdownTable::navigate`.
    ///   3. If the target requires a new row, call `insert_empty_row`
    ///      and splice the resulting patch into the buffer.
    ///   4. Stash `(table_id, row, col)` in `pending_focus_cell` so
    ///      the *next* `refresh_preview` injects a one-shot script
    ///      that programmatically clicks the target cell — which the
    ///      existing click handler treats as a normal beginEdit.
    fn handle_table_navigate(&self, message: &str) {
        let mut parts = message.splitn(4, '\t');
        let table_id: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let row: i32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let col: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let direction_str = parts.next().unwrap_or("");
        if table_id == 0 {
            return;
        }
        let direction = match direction_str {
            "next" => tables::model::NavDirection::Next,
            "prev" => tables::model::NavDirection::Prev,
            "down" => tables::model::NavDirection::Down,
            "up" => tables::model::NavDirection::Up,
            _ => return,
        };

        let buffer_text = self.buffer_text();
        let mut tables_vec = tables::parse_tables(&buffer_text);
        let table = match tables_vec.iter_mut().find(|t| t.id == table_id) {
            Some(t) => t,
            None => {
                self.show_toast("Couldn't locate that table — refreshing");
                if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                    self.refresh_preview();
                }
                return;
            }
        };

        let target = table.navigate(row, col, direction);

        if target.created_row {
            let mut shadow = buffer_text.clone();
            match table.insert_empty_row(target.row as usize, &mut shadow) {
                Ok(delta) => self.apply_buffer_patch(&buffer_text, &shadow, &delta),
                Err(e) => {
                    self.show_toast(&format!("Couldn't add row: {e}"));
                    return;
                }
            }
        }

        *self.inner.pending_focus_cell.borrow_mut() = Some((table_id, target.row, target.col));

        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    /// Apply a click-to-edit cell commit from the WebView.
    ///
    /// Payload is tab-delimited `table_id\trow\tcol\tcontent`,
    /// matching the existing image-handler protocol. The content
    /// portion may contain literal newlines (Ctrl+Enter soft break) —
    /// `splitn(4, '\t')` keeps them intact.
    ///
    /// The flow:
    ///   1. Re-parse tables from the current buffer (cheap; pulldown-cmark
    ///      is fast). Find the table whose id matches the click.
    ///   2. Compute the patch by running `MarkdownTable::update_cell` on
    ///      a *clone* of the buffer — gives us the precise byte range
    ///      that changed without touching the real buffer.
    ///   3. Apply the same patch to the `GtkTextBuffer` as a single
    ///      atomic edit (delete + insert wrapped in
    ///      `begin_user_action` / `end_user_action`) so undo treats
    ///      the whole cell change as one step.
    ///   4. Refresh the preview so the edited table re-renders with
    ///      fresh `data-*` attributes.
    fn handle_table_edit(&self, message: &str) {
        let mut parts = message.splitn(4, '\t');
        let table_id: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let row: i32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let col: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let content = parts.next().unwrap_or("");
        if table_id == 0 {
            return;
        }
        // A cell-content change invalidates any pre-sort snapshot
        // for this table — restoring the original order would
        // silently drop the user's edit.
        self.inner
            .table_sort_snapshots
            .borrow_mut()
            .remove(&table_id);

        let buffer_text = self.buffer_text();
        let mut tables = tables::parse_tables(&buffer_text);
        let table = match tables.iter_mut().find(|t| t.id == table_id) {
            Some(t) => t,
            None => {
                // Doc layout changed between render and click. Re-render
                // to resync the WebView with the source.
                self.show_toast("Couldn't locate that table — refreshing");
                if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                    self.refresh_preview();
                }
                return;
            }
        };

        // Compute the patch against a clone so we get the exact byte
        // range without mutating the real source.
        let mut shadow = buffer_text.clone();
        let delta = match table.update_cell(row, col, content, &mut shadow) {
            Ok(d) => d,
            Err(e) => {
                self.show_toast(&format!("Edit failed: {e}"));
                return;
            }
        };
        if delta.patched_range.start == delta.patched_range.end && delta.byte_delta == 0 {
            // No-op edit (content identical) — nothing to do.
            return;
        }
        self.apply_buffer_patch(&buffer_text, &shadow, &delta);
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    fn handle_commit_click(&self, sha: &str) {
        let sha = sha.trim();
        if sha.is_empty() {
            return;
        }

        // Click the active commit again to return to the working copy.
        let already_viewing = self
            .inner
            .viewing_snapshot
            .borrow()
            .as_ref()
            .map(|s| s.sha == sha)
            .unwrap_or(false);
        if already_viewing {
            *self.inner.viewing_snapshot.borrow_mut() = None;
            self.show_toast("Returned to working copy");
            self.refresh_preview();
            return;
        }

        let path = match self.inner.current_file.borrow().clone() {
            Some(p) => p,
            None => return,
        };

        let text = match fetch_revision_text(&path, sha) {
            Some(t) => t,
            None => {
                self.show_toast("Couldn't fetch that revision (file may have been renamed)");
                return;
            }
        };

        // Parent (if any) gives us a baseline for the diff markers.
        let parent_text =
            fetch_parent_sha(&path, sha).and_then(|psha| fetch_revision_text(&path, &psha));

        // Find the commit in our cached history to get its date + subject.
        let history = self.inner.git_history.borrow();
        let commit = history
            .as_ref()
            .and_then(|cs| cs.iter().find(|c| c.sha == sha))
            .cloned();
        drop(history);
        let commit_unix = commit
            .as_ref()
            .map(|c| iso_to_unix_secs(&c.iso_date))
            .unwrap_or(0);
        let toast_msg = match commit {
            Some(c) => format!("Viewing {} — {}", c.short_sha, c.subject),
            None => format!("Viewing {}", &sha[..sha.len().min(7)]),
        };

        // Wire up the change-marker pipeline against the parent text so
        // the user gets the same yellow-bar + hover-diff view they're
        // used to from external edits.
        if let Some(p_text) = parent_text.as_ref() {
            let changed = compute_changed_lines(p_text, &text);
            if !changed.is_empty() {
                *self.inner.pending_changes.borrow_mut() = Some(PendingChanges {
                    changed_lines: changed,
                    old_text: p_text.clone(),
                    reload_ts: commit_unix,
                });
            }
        }

        *self.inner.viewing_snapshot.borrow_mut() = Some(HistorySnapshot {
            sha: sha.to_string(),
            text,
            parent_text,
            commit_unix_secs: commit_unix,
        });

        self.show_toast(&toast_msg);
        self.refresh_preview();
    }

    fn handle_image_click(&self, message: &str) {
        let mut parts = message.splitn(3, '\t');
        let src = parts.next().unwrap_or("").to_string();
        let width = parts.next().unwrap_or("").to_string();
        let alt = parts.next().unwrap_or("").to_string();
        if src.is_empty() {
            return;
        }
        self.show_image_options_dialog(&src, &width, &alt);
    }

    fn handle_image_resize(&self, message: &str) {
        let mut parts = message.splitn(3, '\t');
        let src = parts.next().unwrap_or("").to_string();
        let width = parts.next().unwrap_or("").trim().to_string();
        let alt = parts.next().unwrap_or("").to_string();
        if src.is_empty() || width.is_empty() {
            return;
        }
        self.apply_image_change(&src, Some(&width), &alt, false);
    }

    fn handle_image_move(&self, message: &str) {
        let mut parts = message.splitn(2, '\t');
        let src = parts.next().unwrap_or("").to_string();
        let target_src = parts.next().unwrap_or("").to_string();
        if src.is_empty() {
            return;
        }
        self.move_image_before(&src, &target_src);
    }

    // Insert the dragged image immediately before another image
    // (identified by its src) in the source. Empty `target_src` means
    // "append at end". Always commits as a block-level paragraph, which
    // means inline-image groups get split when the user drops between
    // siblings — that's the only way markdown can express the new
    // ordering cleanly.
    fn move_image_before(&self, src: &str, target_src: &str) {
        let buffer = &self.inner.buffer;
        let refresh_after = self.inner.mode.borrow().as_str() == MODE_PREVIEW;

        let (s_iter, e_iter) = buffer.bounds();
        let text = buffer.text(&s_iter, &e_iter, true).to_string();
        let Some((char_offset, char_len)) = find_image_ref(&text, src) else {
            self.show_toast("Couldn't locate that image in the source");
            if refresh_after {
                self.refresh_preview();
            }
            return;
        };
        let bytes_start = char_to_byte_offset(&text, char_offset);
        let bytes_end = char_to_byte_offset(&text, char_offset + char_len);
        let image_expr = text[bytes_start..bytes_end].to_string();

        buffer.begin_user_action();

        // Delete the dragged image.
        let mut s = buffer.iter_at_offset(char_offset as i32);
        let mut e = buffer.iter_at_offset((char_offset + char_len) as i32);
        buffer.delete(&mut s, &mut e);

        // Compute the insertion point in the *modified* buffer.
        let (s_iter, e_iter) = buffer.bounds();
        let modified = buffer.text(&s_iter, &e_iter, true).to_string();

        let insert_char_offset = if target_src.is_empty() {
            modified.chars().count()
        } else {
            match find_image_ref(&modified, target_src) {
                Some((c_off, _)) => c_off,
                None => modified.chars().count(),
            }
        };

        // Pad with newlines so the moved image always lands as its own
        // paragraph. If we're splitting an inline image group, the
        // siblings end up in separate paragraphs above and below.
        let byte_idx = char_to_byte_offset(&modified, insert_char_offset);
        let before = &modified[..byte_idx];
        let after = &modified[byte_idx..];
        let prefix = if before.is_empty() || before.ends_with("\n\n") {
            String::new()
        } else if before.ends_with('\n') {
            "\n".to_string()
        } else {
            "\n\n".to_string()
        };
        let suffix = if after.is_empty() || after.starts_with("\n\n") {
            String::new()
        } else if after.starts_with('\n') {
            "\n".to_string()
        } else {
            "\n\n".to_string()
        };

        let insertion = format!("{prefix}{image_expr}{suffix}");
        let mut at = buffer.iter_at_offset(insert_char_offset as i32);
        buffer.insert(&mut at, &insertion);

        buffer.end_user_action();

        if refresh_after {
            self.refresh_preview();
        }
    }

    fn show_image_options_dialog(&self, src: &str, current_width: &str, current_alt: &str) {
        let dialog = adw::AlertDialog::builder().heading("Image options").build();
        dialog.set_body(src);

        // Form: alt + width inputs side by side.
        let form = gtk::Box::new(gtk::Orientation::Vertical, 12);
        form.set_margin_top(8);

        let alt_label = gtk::Label::new(Some("Alt text"));
        alt_label.set_xalign(0.0);
        let alt_entry = gtk::Entry::new();
        alt_entry.set_text(current_alt);
        alt_entry.set_hexpand(true);

        let width_label = gtk::Label::new(Some("Width (px, blank for auto)"));
        width_label.set_xalign(0.0);
        let width_entry = gtk::Entry::new();
        width_entry.set_text(current_width);
        width_entry.set_max_length(8);

        form.append(&alt_label);
        form.append(&alt_entry);
        form.append(&width_label);
        form.append(&width_entry);
        dialog.set_extra_child(Some(&form));

        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove");
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.add_response("apply", "Apply");
        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("cancel");

        let st = self.clone();
        let src_owned = src.to_string();
        dialog.connect_response(None, move |dlg, response| {
            match response {
                "apply" => {
                    let new_alt = alt_entry.text().to_string();
                    let raw_width = width_entry.text().to_string();
                    let new_width = raw_width.trim();
                    let new_width = if new_width.is_empty() {
                        None
                    } else {
                        Some(new_width.to_string())
                    };
                    st.apply_image_change(&src_owned, new_width.as_deref(), &new_alt, false);
                }
                "remove" => {
                    st.apply_image_change(&src_owned, None, "", true);
                }
                _ => {}
            }
            dlg.close();
        });
        dialog.present(Some(&self.inner.window));
    }

    fn apply_image_change(&self, src: &str, new_width: Option<&str>, new_alt: &str, remove: bool) {
        let buffer = &self.inner.buffer;
        let (start, end) = buffer.bounds();
        let text = buffer.text(&start, &end, true).to_string();
        let Some((char_start, char_len)) = find_image_ref(&text, src) else {
            self.show_toast("Couldn't locate that image in the source");
            return;
        };
        let replacement = if remove {
            String::new()
        } else {
            build_image_markup(src, new_width, new_alt)
        };
        let mut s = buffer.iter_at_offset(char_start as i32);
        let mut e = buffer.iter_at_offset((char_start + char_len) as i32);
        buffer.delete(&mut s, &mut e);
        if !replacement.is_empty() {
            let mut at = buffer.iter_at_offset(char_start as i32);
            buffer.insert(&mut at, &replacement);
        }
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    fn setup_drag_and_drop(&self) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(clone!(
            #[strong(rename_to = state)]
            self,
            move |_, value, _, _| {
                let Ok(file_list) = value.get::<gdk::FileList>() else {
                    return false;
                };
                let files = file_list.files();
                let Some(first) = files.first() else {
                    return false;
                };
                let Some(path) = first.path() else {
                    return false;
                };
                state.maybe_save_then(Box::new(clone!(
                    #[strong]
                    state,
                    move || state.open_path(&path)
                )));
                true
            }
        ));
        self.inner.window.add_controller(target);
    }

    // -- Mode switching --------------------------------------------------
    fn set_mode(&self, mode: &str, force: bool) {
        let s = &self.inner;
        if !force && s.mode.borrow().as_str() == mode {
            return;
        }
        match mode {
            MODE_PREVIEW => {
                self.refresh_preview();
                s.stack.set_visible_child_name(MODE_PREVIEW);
                s.toggle_icon.set_icon_name(Some("document-edit-symbolic"));
                s.toggle_label.set_text("Edit");
                s.status_mode.set_text("Preview");
            }
            MODE_EDIT => {
                s.stack.set_visible_child_name(MODE_EDIT);
                s.toggle_icon.set_icon_name(Some("view-reveal-symbolic"));
                s.toggle_label.set_text("Preview");
                s.status_mode.set_text("Edit");
                let view = s.source_view.clone();
                glib::idle_add_local_once(move || {
                    view.grab_focus();
                });
            }
            _ => return,
        }
        *s.mode.borrow_mut() = mode.to_string();

        // Sync the toggle button without recursing.
        if let Some(handler) = s.toggle_handler.borrow().as_ref() {
            s.toggle_btn.block_signal(handler);
            s.toggle_btn.set_active(mode == MODE_EDIT);
            s.toggle_btn.unblock_signal(handler);
        } else {
            s.toggle_btn.set_active(mode == MODE_EDIT);
        }
    }

    fn action_toggle(&self) {
        let next = if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            MODE_EDIT
        } else {
            MODE_PREVIEW
        };
        self.set_mode(next, false);
    }

    // -- Preview ---------------------------------------------------------
    fn refresh_preview(&self) {
        let s = &self.inner;
        // When viewing a historical revision, render the snapshot text
        // instead of the working copy. The buffer is left alone. Also
        // re-populate pending_changes from the snapshot so diff markers
        // persist across refreshes (theme switch, mode toggle, etc.).
        let mut text = match s.viewing_snapshot.borrow().as_ref() {
            Some(snap) => {
                if s.pending_changes.borrow().is_none() {
                    if let Some(parent) = &snap.parent_text {
                        let changed = compute_changed_lines(parent, &snap.text);
                        if !changed.is_empty() {
                            *s.pending_changes.borrow_mut() = Some(PendingChanges {
                                changed_lines: changed,
                                old_text: parent.clone(),
                                reload_ts: snap.commit_unix_secs,
                            });
                        }
                    }
                }
                snap.text.clone()
            }
            None => self.buffer_text(),
        };
        if let Some(changes) = s.pending_changes.borrow_mut().take() {
            text = inject_change_markers(&text, &changes, self.is_dark());
        }
        let current = s.current_file.borrow().clone();
        let base_dir: PathBuf = current
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let title = current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| APP_NAME.to_string());
        let html = render_markdown_to_html(&text, Some(&base_dir), self.is_dark(), &title);

        // Tables: parse from source (cheap) and post-process the
        // rendered HTML to inject data-* attributes + the click-to-
        // edit JS so cells become interactive in the live preview.
        // No-op when the doc has no tables.
        let mut parsed_tables = tables::parse_tables(&text);
        // Hydrate transient sort indicators from the per-document
        // snapshot map so the post-processor can emit data-sort-dir
        // on the active header cell.
        {
            let snapshots = self.inner.table_sort_snapshots.borrow();
            for t in &mut parsed_tables {
                if let Some(snap) = snapshots.get(&t.id) {
                    t.sort_indicator = Some((snap.col, snap.direction));
                }
            }
        }
        let html_with_tables = if parsed_tables.is_empty() {
            html
        } else {
            let injected = tables::render::inject_table_attrs(&html, &parsed_tables);
            injected.replacen(
                "</body>",
                &format!("{}\n</body>", tables::render::TABLE_EDIT_JS),
                1,
            )
        };

        // Inject the git history rail just before </body> when available.
        // Stays out of the way (no rail markup at all) when the file
        // isn't in a git repo.
        let final_html = match s.git_history.borrow().as_ref() {
            Some(commits) => {
                let viewing = s.viewing_snapshot.borrow().as_ref().map(|s| s.sha.clone());
                let rail =
                    build_history_rail_html(commits, viewing.as_deref(), s.history_visible.get());
                if rail.is_empty() {
                    html_with_tables
                } else {
                    html_with_tables.replacen("</body>", &format!("{}\n</body>", rail), 1)
                }
            }
            None => html_with_tables,
        };

        // Tab/Enter navigation landed us on a specific cell — inject a
        // one-shot script that calls `window.rmdFocusCell(...)` once
        // the page loads. The IIFE retries briefly because TABLE_EDIT_JS
        // may run a tick later than this script depending on load order.
        let final_html = if let Some((tid, r, c)) = s.pending_focus_cell.borrow_mut().take() {
            let focus_js = format!(
                "<script>(function(){{\n\
                  function tryFocus(retries){{\n\
                    var fn = window.rmdFocusCell;\n\
                    if (!fn) {{ if (retries > 0) setTimeout(function(){{ tryFocus(retries-1); }}, 30); return; }}\n\
                    fn({tid}, {r}, {c});\n\
                  }}\n\
                  if (document.readyState === \"loading\") {{\n\
                    document.addEventListener(\"DOMContentLoaded\", function(){{ tryFocus(10); }});\n\
                  }} else {{ tryFocus(10); }}\n\
                }})();</script>",
                tid = tid,
                r = r,
                c = c
            );
            final_html.replacen("</body>", &format!("{focus_js}\n</body>"), 1)
        } else {
            final_html
        };

        let path_str = base_dir.to_string_lossy();
        let escaped = glib::Uri::escape_string(&path_str, Some("/"), false).to_string();
        let base_uri = format!("file://{}/", escaped);
        s.webview.load_html(&final_html, Some(&base_uri));
    }

    // -- Theme -----------------------------------------------------------
    fn is_dark(&self) -> bool {
        adw::StyleManager::default().is_dark()
    }

    fn apply_source_style_scheme(&self) {
        let sm = sourceview5::StyleSchemeManager::default();
        let preferred: &[&str] = if self.is_dark() {
            &["Adwaita-dark", "solarized-dark", "oblivion"]
        } else {
            &["Adwaita", "solarized-light", "tango"]
        };
        for name in preferred {
            if let Some(scheme) = sm.scheme(name) {
                self.inner.buffer.set_style_scheme(Some(&scheme));
                return;
            }
        }
    }

    fn on_theme_changed(&self) {
        self.apply_source_style_scheme();
        let bg = if self.is_dark() { "#1e1e2e" } else { "#ffffff" };
        if let Ok(rgba) = gdk::RGBA::parse(bg) {
            self.inner.webview.set_background_color(&rgba);
        }
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    // -- File operations -------------------------------------------------
    fn action_new(&self) {
        let state = self.clone();
        self.maybe_save_then(Box::new(move || {
            state.load_text("", None);
            state.set_mode(MODE_EDIT, false);
        }));
    }

    fn action_open(&self) {
        let state = self.clone();
        self.maybe_save_then(Box::new(move || {
            let dialog = gtk::FileDialog::builder()
                .title("Open Markdown file")
                .build();

            let filters = gio::ListStore::new::<gtk::FileFilter>();
            let md_filter = gtk::FileFilter::new();
            md_filter.set_name(Some("Markdown"));
            for pat in ["*.md", "*.markdown", "*.mdown", "*.mkd", "*.mkdn", "*.txt"] {
                md_filter.add_pattern(pat);
            }
            filters.append(&md_filter);
            let all_filter = gtk::FileFilter::new();
            all_filter.set_name(Some("All files"));
            all_filter.add_pattern("*");
            filters.append(&all_filter);
            dialog.set_filters(Some(&filters));
            dialog.set_default_filter(Some(&md_filter));

            let st = state.clone();
            dialog.open(
                Some(&state.inner.window),
                None::<&gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            st.open_path(&path);
                        }
                    }
                },
            );
        }));
    }

    fn open_path(&self, path: &Path) {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => match fs::read(path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    self.error_dialog("Could not open file", &e.to_string());
                    return;
                }
            },
        };
        self.load_text(&text, Some(path.to_path_buf()));
        // force=true so re-opening into an already-active preview still re-renders.
        self.set_mode(MODE_PREVIEW, true);
    }

    fn action_save(&self) {
        let path = self.inner.current_file.borrow().clone();
        match path {
            Some(p) => self.write_to(&p),
            None => self.action_save_as(),
        }
    }

    fn action_save_as(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Save Markdown file")
            .build();

        if let Some(current) = self.inner.current_file.borrow().clone() {
            if let Some(name) = current.file_name() {
                dialog.set_initial_name(Some(&name.to_string_lossy()));
            }
            if let Some(parent) = current.parent() {
                dialog.set_initial_folder(Some(&gio::File::for_path(parent)));
            }
        } else {
            dialog.set_initial_name(Some("untitled.md"));
        }

        let md_filter = gtk::FileFilter::new();
        md_filter.set_name(Some("Markdown"));
        md_filter.add_pattern("*.md");
        md_filter.add_pattern("*.markdown");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&md_filter);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&md_filter));

        let st = self.clone();
        dialog.save(
            Some(&self.inner.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(mut path) = file.path() {
                        if path.extension().is_none() {
                            path.set_extension("md");
                        }
                        st.write_to(&path);
                    }
                }
            },
        );
    }

    // -- Export ----------------------------------------------------------
    fn action_export_html(&self) {
        let dialog = gtk::FileDialog::builder().title("Export as HTML").build();
        dialog.set_initial_name(Some(&self.export_default_name("html")));
        if let Some(folder) = self.export_initial_folder() {
            dialog.set_initial_folder(Some(&folder));
        }

        let html_filter = gtk::FileFilter::new();
        html_filter.set_name(Some("HTML"));
        html_filter.add_pattern("*.html");
        html_filter.add_pattern("*.htm");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&html_filter);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&html_filter));

        let st = self.clone();
        dialog.save(
            Some(&self.inner.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(mut path) = file.path() {
                        if path.extension().is_none() {
                            path.set_extension("html");
                        }
                        st.do_export_html(&path);
                    }
                }
            },
        );
    }

    fn do_export_html(&self, path: &Path) {
        let text = self.buffer_text();
        let current = self.inner.current_file.borrow().clone();
        let base_dir = current
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let title = current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| APP_NAME.to_string());
        let html = render_markdown_to_html(&text, base_dir.as_deref(), self.is_dark(), &title);

        // Atomic write: tmp + rename, mirroring write_to.
        let mut tmp = path.to_path_buf();
        let tmp_name = match path.file_name() {
            Some(n) => {
                let mut s = n.to_os_string();
                s.push(".tmp");
                s
            }
            None => {
                self.error_dialog("Could not export HTML", "Invalid path");
                return;
            }
        };
        tmp.set_file_name(tmp_name);
        if let Err(e) = fs::write(&tmp, html.as_bytes()) {
            self.error_dialog("Could not export HTML", &e.to_string());
            return;
        }
        if let Err(e) = fs::rename(&tmp, path) {
            self.error_dialog("Could not export HTML", &e.to_string());
            return;
        }
        self.show_toast(&format!("HTML exported to {}", path.display()));
    }

    fn action_export_pdf(&self) {
        let dialog = gtk::FileDialog::builder().title("Export as PDF").build();
        dialog.set_initial_name(Some(&self.export_default_name("pdf")));
        if let Some(folder) = self.export_initial_folder() {
            dialog.set_initial_folder(Some(&folder));
        }

        let pdf_filter = gtk::FileFilter::new();
        pdf_filter.set_name(Some("PDF"));
        pdf_filter.add_pattern("*.pdf");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&pdf_filter);
        dialog.set_filters(Some(&filters));
        dialog.set_default_filter(Some(&pdf_filter));

        let st = self.clone();
        dialog.save(
            Some(&self.inner.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(mut path) = file.path() {
                        if path.extension().is_none() {
                            path.set_extension("pdf");
                        }
                        st.do_export_pdf(&path);
                    }
                }
            },
        );
    }

    fn do_export_pdf(&self, path: &Path) {
        // PrintOperation snapshots the current WebView content. If the user
        // is in edit mode (or has typed since the last preview), force a
        // re-render and run the print only after load_changed=Finished, so
        // the PDF reflects the current buffer.
        let st = self.clone();
        let path = path.to_path_buf();

        let handler_id: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
        let handler_id_for_closure = handler_id.clone();

        let id = self.inner.webview.connect_load_changed(move |wv, event| {
            if event != webkit6::LoadEvent::Finished {
                return;
            }
            if let Some(id) = handler_id_for_closure.borrow_mut().take() {
                wv.disconnect(id);
            }
            st.run_pdf_print(&path);
        });
        *handler_id.borrow_mut() = Some(id);

        self.refresh_preview();
    }

    fn run_pdf_print(&self, path: &Path) {
        let op = webkit6::PrintOperation::new(&self.inner.webview);

        let settings = gtk::PrintSettings::new();
        // The "Print to File" virtual printer is part of GTK's print backend
        // and writes whatever the configured output-uri points at.
        settings.set_printer("Print to File");
        let path_str = path.to_string_lossy();
        let escaped = glib::Uri::escape_string(&path_str, Some("/"), false).to_string();
        settings.set("output-uri", Some(&format!("file://{}", escaped)));
        op.set_print_settings(&settings);

        let setup = gtk::PageSetup::new();
        setup.set_paper_size(&gtk::PaperSize::new(Some("iso_a4")));
        op.set_page_setup(&setup);

        let st = self.clone();
        let done_path = path.to_path_buf();
        op.connect_finished(move |_| {
            st.show_toast(&format!("PDF exported to {}", done_path.display()));
        });

        let st = self.clone();
        op.connect_failed(move |_, err| {
            st.error_dialog("Could not export PDF", &err.to_string());
        });

        op.print();
    }

    fn export_default_name(&self, ext: &str) -> String {
        let stem = self
            .inner
            .current_file
            .borrow()
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".to_string());
        format!("{}.{}", stem, ext)
    }

    fn export_initial_folder(&self) -> Option<gio::File> {
        self.inner
            .current_file
            .borrow()
            .as_ref()
            .and_then(|p| p.parent().map(gio::File::for_path))
    }

    fn write_to(&self, path: &Path) {
        let text = self.buffer_text();
        // Atomic-ish: write to .tmp then rename.
        let mut tmp = path.to_path_buf();
        let tmp_name = match path.file_name() {
            Some(n) => {
                let mut s = n.to_os_string();
                s.push(".tmp");
                s
            }
            None => {
                self.error_dialog("Could not save file", "Invalid path");
                return;
            }
        };
        tmp.set_file_name(tmp_name);
        if let Err(e) = fs::write(&tmp, text.as_bytes()) {
            self.error_dialog("Could not save file", &e.to_string());
            return;
        }
        // Stamp before the rename so the watcher's resulting event is
        // suppressed (the 1500 ms window in start_watching's timer).
        self.inner.last_self_write.set(Instant::now());
        if let Err(e) = fs::rename(&tmp, path) {
            self.error_dialog("Could not save file", &e.to_string());
            return;
        }
        *self.inner.current_file.borrow_mut() = Some(path.to_path_buf());
        self.mark_clean();
        self.update_title();
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    // -- Buffer / state --------------------------------------------------
    fn load_text(&self, text: &str, path: Option<PathBuf>) {
        self.inner.suppress_modify.set(true);
        self.inner.buffer.set_text(text);
        self.inner.suppress_modify.set(false);
        *self.inner.current_file.borrow_mut() = path.clone();
        // Drop any stale change-markers from a prior reload — they belong to
        // a different file or a stale baseline. reload_from_disk re-sets this
        // *after* calling load_text.
        *self.inner.pending_changes.borrow_mut() = None;
        // Fetch git history for the new file (None if not in a repo).
        // Synchronous for now — capped at 100 commits keeps it cheap.
        let history = path.as_ref().and_then(|p| fetch_git_history(p));
        *self.inner.git_history.borrow_mut() = history;
        *self.inner.viewing_snapshot.borrow_mut() = None;
        self.mark_clean();
        self.update_title();
        self.update_watch(path.as_deref());
    }

    // -- External-change watcher -----------------------------------------
    fn update_watch(&self, path: Option<&Path>) {
        self.stop_watching();
        if let Some(p) = path {
            self.start_watching(p);
        }
    }

    fn start_watching(&self, path: &Path) {
        // Watch the parent directory rather than the file itself: editors
        // (vim, VSCode, our own atomic save) replace files via tmp+rename,
        // which deletes the original inode and would orphan a file-level
        // watch. NonRecursive keeps us from snooping siblings unnecessarily.
        let parent = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let target_name = match path.file_name() {
            Some(n) => n.to_os_string(),
            None => return,
        };

        let (tx, rx) = mpsc::channel::<()>();

        let mut watcher: notify::RecommendedWatcher =
            match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let touches_target = event
                        .paths
                        .iter()
                        .any(|p| p.file_name() == Some(&target_name));
                    if touches_target {
                        let _ = tx.send(());
                    }
                }
            }) {
                Ok(w) => w,
                Err(_) => return,
            };

        if watcher
            .watch(&parent, notify::RecursiveMode::NonRecursive)
            .is_err()
        {
            return;
        }
        *self.inner.watcher.borrow_mut() = Some(watcher);

        // 150 ms timer pulls events from notify's worker thread onto the
        // main loop and coalesces inotify bursts (a single save can fire
        // CREATE+MODIFY+ATTRIB+RENAME) into one reload.
        let st = self.clone();
        let source_id = glib::timeout_add_local(Duration::from_millis(150), move || {
            let mut got_event = false;
            while rx.try_recv().is_ok() {
                got_event = true;
            }
            if got_event && st.inner.last_self_write.get().elapsed() > Duration::from_millis(1500) {
                st.on_external_change();
            }
            glib::ControlFlow::Continue
        });
        *self.inner.watch_source_id.borrow_mut() = Some(source_id);
    }

    fn stop_watching(&self) {
        if let Some(source_id) = self.inner.watch_source_id.take() {
            source_id.remove();
        }
        *self.inner.watcher.borrow_mut() = None;
    }

    fn on_external_change(&self) {
        let path = match self.inner.current_file.borrow().clone() {
            Some(p) => p,
            None => return,
        };

        let in_edit = self.inner.mode.borrow().as_str() == MODE_EDIT;
        let dirty = self.inner.is_modified.get();

        if in_edit && dirty {
            // Don't clobber unsaved edits — surface a Reload prompt.
            self.show_reload_toast();
            return;
        }
        self.reload_from_disk(&path);
        self.show_toast("Reloaded — file changed on disk");
    }

    fn reload_from_disk(&self, path: &Path) {
        let new_text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => match fs::read(path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(_) => {
                    self.show_toast(&format!("{} is no longer readable", path.display()));
                    return;
                }
            },
        };
        let old_text = self.buffer_text();
        self.load_text(&new_text, Some(path.to_path_buf()));
        let changed = compute_changed_lines(&old_text, &new_text);
        if !changed.is_empty() {
            let reload_ts = std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            *self.inner.pending_changes.borrow_mut() = Some(PendingChanges {
                changed_lines: changed,
                old_text,
                reload_ts,
            });
        }
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    fn show_toast(&self, message: &str) {
        const TIMEOUT_SECS: u32 = 6;
        let toast = adw::Toast::new("");
        toast.set_timeout(TIMEOUT_SECS);

        // Custom title: vertical box with the message and a thin progress
        // bar that fills left→right as the timer runs out. Adwaita already
        // shows its own dismiss "X" on the toast, so we don't add another
        // button here (otherwise the user sees two close icons).
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let label = gtk::Label::new(Some(message));
        label.set_wrap(true);
        label.set_xalign(0.0);
        label.set_hexpand(true);
        vbox.append(&label);

        let progress = gtk::ProgressBar::new();
        progress.set_fraction(0.0);
        progress.set_hexpand(true);
        progress.add_css_class("rmd-toast-progress");
        vbox.append(&progress);

        toast.set_custom_title(Some(&vbox));

        // Animate the progress bar from empty → full over the timeout.
        // Stops when the toast has been dismissed (weak-ref upgrade fails).
        let total_ms = u64::from(TIMEOUT_SECS) * 1000;
        let tick_ms: u64 = 50;
        let steps = (total_ms / tick_ms) as f64;
        let inc = 1.0 / steps;
        let progress_clone = progress.clone();
        let toast_weak = toast.downgrade();
        let current = Rc::new(Cell::new(0.0_f64));
        glib::timeout_add_local(Duration::from_millis(tick_ms), move || {
            if toast_weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            let next = current.get() + inc;
            if next >= 1.0 {
                progress_clone.set_fraction(1.0);
                return glib::ControlFlow::Break;
            }
            current.set(next);
            progress_clone.set_fraction(next);
            glib::ControlFlow::Continue
        });

        self.inner.toast_overlay.add_toast(toast);
    }

    fn show_reload_toast(&self) {
        let toast = adw::Toast::new("File changed on disk — discard your edits?");
        toast.set_button_label(Some("Reload"));
        toast.set_timeout(0);
        let st = self.clone();
        toast.connect_button_clicked(move |t| {
            if let Some(p) = st.inner.current_file.borrow().clone() {
                st.reload_from_disk(&p);
            }
            t.dismiss();
        });
        self.inner.toast_overlay.add_toast(toast);
    }

    fn buffer_text(&self) -> String {
        let (start, end) = self.inner.buffer.bounds();
        self.inner.buffer.text(&start, &end, true).to_string()
    }

    fn on_buffer_changed(&self) {
        if self.inner.suppress_modify.get() {
            return;
        }
        // Editing the working copy while viewing history: snap back to
        // the working copy so the preview stays consistent with what
        // they're typing.
        if self.inner.viewing_snapshot.borrow().is_some() {
            *self.inner.viewing_snapshot.borrow_mut() = None;
            if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                self.refresh_preview();
            }
        }
        if !self.inner.is_modified.get() {
            self.inner.is_modified.set(true);
            self.update_title();
        }
    }

    fn mark_clean(&self) {
        self.inner.is_modified.set(false);
    }

    fn update_title(&self) {
        let current = self.inner.current_file.borrow().clone();
        let name = current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string());
        let prefix = if self.inner.is_modified.get() {
            "\u{2022} "
        } else {
            ""
        };
        self.inner
            .window
            .set_title(Some(&format!("{}{} \u{2014} {}", prefix, name, APP_NAME)));
        match current {
            Some(p) => {
                self.inner.status_path.set_text(&p.to_string_lossy());
                self.inner.status_mtime.set_text(&format_mtime(&p));
            }
            None => {
                self.inner.status_path.set_text("(unsaved document)");
                self.inner.status_mtime.set_text("");
            }
        }
    }

    fn do_undo(&self) {
        if self.inner.buffer.can_undo() {
            self.inner.buffer.undo();
            // The buffer-changed signal does run, but on_buffer_changed
            // only flips the dirty flag — it doesn't kick a preview
            // refresh. After undoing a click-to-edit table change the
            // user expects the WebView to revert too, so do it here.
            if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                self.refresh_preview();
            }
        }
    }

    fn do_redo(&self) {
        if self.inner.buffer.can_redo() {
            self.inner.buffer.redo();
            if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
                self.refresh_preview();
            }
        }
    }

    // -- Save-prompt flow ------------------------------------------------
    fn maybe_save_then(&self, then: Box<dyn Fn() + 'static>) {
        if !self.inner.is_modified.get() {
            then();
            return;
        }
        let body = match self.inner.current_file.borrow().as_ref() {
            Some(p) => format!(
                "\u{201C}{}\u{201D} has unsaved changes.",
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            ),
            None => "This document has unsaved changes.".to_string(),
        };

        let dialog = adw::AlertDialog::builder()
            .heading("Save changes?")
            .body(&body)
            .build();
        dialog.add_response("discard", "Discard");
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");

        let then = Rc::new(then);
        let state = self.clone();
        dialog.connect_response(None, move |_, response| match response {
            "save" => {
                let then = then.clone();
                let state2 = state.clone();
                if state.inner.current_file.borrow().is_none() {
                    state.save_as_then(Box::new(move || {
                        if !state2.inner.is_modified.get() {
                            then();
                        }
                    }));
                } else {
                    let path = state.inner.current_file.borrow().clone().unwrap();
                    state.write_to(&path);
                    if !state.inner.is_modified.get() {
                        then();
                    }
                }
            }
            "discard" => {
                state.mark_clean();
                then();
            }
            _ => {}
        });
        dialog.present(Some(&self.inner.window));
    }

    fn save_as_then(&self, then: Box<dyn Fn() + 'static>) {
        let dialog = gtk::FileDialog::builder()
            .title("Save Markdown file")
            .build();
        let initial = self
            .inner
            .current_file
            .borrow()
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "untitled.md".to_string());
        dialog.set_initial_name(Some(&initial));

        let st = self.clone();
        dialog.save(
            Some(&self.inner.window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(mut path) = file.path() {
                        if path.extension().is_none() {
                            path.set_extension("md");
                        }
                        st.write_to(&path);
                        if !st.inner.is_modified.get() {
                            then();
                        }
                    }
                }
            },
        );
    }

    fn on_close_request(&self) -> glib::Propagation {
        if !self.inner.is_modified.get() {
            self.save_window_state();
            return glib::Propagation::Proceed;
        }
        let state = self.clone();
        self.maybe_save_then(Box::new(move || {
            state.save_window_state();
            state.inner.window.destroy();
        }));
        glib::Propagation::Stop
    }

    // -- About / shortcuts ----------------------------------------------
    fn action_about(&self) {
        const REPO_URL: &str = "https://github.com/imcmurray/RenderMD";
        let sha = env!("GIT_SHA");
        let version = if sha == "unknown" {
            env!("CARGO_PKG_VERSION").to_string()
        } else {
            format!("{} ({})", env!("CARGO_PKG_VERSION"), sha)
        };

        let about = adw::AboutDialog::builder()
            .application_name(APP_NAME)
            .application_icon(APP_ID)
            .developer_name("You + Claude")
            .version(&version)
            .comments(
                "A native GTK4 Markdown viewer/editor with one-key toggle between rendered preview and editing.",
            )
            .license_type(gtk::License::MitX11)
            .website(REPO_URL)
            .issue_url(format!("{}/issues", REPO_URL))
            .build();

        // Direct link to the exact commit this binary was built from.
        // Skipped when the SHA is unknown (e.g. building from a tarball).
        if sha != "unknown" {
            about.add_link("View this commit", &format!("{}/commit/{}", REPO_URL, sha));
        }

        about.present(Some(&self.inner.window));
    }

    fn action_shortcuts(&self) {
        let body = "Ctrl+N            New\n\
                    Ctrl+O            Open\n\
                    Ctrl+S            Save\n\
                    Ctrl+Shift+S      Save As\n\
                    F5 / Ctrl+Shift+E Toggle Preview / Edit\n\
                    Ctrl+Z / Y        Undo / Redo\n\
                    Ctrl+W / Q        Close window / quit";
        let dialog = adw::AlertDialog::builder()
            .heading("Keyboard Shortcuts")
            .body(body)
            .build();
        dialog.add_response("ok", "OK");
        dialog.set_default_response(Some("ok"));
        dialog.present(Some(&self.inner.window));
    }

    fn error_dialog(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .build();
        dialog.add_response("ok", "OK");
        dialog.set_default_response(Some("ok"));
        dialog.present(Some(&self.inner.window));
    }

    // -- Git history toggle ---------------------------------------------
    fn action_toggle_history(&self) {
        let now = !self.inner.history_visible.get();
        self.inner.history_visible.set(now);
        if self.inner.git_history.borrow().is_some() {
            self.show_toast(if now {
                "History rail shown"
            } else {
                "History rail hidden"
            });
        } else {
            self.show_toast("This file isn't tracked in a git repo");
        }
        if self.inner.mode.borrow().as_str() == MODE_PREVIEW {
            self.refresh_preview();
        }
    }

    // -- Window + UI settings persistence -------------------------------
    fn restore_window_state(&self) {
        let path = settings_file();
        if !path.exists() {
            return;
        }
        let kf = glib::KeyFile::new();
        if kf.load_from_file(&path, glib::KeyFileFlags::NONE).is_err() {
            return;
        }
        let w = kf.integer("window", "width").unwrap_or(0);
        let h = kf.integer("window", "height").unwrap_or(0);
        let maxd = kf.boolean("window", "maximized").unwrap_or(false);
        if w > 400 && h > 300 {
            self.inner.window.set_default_size(w, h);
        }
        if maxd {
            self.inner.window.maximize();
        }
        // Default to true if the key isn't present yet — first-run users
        // should see the rail when they open something in a git repo.
        let history = kf.boolean("ui", "history-visible").unwrap_or(true);
        self.inner.history_visible.set(history);
    }

    fn save_window_state(&self) {
        let dir = settings_dir();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let kf = glib::KeyFile::new();
        let (w, h) = self.inner.window.default_size();
        kf.set_integer("window", "width", w);
        kf.set_integer("window", "height", h);
        kf.set_boolean("window", "maximized", self.inner.window.is_maximized());
        kf.set_boolean("ui", "history-visible", self.inner.history_visible.get());
        let _ = kf.save_to_file(settings_file());
    }
}

// ---- Icon theme search ------------------------------------------------------
// AdwAboutDialog and the .desktop entry both reference the icon by name
// (`io.github.rendermd.RenderMD`). For the lookup to succeed we need the SVG
// to live in a directory the GTK icon theme searches. Try a few candidate
// locations so this works whether you ran `cargo run`, `./rendermd` from the
// project root, or installed the binary somewhere else.
// App-level CSS overrides: bumps toast contrast (the default Adwaita toast
// can look washed-out over a busy WebView background) and tightens the
// embedded progress bar we use for the auto-dismiss countdown.
const APP_CSS: &str = r#"
toast {
  background-color: rgba(28, 28, 32, 0.96);
  color: #ffffff;
}
toast button {
  color: #ffffff;
}
.rmd-toast-progress trough,
.rmd-toast-progress progress {
  min-height: 3px;
  border-radius: 1.5px;
}
.rmd-toast-progress progress {
  background-color: #e3b341;
}
"#;

fn register_app_css() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(APP_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn register_icon_search_paths() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let theme = gtk::IconTheme::for_display(&display);

    let mut candidates: Vec<PathBuf> = Vec::new();

    // Dev mode: the source tree, baked in at compile time.
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/icons"));

    // Installed alongside the binary.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("data/icons"));
            candidates.push(parent.join("../share/icons"));
            candidates.push(parent.join("../share/rendermd/icons"));
        }
    }

    for path in candidates {
        if path.exists() {
            theme.add_search_path(&path);
        }
    }
}

// ---- Free functions --------------------------------------------------------
fn on_webview_policy(
    window: &adw::ApplicationWindow,
    decision: &webkit6::PolicyDecision,
    decision_type: webkit6::PolicyDecisionType,
) -> bool {
    if decision_type != webkit6::PolicyDecisionType::NavigationAction {
        return false;
    }
    let Some(nav_decision) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
        return false;
    };
    let Some(mut action) = nav_decision.navigation_action() else {
        return false;
    };
    if action.navigation_type() != webkit6::NavigationType::LinkClicked {
        return false;
    }
    let Some(request) = action.request() else {
        return false;
    };
    let Some(uri) = request.uri() else {
        return false;
    };
    let uri_str = uri.as_str();
    if uri_str.is_empty() || uri_str.starts_with("about:") {
        return false;
    }
    let launcher = gtk::UriLauncher::new(uri_str);
    launcher.launch(Some(window), None::<&gio::Cancellable>, |_| {});
    decision.ignore();
    true
}

// ---- Active-or-new window tracking -----------------------------------------
thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn ensure_state(app: &adw::Application) -> State {
    STATE.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return existing.clone();
        }
        let state = State::new(app);
        state.build_ui();
        state.wire_actions(app);
        state.setup_drag_and_drop();
        state.setup_image_paste();
        state.setup_image_click_handler();
        state.restore_window_state();
        // Empty doc opens in edit mode; loaded files later switch to preview.
        state.set_mode(MODE_EDIT, true);
        state.update_title();
        *cell.borrow_mut() = Some(state.clone());
        state
    })
}

fn main() -> glib::ExitCode {
    // Default GDK to the GL renderer. The Vulkan path on Wayland+Mesa spams
    // VK_SUBOPTIMAL_KHR warnings on every resize/present cycle; the GL
    // renderer is silent and visually identical for our use. We only set it
    // when the user hasn't picked something explicitly.
    // SAFETY: called before any other thread starts, so no env-var race.
    unsafe {
        if std::env::var_os("GSK_RENDERER").is_none() {
            std::env::set_var("GSK_RENDERER", "gl");
        }
    }

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|_| {
        register_icon_search_paths();
        register_app_css();
    });

    app.connect_activate(|app| {
        let state = ensure_state(app);
        state.inner.window.present();
    });

    app.connect_open(|app, files: &[gio::File], _hint| {
        let state = ensure_state(app);
        if let Some(file) = files.first() {
            if let Some(path) = file.path() {
                state.open_path(&path);
            }
        }
        state.inner.window.present();
    });

    app.run()
}

// ---- Tests ------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_no_fences() {
        let (out, had) = preprocess_mermaid_blocks("# hi\n\nplain text\n");
        assert!(!had);
        assert!(out.contains("# hi"));
        assert!(out.contains("plain text"));
        assert!(!out.contains("class=\"mermaid\""));
    }

    #[test]
    fn preprocess_simple_block() {
        let input = "before\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\nafter\n";
        let (out, had) = preprocess_mermaid_blocks(input);
        assert!(had);
        assert!(out.contains(r#"<pre class="mermaid">"#));
        assert!(out.contains("</pre>"));
        assert!(out.contains("flowchart TD"));
        assert!(
            out.contains("A --&gt; B"),
            "diagram body should be HTML-escaped"
        );
        assert!(out.contains("before") && out.contains("after"));
    }

    #[test]
    fn preprocess_blank_lines_inside() {
        // Regression for issue #1: a multi-section flowchart with a blank line
        // between sections must be preserved as a single mermaid block, not
        // sliced at the blank line.
        let input = "```mermaid\nflowchart TD\n  A --> B\n\n  C --> D\n```\n";
        let (out, _) = preprocess_mermaid_blocks(input);
        // The opening <pre> and closing </pre> must bracket BOTH sections.
        let open = out
            .find(r#"<pre class="mermaid">"#)
            .expect("opening tag present");
        let close = out.find("</pre>").expect("closing tag present");
        assert!(open < close);
        let body = &out[open..close];
        assert!(body.contains("A --&gt; B"));
        assert!(body.contains("C --&gt; D"));
    }

    #[test]
    fn preprocess_unclosed_fence() {
        // Unclosed fence: don't lose user content, restore as-is.
        let input = "before\n\n```mermaid\nflowchart TD\n  A --> B\n";
        let (out, had) = preprocess_mermaid_blocks(input);
        assert!(!had, "unclosed fence shouldn't count as a mermaid block");
        assert!(out.contains("```mermaid"));
        assert!(out.contains("flowchart TD"));
        assert!(!out.contains(r#"<pre class="mermaid">"#));
    }

    #[test]
    fn preprocess_indented_fence_current_behavior() {
        // TODO: CommonMark spec says fences with 4+ spaces of indent are an
        // indented code block, not a fence. Current implementation uses
        // line.trim() so it matches any indent. This test pins the current
        // behavior; tighten if/when we hew closer to spec.
        let input = "    ```mermaid\n    flowchart TD\n    ```\n";
        let (_, had) = preprocess_mermaid_blocks(input);
        assert!(had, "current implementation matches indented fences");
    }

    #[test]
    fn preprocess_html_escapes_diagram() {
        let input = "```mermaid\nA[<b>x</b> & \"y\"]\n```\n";
        let (out, _) = preprocess_mermaid_blocks(input);
        assert!(out.contains("&lt;b&gt;"));
        assert!(out.contains("&amp;"));
        assert!(out.contains("&quot;"));
        assert!(!out.contains("<b>x</b>"));
    }

    #[test]
    fn html_escape_entities() {
        assert_eq!(html_escape("&"), "&amp;");
        assert_eq!(html_escape("<"), "&lt;");
        assert_eq!(html_escape(">"), "&gt;");
        assert_eq!(html_escape("\""), "&quot;");
        assert_eq!(html_escape("'"), "&#39;");
        assert_eq!(
            html_escape("a < b && c > d"),
            "a &lt; b &amp;&amp; c &gt; d"
        );
    }

    #[test]
    fn render_smoke_no_mermaid() {
        let html = render_markdown_to_html("# hello\n\ntext", None, false, "doc");
        assert!(html.contains("<h1"));
        assert!(html.contains("hello"));
        // Bundle marker — first ~30 chars of MERMAID_BUNDLE that wouldn't
        // appear by accident in a normal doc:
        let bundle_fingerprint = &MERMAID_BUNDLE[..30.min(MERMAID_BUNDLE.len())];
        assert!(
            !html.contains(bundle_fingerprint),
            "Mermaid bundle should not be injected when no mermaid blocks are present"
        );
    }

    #[test]
    fn render_injects_mermaid_when_present() {
        let html = render_markdown_to_html(
            "```mermaid\nflowchart TD\n  A --> B\n```\n",
            None,
            false,
            "doc",
        );
        assert!(
            html.contains("mermaid.run()"),
            "init script should be injected"
        );
        let bundle_fingerprint = &MERMAID_BUNDLE[..30.min(MERMAID_BUNDLE.len())];
        assert!(
            html.contains(bundle_fingerprint),
            "bundle should be injected"
        );
    }

    #[test]
    fn render_base_href_set_when_dir_given() {
        let dir = std::path::Path::new("/tmp/somewhere");
        let html = render_markdown_to_html("hi", Some(dir), false, "doc");
        assert!(html.contains(r#"<base href="file:///tmp/somewhere/">"#));
    }

    #[test]
    fn render_base_href_empty_when_no_dir() {
        let html = render_markdown_to_html("hi", None, false, "doc");
        assert!(html.contains(r#"<base href="">"#));
    }

    #[test]
    fn render_dark_theme_picks_dark_css() {
        let dark = render_markdown_to_html("hi", None, true, "doc");
        let light = render_markdown_to_html("hi", None, false, "doc");
        // The two themes diverge in their CSS variable values; confirm we get
        // different output for the two flags.
        assert_ne!(dark, light);
        // Sanity: both contain the shared base CSS.
        assert!(dark.contains(":root"));
        assert!(light.contains(":root"));
    }

    #[test]
    fn emoji_replaces_known_shortcode() {
        let out = replace_shortcodes_in_line("Ship it :rocket:!");
        assert_eq!(out, "Ship it 🚀!");
    }

    #[test]
    fn emoji_passes_unknown_shortcode_through() {
        let out = replace_shortcodes_in_line("This :notarealemoji: stays");
        assert_eq!(out, "This :notarealemoji: stays");
    }

    #[test]
    fn emoji_handles_multiple_per_line() {
        let out = replace_shortcodes_in_line(":rocket: and :tada: and :+1:");
        assert_eq!(out, "🚀 and 🎉 and 👍");
    }

    #[test]
    fn emoji_handles_aliases() {
        // +1, thumbsup are aliases
        assert_eq!(replace_shortcodes_in_line(":+1:"), "👍");
        assert_eq!(replace_shortcodes_in_line(":thumbsup:"), "👍");
    }

    #[test]
    fn emoji_skipped_inside_fenced_block() {
        let input = "before :rocket:\n\n```\n:rocket: in code\n```\n\nafter :tada:\n";
        let out = preprocess_emoji(input);
        assert!(out.contains("before 🚀"));
        assert!(
            out.contains(":rocket: in code"),
            "fence content should be left alone"
        );
        assert!(out.contains("after 🎉"));
    }

    #[test]
    fn emoji_skipped_inside_mermaid_fence() {
        // Emoji runs before mermaid pre-processing; it must skip ```mermaid
        // fences too so the source reaches Mermaid.js untouched.
        let input = "```mermaid\nflowchart TD\n  A[:rocket:] --> B\n```\n";
        let out = preprocess_emoji(input);
        assert!(
            out.contains(":rocket:"),
            "mermaid fence content should be untouched"
        );
    }

    #[test]
    fn emoji_lone_colons_pass_through() {
        let out = replace_shortcodes_in_line("ratio 4:3 and time 12:30:45");
        assert_eq!(out, "ratio 4:3 and time 12:30:45");
    }

    #[test]
    fn alert_note_emits_div() {
        let out = preprocess_alerts("> [!NOTE]\n> Hello there.\n");
        assert!(out.contains(r#"<div class="alert alert-note">"#));
        assert!(out.contains("<span>Note</span>"));
        assert!(out.contains("Hello there."));
    }

    #[test]
    fn alert_each_variant_recognized() {
        for (token, variant, label, _) in ALERT_VARIANTS {
            let input = format!("> {}\n> body\n", token);
            let out = preprocess_alerts(&input);
            assert!(
                out.contains(&format!(r#"alert-{}""#, variant)),
                "missing class for {}",
                token
            );
            assert!(
                out.contains(&format!("<span>{}</span>", label)),
                "missing label for {}",
                token
            );
        }
    }

    #[test]
    fn alert_collects_multiline_body() {
        let input = "> [!WARNING]\n> first line\n> second line\n> third line\n";
        let out = preprocess_alerts(input);
        // The whole alert must live on one line so the <div> survives
        // CommonMark's blank-line termination of HTML blocks.
        let div_start = out.find("<div class=\"alert").unwrap();
        let line_end = out[div_start..]
            .find('\n')
            .map(|i| div_start + i)
            .unwrap_or(out.len());
        let alert_line = &out[div_start..line_end];
        assert!(alert_line.contains("first line"));
        assert!(alert_line.contains("second line"));
        assert!(alert_line.contains("third line"));
        assert!(alert_line.ends_with("</div>"));
    }

    #[test]
    fn alert_terminates_at_non_blockquote_line() {
        let input = "> [!NOTE]\n> inside\nafter\n";
        let out = preprocess_alerts(input);
        let div_start = out.find("<div class=\"alert").unwrap();
        let line_end = out[div_start..]
            .find('\n')
            .map(|i| div_start + i)
            .unwrap_or(out.len());
        let alert_line = &out[div_start..line_end];
        assert!(alert_line.contains("inside"));
        // "after" must NOT be inside the alert; it should be a separate line below.
        assert!(!alert_line.contains("after"));
        assert!(out[line_end..].contains("after"));
    }

    #[test]
    fn alert_unknown_variant_passes_through_as_blockquote() {
        // [!FOOBAR] is not a known variant — leave the lines untouched
        // so comrak renders them as a regular blockquote.
        let input = "> [!FOOBAR]\n> body\n";
        let out = preprocess_alerts(input);
        assert!(out.contains("[!FOOBAR]"));
        assert!(!out.contains("class=\"alert"));
    }

    #[test]
    fn alert_case_sensitive_matches_github() {
        // Lowercase shouldn't match.
        let out = preprocess_alerts("> [!note]\n> body\n");
        assert!(!out.contains("class=\"alert"));
    }

    #[test]
    fn alert_inline_emoji_in_body_renders() {
        // emoji preprocessing runs first, so the body has 🚀 by the time we
        // collect it.
        let input = "> [!TIP]\n> Ship it 🚀\n";
        let out = preprocess_alerts(input);
        assert!(out.contains("🚀"));
    }

    #[test]
    fn render_mermaid_theme_follows_dark_flag() {
        let input = "```mermaid\nflowchart TD\nA --> B\n```\n";
        let dark = render_markdown_to_html(input, None, true, "doc");
        let light = render_markdown_to_html(input, None, false, "doc");
        assert!(dark.contains("theme: 'dark'"));
        assert!(light.contains("theme: 'default'"));
    }
}
