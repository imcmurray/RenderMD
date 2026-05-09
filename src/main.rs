// RenderMD — a native GTK4 Markdown viewer/editor.
// Renders Markdown by default and toggles to a GtkSourceView 5 editor with
// F5 / Ctrl+Shift+E. Single-file Rust port of the original Python prototype.

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{markdown_to_html_with_plugins, ComrakOptions, ComrakPlugins};
use gtk::glib::{self, clone};
use gtk::{gdk, gio, pango};
use sourceview5::prelude::*;
use webkit6::prelude::*;

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
table tr:nth-child(2n) { background: var(--table-stripe); }
table th { font-weight: 600; background: var(--table-stripe); }

img {
  max-width: 100%;
  height: auto;
  border-radius: 6px;
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
</body>
</html>
"#;

// ---- Settings ---------------------------------------------------------------
fn settings_dir() -> PathBuf {
    glib::user_config_dir().join("rendermd")
}

fn settings_file() -> PathBuf {
    settings_dir().join("settings.ini")
}

// ---- Markdown rendering -----------------------------------------------------
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
    let theme = if dark { "base16-ocean.dark" } else { "InspiredGitHub" };
    let adapter = SyntectAdapter::new(Some(theme));
    let mut plugins = ComrakPlugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&adapter);

    let body = markdown_to_html_with_plugins(text, &options, &plugins);

    let base_href = match base_dir {
        Some(dir) => {
            let path_str = dir.to_string_lossy();
            // Keep '/' unescaped so the resulting URI is well-formed.
            let escaped =
                glib::Uri::escape_string(&path_str, Some("/"), false).to_string();
            format!("file://{}/", escaped)
        }
        None => String::new(),
    };

    let theme_css = if dark { PREVIEW_CSS_DARK } else { PREVIEW_CSS_LIGHT };
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
}

// ---- App state --------------------------------------------------------------
struct StateInner {
    window: adw::ApplicationWindow,
    webview: webkit6::WebView,
    buffer: sourceview5::Buffer,
    source_view: sourceview5::View,
    stack: gtk::Stack,
    toggle_btn: gtk::ToggleButton,
    toggle_icon: gtk::Image,
    toggle_label: gtk::Label,
    status_path: gtk::Label,
    status_mode: gtk::Label,

    current_file: RefCell<Option<PathBuf>>,
    is_modified: Cell<bool>,
    mode: RefCell<String>,
    suppress_modify: Cell<bool>,
    toggle_handler: RefCell<Option<glib::SignalHandlerId>>,
    buffer_handler: RefCell<Option<glib::SignalHandlerId>>,
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
        let toggle_btn = gtk::ToggleButton::new();
        let toggle_icon = gtk::Image::from_icon_name("document-edit-symbolic");
        let toggle_label = gtk::Label::new(Some("Edit"));
        let status_path = gtk::Label::new(None);
        let status_mode = gtk::Label::new(None);

