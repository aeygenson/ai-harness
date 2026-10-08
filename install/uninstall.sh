#!/usr/bin/env bash
# Removes the AI harness from macOS or Linux, with one command in the Terminal:
#
#   curl -fsSL https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/uninstall.sh | bash
#
# It removes what install.sh put there for the harness itself: the `harness`
# program in ~/.local/bin, «AI Harness» in the applications (and on the Linux
# desktop) and its icon. Afterwards install.sh installs the newest version
# again. Useful for an old harness that has no «Update Harness» button yet.
#
# Kept, so a new install finds everything as it was:
#   ~/.harness          the project list, logins, keys, plugin catalogs, settings
#   the projects        with their own .harness/ folders and tasks
#   ~/code/ai-harness   the source code of --dev, if it is there
#   Git, Node.js, Zed and the agents, which other programs may use as well
#
# Also remove ~/.harness (all saved logins and keys; there is no undo):
#   curl -fsSL .../uninstall.sh | bash -s -- --purge
set -euo pipefail

BIN="$HOME/.local/bin"
SHARE="$HOME/.local/share/ai-harness"
PURGE=0
[ "${1:-}" = "--purge" ] && PURGE=1

say() { printf '\033[36m==> %s\033[0m\n' "$*"; }
note() { printf '    %s\n' "$*"; }
has() { command -v "$1" >/dev/null 2>&1; }

# Removes each path that exists and says so.
remove() {
    local path
    for path in "$@"; do
        if [ -e "$path" ] || [ -L "$path" ]; then
            rm -rf "$path"
            note "removed $path"
        fi
    done
}

say "harness: removing the program"
# A --dev install was made with `cargo install --root ~/.local`: cargo forgets it too.
if grep -qs '^"harness-cli ' "$HOME/.local/.crates.toml" && has cargo; then
    cargo uninstall --root "$HOME/.local" harness-cli >/dev/null 2>&1 || true
fi
# harness.new and harness.old may be left by an update that was stopped half-way.
remove "$BIN/harness" "$BIN/harness.new" "$BIN/harness.old"

say "«AI Harness» in the applications"
if [ "$(uname -s)" = Darwin ]; then
    remove "$HOME/Applications/AI Harness.app"
else
    desktop="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
    remove "$HOME/.local/share/applications/ai-harness.desktop" "$desktop/ai-harness.desktop"
fi
remove "$SHARE"

if [ "$PURGE" = 1 ]; then
    say "~/.harness: removing logins, keys and settings"
    remove "$HOME/.harness"
fi

other="$(command -v harness 2>/dev/null || true)"
echo
say "Done."
if [ -n "$other" ]; then
    note "Another harness is still found at $other (installed some other way); remove it yourself."
fi
if [ "$PURGE" = 0 ] && [ -d "$HOME/.harness" ]; then
    note "Kept ~/.harness (projects, logins, settings): a new install uses it again."
fi
cat <<'NEXT'

To install the newest version again:
  curl -fsSL https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.sh | bash
NEXT
