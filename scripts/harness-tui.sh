#!/usr/bin/env bash
# Builds the harness from this repository and opens its TUI.
#
#   scripts/harness-tui.sh [project folder]
#
# Without a folder the TUI opens the project opened last (the list is in
# ~/.harness/projects.toml). The very first time, before that list exists,
# it opens ~/code/harness-test if it is there.
#
# The desktop icon made by scripts/install-desktop-launcher.sh runs this file.
set -u

# Started from the desktop, the shell may not know where cargo is.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
# Nor ~/.local/bin, where Claude Code and Antigravity install themselves
# (the desktop does not read .bashrc or .profile). The roles start them.
case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) PATH="$HOME/.local/bin:$PATH" ;;
esac
export PATH

REPO="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
PROJECT="${1:-$HOME}"
if [ $# -eq 0 ] && [ ! -f "$HOME/.harness/projects.toml" ] && [ -d "$HOME/code/harness-test" ]; then
    PROJECT="$HOME/code/harness-test"
fi

cd "$REPO" || exit 1
echo "Building the harness in $REPO ..."
if cargo build --release -q -p harness-cli; then
    "$REPO/target/release/harness" -C "$PROJECT" tui
    status=$?
else
    status=1
fi

# Keep the window open after an error, so the message can be read.
if [ "$status" -ne 0 ]; then
    echo
    read -r -p "Something went wrong (exit code $status). Press Enter to close. " _
fi
exit "$status"
