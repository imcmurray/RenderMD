# Future features to consider

A scratchpad for ideas that aren't on the active roadmap. Entries are unordered
and unestimated — pull into a release when the timing's right. Notes in
parentheses flag what's already partially wired in `src/main.rs`.

## Navigation & structure

- [ ] **Outline / TOC sidebar.** Scrape `<h1>–<h6>` from the rendered HTML into a
  collapsible side panel with click-to-scroll. (`header_ids` is already enabled
  on the comrak options, so anchors exist.)
- [ ] **Find in preview / editor.** `Ctrl+F` overlay. WebKit has a native
  `FindController`; the editor can use `GtkSourceSearchContext`.
- [ ] **Jump to line.** `Ctrl+G` in edit mode.
- [ ] **Scroll sync.** Map editor cursor line ↔ preview heading/paragraph so
  scrolling one moves the other. The single biggest UX upgrade for long docs.
- [ ] **Document bookmarks.** Mark positions and jump back, persisted per file.

## Rendering

- [ ] **Math (KaTeX).** Mermaid bundle pattern is a clean template — enable
  comrak's `math_dollars`/`math_code` extensions and inject KaTeX. ~270 kB
  bundle cost.
- [ ] **PlantUML / Graphviz fenced blocks.** Same pattern as Mermaid. PlantUML
  needs an external server or a local jar; Graphviz can run via `dot`.
- [ ] **Frontmatter rendering.** Detect YAML/TOML frontmatter and render as a
  metadata card at the top (title, date, tags) instead of a code block.
- [ ] **Wiki-links.** `[[Other Note]]` → link to a sibling `.md`. Useful once
  folder mode (below) lands.
- [ ] **Footnote popovers.** Hover/click a footnote ref to show the body inline
  instead of jumping to the bottom.
- [ ] **Image lightbox.** Click an image to view full-size with zoom/pan.
- [ ] **Custom preview CSS.** Let users drop a `preview.css` next to the file or
  in `~/.config/rendermd/` to override the built-in stylesheet.
- [ ] **Print stylesheet.** Distinct CSS for the `Ctrl+P` path so printed output
  drops the dark background and uses serif body text.

## Editor

- [ ] **Spell check.** GtkSource has `gspell` integration; toggle from a menu.
- [ ] **Split view.** Editor + preview side-by-side as a third mode (alongside
  the existing edit/preview toggle).
- [ ] **Smart shortcuts.** `Ctrl+B`/`Ctrl+I` wrap selection; `Ctrl+K` for link.
- [ ] **Auto-continue lists.** Pressing Enter inside `- ` or `1. ` continues the
  list; pressing again on an empty item ends it.
- [ ] **Image paste from clipboard.** Save into a sibling `assets/` folder and
  insert the relative `![](...)` reference.
- [ ] **Smart link paste.** If clipboard is a URL and there's a selection,
  replace selection with `[selection](url)`.
- [ ] **Structure hinter / linter.** Inline diagnostics in the editor that
  flag formatting problems as you type, so users see *where* a doc is broken
  instead of guessing from a misrendered preview. Comrak already builds an
  AST during render — walk that same AST to emit diagnostics, no extra
  parser needed.
  - [ ] Underline / squiggle the offending range in the editor (GtkSource
    supports tag-based highlighting; or use a `GtkSourceGutterRenderer` for
    a margin marker).
  - [ ] Hover tooltip with the rule name and a short explanation.
  - [ ] Status-bar pill: "3 issues" — click to jump to the next one.
  - [ ] `F8` / `Shift+F8` to cycle through issues.
  - [ ] Toggle on/off from the menu; remember the choice in `settings.ini`.
  - [ ] Rules to start with:
    - Skipped heading level (h1 → h3 with no h2).
    - Unclosed code fence or mermaid block.
    - Broken relative link / image (file doesn't exist next to the doc).
    - Duplicate heading text (which would collide on `header_ids`).
    - Mixed list markers within one list (`-` and `*` interleaved).
    - Mixed emphasis style (`*foo*` and `_foo_` in the same doc).
    - Missing alt text on images.
    - Trailing whitespace and hard tabs in body text.
    - Table column count mismatch between header and body rows.
  - [ ] Per-rule severity (error / warning / hint) and a way to disable a
    rule for a single line via an HTML comment, e.g. `<!-- rendermd-disable
    no-skipped-heading -->`.
- [ ] **Vim mode.** Lots of GTK editor users would expect this. Possibly via a
  separate keymap rather than full modal editing.

## File management

- [ ] **Recent files menu.** Persist a short MRU list to the existing
  `~/.config/rendermd/settings.ini`.
- [ ] **Folder mode.** Open a directory and show a file tree in a sidebar, like
  Obsidian / Typora's vault view. Unlocks wiki-links and global search.
- [ ] **Quick switcher.** `Ctrl+P` fuzzy finder over the open folder.
- [ ] **Tabs (or window-per-doc).** Currently single-window — pick one model
  before the surface gets bigger.
- [ ] **Session restore.** Reopen the last file (and mode) on launch.
- [ ] **Git status indicators.** Tiny dot in the title or sidebar when the file
  has uncommitted changes.

## Export & sharing

- [ ] **Native print.** `Ctrl+P` via `WebKitPrintOperation`, distinct from the
  existing Export-as-PDF path.
- [ ] **Copy as rich text.** Put HTML on the clipboard so paste into email /
  docs / Slack works without losing formatting.
- [ ] **Pandoc backend (optional).** Detect `pandoc` and offer DOCX / EPUB /
  reStructuredText export when present.
- [ ] **Export templates.** Bundle a couple of preview themes (e.g. "GitHub",
  "minimal", "academic") and let the user pick at export time.

## UX & polish

- [ ] **Word count / reading time** in the status bar.
- [ ] **Distraction-free mode.** Hide the header bar and centre the editor on a
  narrow column. `F11`-ish.
- [ ] **Typewriter scroll.** Keep the active line vertically centred while
  typing.
- [ ] **Font / size / content-width preferences.** A small Preferences dialog
  that writes to `settings.ini`.
- [ ] **Zoom in preview.** `Ctrl++` / `Ctrl+-` / `Ctrl+0`. WebKit supports it
  natively.
- [ ] **First-run welcome.** A short markdown doc that demonstrates the
  rendering features (alerts, mermaid, code blocks, etc.) — opens once on
  first launch and is reachable from the menu.

## Integrations & power-user

- [ ] **LSP for markdown.** Wire up `marksman` for completion, hover, go-to-def
  on wiki-links and headings.
- [ ] **Watcher tuning for sync tools.** Syncthing/Dropbox can fire spurious
  modify events; offer a setting to lengthen the debounce when the file
  lives under a known sync root.
- [ ] **Headless render CLI.** `rendermd --to-html file.md` for piping into
  scripts, reusing the same renderer the GUI uses.
- [ ] **MCP / API server mode.** Expose the renderer over a local socket so
  other tools can request HTML rendering with the same theme/extensions.

## Tech debt / internals

- [ ] **Async render.** Move `render_markdown_to_html` off the main loop for
  large docs so typing-while-large-preview doesn't stutter.
- [ ] **Incremental render.** Only re-render the changed sections rather than
  the whole document. Big lift; revisit only if async isn't enough.
- [ ] **UI tests.** A minimal harness that drives `adw::Application` headlessly
  to catch regressions like issue #2 before they reach the user.
- [ ] **Tighten CommonMark spec compliance.** See the `preprocess_indented_fence_current_behavior`
  test — fences with 4+ spaces of indent should be code blocks, not fences.