        let inner = StateInner {
            window,
            webview,
            buffer,
            source_view,
            stack,
            toggle_btn,
            toggle_icon,
            toggle_label,
            status_path,
            status_mode,
            current_file: RefCell::new(None),
            is_modified: Cell::new(false),
            mode: RefCell::new(MODE_PREVIEW.to_string()),
            suppress_modify: Cell::new(false),
            toggle_handler: RefCell::new(None),
            buffer_handler: RefCell::new(None),
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
            #[strong(rename_to = state)] self,
            move |_| state.action_new()
        ));
        header.pack_start(&new_btn);

        let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
        open_btn.set_tooltip_text(Some("Open\u{2026} (Ctrl+O)"));
        open_btn.connect_clicked(clone!(
            #[strong(rename_to = state)] self,
            move |_| state.action_open()
        ));
        header.pack_start(&open_btn);

        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some("Save (Ctrl+S)"));
        save_btn.connect_clicked(clone!(
            #[strong(rename_to = state)] self,
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
            #[strong(rename_to = state)] self,
            move |btn| {
                let target = if btn.is_active() { MODE_EDIT } else { MODE_PREVIEW };
                state.set_mode(target, false);
            }
        ));
        *s.toggle_handler.borrow_mut() = Some(handler_id);
        header.pack_end(&s.toggle_btn);

        // Stack for content
        s.stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        s.stack.set_transition_duration(140);
        toolbar_view.set_content(Some(&s.stack));

        // --- Preview page ---
        if let Some(ws) = webkit6::prelude::WebViewExt::settings(&s.webview) {
            ws.set_enable_developer_extras(false);
            ws.set_javascript_can_access_clipboard(false);
            ws.set_enable_javascript(true);
        }
        let bg = if self.is_dark() { "#1e1e2e" } else { "#ffffff" };
        if let Some(rgba) = gdk::RGBA::parse(bg).ok() {
            s.webview.set_background_color(&rgba);
        }
        s.webview.connect_decide_policy(clone!(
            #[weak(rename_to = window)] s.window,
            #[upgrade_or] false,
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
            #[strong(rename_to = state)] self,
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
        s.status_mode.add_css_class("dim-label");
        status_bar.append(&s.status_path);
        status_bar.append(&s.status_mode);
        toolbar_view.add_bottom_bar(&status_bar);

        // Track theme changes live
        let style_manager = adw::StyleManager::default();
        style_manager.connect_dark_notify(clone!(
            #[strong(rename_to = state)] self,
            move |_| state.on_theme_changed()
        ));

        // Confirm-on-close
        s.window.connect_close_request(clone!(
            #[strong(rename_to = state)] self,
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

        let edit_section = gio::Menu::new();
        edit_section.append(Some("Undo"), Some("win.undo"));
        edit_section.append(Some("Redo"), Some("win.redo"));
        menu.append_section(Some("Edit"), &edit_section);

        let view_section = gio::Menu::new();
        view_section.append(Some("Toggle Preview / Edit"), Some("win.toggle"));
        menu.append_section(Some("View"), &view_section);

        let help_section = gio::Menu::new();
        help_section.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        help_section.append(
            Some(&format!("About {}", APP_NAME)),
            Some("win.about"),
        );
        menu.append_section(Some("Help"), &help_section);

        menu
    }

    fn wire_actions(&self, app: &adw::Application) {
        let s = &self.inner;

        let actions: Vec<(&str, Box<dyn Fn(&Self)>, &[&str])> = vec![
            ("new", Box::new(|st: &State| st.action_new()), &["<Primary>n"]),
            ("open", Box::new(|st: &State| st.action_open()), &["<Primary>o"]),
            ("save", Box::new(|st: &State| st.action_save()), &["<Primary>s"]),
            (
                "save-as",
                Box::new(|st: &State| st.action_save_as()),
                &["<Primary><Shift>s"],
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
            #[strong(rename_to = state)] self,
            move |_, _| { state.inner.window.close(); }
        ));
        s.window.add_action(&quit_action);
        app.set_accels_for_action("win.quit", &["<Primary>q", "<Primary>w"]);
    }

    fn setup_drag_and_drop(&self) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        target.connect_drop(clone!(
            #[strong(rename_to = state)] self,
            move |_, value, _, _| {
                let Ok(file_list) = value.get::<gdk::FileList>() else {
                    return false;
                };
                let files = file_list.files();
                let Some(first) = files.first() else { return false };
                let Some(path) = first.path() else { return false };
                state.maybe_save_then(Box::new(clone!(
                    #[strong] state,
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
        let text = self.buffer_text();
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
        let path_str = base_dir.to_string_lossy();
        let escaped = glib::Uri::escape_string(&path_str, Some("/"), false).to_string();
        let base_uri = format!("file://{}/", escaped);
        s.webview.load_html(&html, Some(&base_uri));
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
        self.set_mode(MODE_PREVIEW, false);
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
        *self.inner.current_file.borrow_mut() = path;
        self.mark_clean();
        self.update_title();
    }

    fn buffer_text(&self) -> String {
        let (start, end) = self.inner.buffer.bounds();
        self.inner.buffer.text(&start, &end, true).to_string()
    }

    fn on_buffer_changed(&self) {
        if self.inner.suppress_modify.get() {
            return;
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
        let prefix = if self.inner.is_modified.get() { "\u{2022} " } else { "" };
        self.inner
            .window
            .set_title(Some(&format!("{}{} \u{2014} {}", prefix, name, APP_NAME)));
        match current {
            Some(p) => self.inner.status_path.set_text(&p.to_string_lossy()),
            None => self.inner.status_path.set_text("(unsaved document)"),
        }
    }

    fn do_undo(&self) {
        if self.inner.buffer.can_undo() {
            self.inner.buffer.undo();
        }
    }

    fn do_redo(&self) {
        if self.inner.buffer.can_redo() {
            self.inner.buffer.redo();
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
        dialog.connect_response(None, move |_, response| {
            match response {
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
            }
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
        let about = adw::AboutDialog::builder()
            .application_name(APP_NAME)
            .application_icon(APP_ID)
            .developer_name("You + Claude")
            .version(env!("CARGO_PKG_VERSION"))
            .comments(
                "A native GTK4 Markdown viewer/editor with one-key toggle between rendered preview and editing.",
            )
            .license_type(gtk::License::MitX11)
            .website("https://github.com/")
            .build();
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

    // -- Window state persistence ---------------------------------------
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
    }

    fn save_window_state(&self) {
        let dir = settings_dir();
        if let Err(_) = fs::create_dir_all(&dir) {
            return;
        }
        let kf = glib::KeyFile::new();
        let (w, h) = self.inner.window.default_size();
        kf.set_integer("window", "width", w);
        kf.set_integer("window", "height", h);
        kf.set_boolean("window", "maximized", self.inner.window.is_maximized());
        let _ = kf.save_to_file(settings_file());
    }
}

// ---- Icon theme search ------------------------------------------------------
// AdwAboutDialog and the .desktop entry both reference the icon by name
// (`io.github.rendermd.RenderMD`). For the lookup to succeed we need the SVG
// to live in a directory the GTK icon theme searches. Try a few candidate
// locations so this works whether you ran `cargo run`, `./rendermd` from the
// project root, or installed the binary somewhere else.
fn register_icon_search_paths() {
    let Some(display) = gdk::Display::default() else { return };
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
    let Some(request) = action.request() else { return false };
    let Some(uri) = request.uri() else { return false };
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
    static STATE: RefCell<Option<State>> = RefCell::new(None);
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

    app.connect_startup(|_| register_icon_search_paths());

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
