# RenderMD

A native GTK4 Markdown viewer/editor for EndeavourOS / Arch + Budgie. Opens
`.md` files in a rendered preview by default, toggles to a syntax-highlighted
editor with `F5` or `Ctrl+Shift+E`.

## Layout

- `src/main.rs` — the app shell: UI, actions, preview rendering, scroll
  sync, file I/O, git history rail, emoji/alert/mermaid preprocessing
  (~5,400 lines of Rust)
- `src/tables/` — the markdown table subsystem (parse, model, serialize,
  render, smart-paste). `mod.rs` documents the design; `main.rs` drives it
  via the `handle_table_*` methods. This is the one place the app is *not*
  single-file — it earned its own module.
- `build.rs` — embeds the short git SHA (`GIT_SHA`) at build time for the
  About dialog. Falls back to `git rev-parse`, then literal `unknown`.
- `data/js/mermaid.min.js` — vendored Mermaid bundle, embedded via
  `include_str!` and served to the WebView from a `OnceLock` URI.
- `Cargo.toml` — crate manifest
- `rendermd` — bash launcher; resolves the release binary next to itself
- `io.github.rendermd.RenderMD.desktop` — Budgie menu entry + `.md` MIME
  association. The filename **must** match `APP_ID` so Wayland compositors
  can correlate windows (whose `app_id` GTK4 sets from `application_id`) to
  this desktop entry — otherwise the panel falls back to a generic icon.
  Don't rename to `rendermd.desktop`.
- `setup.sh` — runs `cargo build --release`
- `data/icons/hicolor/scalable/apps/io.github.rendermd.RenderMD.svg` — app
  icon. `register_icon_search_paths()` adds candidate dirs to the GTK icon
  theme at `connect_startup`; `AdwAboutDialog::application_icon(APP_ID)` and
  the `.desktop` `Icon=` line then resolve to it.
- `docs/screenshot.png` — used in the README

## Tech stack

- Rust 2021 edition
- `gtk4` (`adw::Application`, `adw::HeaderBar`, `adw::ToolbarView`,
  `adw::AlertDialog`, `adw::AboutDialog`) via the
  [gtk4-rs](https://gtk-rs.org/) crates
- `libadwaita` (aliased to `adw` in `Cargo.toml`)
- `sourceview5` for the editor
- `webkit6` for the preview pane
- `comrak` (with the `syntect` feature) for Markdown rendering + code highlight

Crate features pinned: `gtk4 v4_12`, `libadwaita v1_5`, `sourceview5 v5_10`.

## Build & run

```bash
./setup.sh                              # cargo build --release
./rendermd                              # blank doc
./rendermd file.md                      # opens straight into preview
# or, in dev:
cargo run --release -- file.md
```

App ID: `io.github.rendermd.RenderMD`. Settings:
`~/.config/rendermd/settings.ini`.

## Conventions

- Mostly-single-file app: keep new code in `src/main.rs` unless it earns its
  own module the way `src/tables/` did (a self-contained subsystem with its
  own tests). Don't split `main.rs` up for its own sake.
- Keep the preview CSS in the three string constants at the top of the file
  (`PREVIEW_CSS_LIGHT`, `PREVIEW_CSS_DARK`, `PREVIEW_CSS_BASE`). The base
  stylesheet uses CSS variables so the same rules work in both themes.
- Preview rendering goes through `render_markdown_to_html()` and
  `WebView::load_html(html, base_uri)`. The `<base href>` and `base_uri` must
  both be the markdown file's directory or relative images break.
- File writes are atomic: write `path.tmp`, then `fs::rename`. Don't bypass.
- Theme: react to `adw::StyleManager::connect_dark_notify`, then re-render
  preview and reapply the GtkSource style scheme. Don't read GTK settings
  directly.
- Actions are `gio::SimpleAction` on the window, with accelerators set via
  `app.set_accels_for_action("win.<name>", ...)`. Add new shortcuts there,
  not on individual widgets.
- The toggle button's `toggled` signal handler ID is stored on `StateInner`
  and `block_signal`/`unblock_signal` is used when programmatically syncing
  in `set_mode`, otherwise mode changes recurse.
- Cross-callback state lives in `Rc<StateInner>` with `RefCell`/`Cell`
  interior mutability. Closures clone the `State` wrapper; signal handlers
  needing weak references use `glib::clone!` with `#[weak]` / `#[upgrade_or]`.
- Single-window app — `STATE` thread-local holds the one live `State`;
  `ensure_state` returns it for both `activate` and `open` callbacks.
- `main()` sets `GSK_RENDERER=gl` (only if not already set) before app
  startup to avoid the noisy `VK_SUBOPTIMAL_KHR` warnings the Vulkan path
  emits on Wayland+Mesa during resize/present cycles. Don't remove this
  unless you've verified those warnings stay silent on the GL renderer.

## Things that are intentional, do not "fix"

- Empty document opens in **edit** mode; loaded files open in **preview** mode.
- External link clicks in preview are routed to `gtk::UriLauncher` and the
  WebView's navigation is cancelled — the preview must never navigate away.
- The `gtk::prelude::*` import is *not* needed once `adw::prelude::*` and
  `webkit6::prelude::*` are in scope; both re-export gtk's traits.
- Vulkan `VK_SUBOPTIMAL_KHR` warnings on Wayland are benign — they come from
  GDK's swapchain-resize handling, not from this app.

## Arch package names (for reference when adding deps)

System libraries (only needed for the build/runtime, not at the language
level): `gtk4`, `libadwaita`, `gtksourceview5`, `webkitgtk-6.0`, `glib2`,
`hicolor-icon-theme`. The Rust toolchain itself: `rust`.
