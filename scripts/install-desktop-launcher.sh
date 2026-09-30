#!/usr/bin/env bash
# Puts an "AI Harness" icon on the desktop and in the applications menu.
# The icon opens a terminal and runs scripts/harness-tui.sh.
#
#   scripts/install-desktop-launcher.sh [project folder]   (default: ~/code/harness-test)
set -eu

REPO="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
PROJECT="${1:-$HOME/code/harness-test}"
DESKTOP_DIR="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
MENU_DIR="$HOME/.local/share/applications"

entry() {
    cat <<ENTRY
[Desktop Entry]
Type=Application
Name=AI Harness
Comment=Show the harness tasks of $PROJECT
Exec="$REPO/scripts/harness-tui.sh" "$PROJECT"
Terminal=true
Icon=utilities-terminal
Categories=Development;
ENTRY
}

chmod +x "$REPO/scripts/harness-tui.sh"
mkdir -p "$DESKTOP_DIR" "$MENU_DIR"
for dir in "$DESKTOP_DIR" "$MENU_DIR"; do
    entry > "$dir/ai-harness.desktop"
    chmod +x "$dir/ai-harness.desktop"
done
# GNOME asks before it runs an icon it does not trust yet.
gio set "$DESKTOP_DIR/ai-harness.desktop" metadata::trusted true 2>/dev/null || true

echo "Added $DESKTOP_DIR/ai-harness.desktop and $MENU_DIR/ai-harness.desktop"
