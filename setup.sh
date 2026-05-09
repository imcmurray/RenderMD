#!/usr/bin/env bash
# Build the Rust binary and install RenderMD to ~/.local so the `rendermd`
# command is on PATH, the .desktop entry shows up in the menu, the app icon
# resolves in the About dialog, and .md files open in RenderMD on double-click.
#
# Pass --build-only to skip the install step (useful during dev).
# Pass --no-mime to skip making RenderMD the default .md opener.
set -euo pipefail

DIR="$(dirname "$(readlink -f "$0")")"
DESKTOP_NAME="io.github.rendermd.RenderMD.desktop"

INSTALL=1
SET_MIME=1
for arg in "$@"; do
    case "$arg" in
        --build-only|-b) INSTALL=0 ;;
        --no-mime)       SET_MIME=0 ;;
        --help|-h)
            cat <<EOF
Usage: $0 [--build-only|-b] [--no-mime]

  (default)         build the release binary and install to ~/.local
  --build-only, -b  build only, don't install
  --no-mime         install but skip making rendermd the default .md opener
EOF
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            echo "Try: $0 --help" >&2
            exit 1
            ;;
    esac
done

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found. Install with: sudo pacman -S rust" >&2
    exit 1
fi

cd "$DIR"
echo "==> Building release binary"
cargo build --release

if [[ $INSTALL -eq 0 ]]; then
    echo
    echo "Done. Run with: $DIR/rendermd [file.md]"
    exit 0
fi

BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
DATA_DIR="$HOME/.local/share/rendermd"
HICOLOR="$HOME/.local/share/icons/hicolor"
SCALABLE_DIR="$HICOLOR/scalable/apps"
ICON_NAME="io.github.rendermd.RenderMD"
ICON_SVG="$DIR/data/icons/hicolor/scalable/apps/$ICON_NAME.svg"

echo "==> Installing to ~/.local"
mkdir -p "$BIN_DIR" "$APP_DIR" "$DATA_DIR" "$SCALABLE_DIR"

# Binary
install -m 755 "$DIR/target/release/rendermd" "$DATA_DIR/rendermd-bin"

# Launcher: write a fresh one pointed at the installed binary so it works
# regardless of where the source tree lives.
cat > "$BIN_DIR/rendermd" <<EOF
#!/usr/bin/env bash
exec "$DATA_DIR/rendermd-bin" "\$@"
EOF
chmod +x "$BIN_DIR/rendermd"

# Desktop entry. The filename must match the application_id so Wayland
# compositors can correlate windows to this entry and pick up the icon.
install -m 644 "$DIR/$DESKTOP_NAME" "$APP_DIR/$DESKTOP_NAME"
# Clean up the legacy filename from earlier installs (harmless if missing).
rm -f "$APP_DIR/rendermd.desktop"

# Install the scalable SVG
install -m 644 "$ICON_SVG" "$SCALABLE_DIR/$ICON_NAME.svg"

# Render PNG bitmaps at standard sizes — many taskbars/panels prefer
# bitmap variants over SVG and pick up whichever size best matches.
if command -v rsvg-convert >/dev/null 2>&1; then
    for size in 16 24 32 48 64 128 256; do
        size_dir="$HICOLOR/${size}x${size}/apps"
        mkdir -p "$size_dir"
        rsvg-convert -w "$size" -h "$size" "$ICON_SVG" \
            -o "$size_dir/$ICON_NAME.png"
    done
else
    echo "  (rsvg-convert not found — skipping PNG bitmap generation;" >&2
    echo "   install librsvg for full taskbar/panel compatibility)"   >&2
fi

# gtk4-update-icon-cache requires an index.theme. Hicolor's spec ships one
# system-wide but the user-local ~/.local/share/icons/hicolor often doesn't
# have one, so write a minimal stub.
if [[ ! -f "$HICOLOR/index.theme" ]]; then
    cat > "$HICOLOR/index.theme" <<'EOF'
[Icon Theme]
Name=Hicolor
Comment=Fallback icon theme
Hidden=true
Directories=16x16/apps,24x24/apps,32x32/apps,48x48/apps,64x64/apps,96x96/apps,128x128/apps,256x256/apps,scalable/apps

[16x16/apps]
Size=16
Type=Fixed
Context=Applications

[24x24/apps]
Size=24
Type=Fixed
Context=Applications

[32x32/apps]
Size=32
Type=Fixed
Context=Applications

[48x48/apps]
Size=48
Type=Fixed
Context=Applications

[64x64/apps]
Size=64
Type=Fixed
Context=Applications

[96x96/apps]
Size=96
Type=Fixed
Context=Applications

[128x128/apps]
Size=128
Type=Fixed
Context=Applications

[256x256/apps]
Size=256
Type=Fixed
Context=Applications

[scalable/apps]
Size=128
MinSize=16
MaxSize=512
Type=Scalable
Context=Applications
EOF
fi

# Refresh caches — surface errors this time, don't silently swallow.
update-desktop-database "$APP_DIR" || \
    echo "  (update-desktop-database failed — non-fatal)"
if command -v gtk4-update-icon-cache >/dev/null 2>&1; then
    gtk4-update-icon-cache --force "$HICOLOR" || \
        echo "  (gtk4-update-icon-cache failed — non-fatal)"
elif command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache --force "$HICOLOR" || \
        echo "  (gtk-update-icon-cache failed — non-fatal)"
fi

# MIME associations: make rendermd the default .md opener
if [[ $SET_MIME -eq 1 ]] && command -v xdg-mime >/dev/null 2>&1; then
    xdg-mime default "$DESKTOP_NAME" text/markdown    2>/dev/null || true
    xdg-mime default "$DESKTOP_NAME" text/x-markdown  2>/dev/null || true
fi

# Budgie's menu applet caches its application list and won't pick up a newly
# installed .desktop until the panel is replaced (or the user logs out/in).
# Kick off a detached --replace so the entry appears immediately. No-op on
# non-Budgie sessions and during builds without a running panel.
if command -v budgie-panel >/dev/null 2>&1 && pgrep -x budgie-panel >/dev/null 2>&1; then
    setsid budgie-panel --replace </dev/null >/dev/null 2>&1 &
    disown
fi

echo
echo "Installed:"
echo "  Binary    $DATA_DIR/rendermd-bin"
echo "  Launcher  $BIN_DIR/rendermd"
echo "  Desktop   $APP_DIR/$DESKTOP_NAME"
echo "  Icon      $HICOLOR/{16,24,32,48,64,128,256}x*/apps/$ICON_NAME.png"
echo "            $SCALABLE_DIR/$ICON_NAME.svg"

# PATH check
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo
        echo "Warning: $BIN_DIR is not on your PATH."
        echo "Add this to your shell rc:"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac

echo
echo "Run with: rendermd [file.md]"
