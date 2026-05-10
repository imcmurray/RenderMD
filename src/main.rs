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
</body>
</html>
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
        let annotated_md = build_annotated_diff_md(&old_block, &new_block);
        let prev_html = render_block_to_inline_html(&annotated_md, dark);
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
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
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
        let mut text = self.buffer_text();
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
        if fs::create_dir_all(&dir).is_err() {
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
