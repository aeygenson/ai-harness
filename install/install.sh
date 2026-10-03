#!/usr/bin/env bash
# Installs the AI harness and everything it needs on macOS or Linux, with one
# command in the Terminal:
#
#   curl -fsSL https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.sh | bash
#
# What is missing is installed, what is old is updated, what is new is left
# alone, so running the same command again later is the update.
#
#   Git, Node.js      Homebrew on a Mac, the system's packages on Linux
#   Zed               Homebrew on a Mac, Zed's own installer on Linux
#   Claude Code       its official installer
#   Codex CLI         npm
#   Antigravity CLI   its official installer
#   harness           the ready program from the Releases page
#
# Developer mode builds the harness from source with Rust instead (and keeps
# the code in ~/code/ai-harness):
#   curl -fsSL .../install.sh | bash -s -- --dev
#
# At the end it adds «AI Harness» to the applications and says how to sign in
# to the three agents. It never asks for or prints a key.
#
# Only the harness itself, nothing else:  HARNESS_ONLY=harness
set -euo pipefail

REPO="aeygenson/ai-harness"
BIN="$HOME/.local/bin"
DEV=0
[ "${1:-}" = "--dev" ] && DEV=1
ONLY_HARNESS=0
[ "${HARNESS_ONLY:-}" = "harness" ] && ONLY_HARNESS=1

say() { printf '\033[36m==> %s\033[0m\n' "$*"; }
note() { printf '    %s\n' "$*"; }
fail() {
    printf '\033[31mInstallation stopped: %s\033[0m\n' "$*" >&2
    echo "Fix it and run the same command again; what is already installed is kept." >&2
    exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

# Questions (sudo's password, Homebrew's «Press RETURN») must come from the
# keyboard: with `curl | bash` the standard input is the script itself.
from_keyboard() {
    if [ -r /dev/tty ]; then "$@" </dev/tty; else "$@"; fi
}

# The first line of `<program> --version`, or "-" when it is not installed.
version_of() {
    if has "$1"; then
        "$1" --version 2>/dev/null | head -n 1 || echo "?"
    else
        echo "-"
    fi
}

# Another official installer: downloaded first, so it can still ask questions.
run_installer() {
    local file
    file="$(mktemp)"
    curl -fsSL "$1" -o "$file" || fail "cannot download $1"
    from_keyboard bash "$file" || fail "the installer from $1 failed"
    rm -f "$file"
}

mkdir -p "$BIN"
case ":$PATH:" in *":$BIN:"*) ;; *) export PATH="$BIN:$PATH" ;; esac

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
    Darwin) SYSTEM=mac ;;
    Linux) SYSTEM=linux ;;
    *) fail "this script is for macOS and Linux; on Windows use install.ps1" ;;
esac

# Versions before, one «tool|version» per line (the Mac's bash 3.2 has no
# associative arrays).
TOOLS="git node claude codex agy harness"
BEFORE=""
for tool in $TOOLS; do BEFORE="$BEFORE$tool|$(version_of "$tool")
"; done
before_of() { printf '%s' "$BEFORE" | grep "^$1|" | cut -d'|' -f2-; }

# --- Git, Node.js, Zed ---------------------------------------------------

brew_package() { # brew_package <name> [--cask]
    if brew list "$@" >/dev/null 2>&1; then
        say "$1: checking for a newer version"
        brew upgrade "$@" >/dev/null 2>&1 && note "up to date" || note "already the newest"
    else
        say "$1: installing"
        brew install "$@" || fail "Homebrew could not install $1"
    fi
}

mac_basics() {
    if ! has brew; then
        say "Homebrew: installing (it may ask for your Mac password)"
        run_installer https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh
    fi
    for prefix in /opt/homebrew /usr/local; do
        [ -x "$prefix/bin/brew" ] && eval "$("$prefix/bin/brew" shellenv)" && break
    done
    has brew || fail "Homebrew was installed but is not found; open a new Terminal and run again"
    brew update >/dev/null 2>&1 || true
    brew_package git
    brew_package node
    brew_package zed --cask
}

# The system's package manager; `sudo` asks for the password once.
linux_packages() {
    if has apt-get; then
        from_keyboard sudo apt-get update -qq
        from_keyboard sudo apt-get install -y -qq "$@"
    elif has dnf; then
        from_keyboard sudo dnf install -y -q "$@"
    elif has pacman; then
        from_keyboard sudo pacman -S --needed --noconfirm "$@"
    elif has zypper; then
        from_keyboard sudo zypper --non-interactive install "$@"
    else
        fail "no known package manager (apt, dnf, pacman, zypper); install $* yourself"
    fi
}

# Node.js 20 or newer. Many distributions ship an older one, so when the
# system's is missing or old, the current Node 22 goes into ~/.local.
NODE_DIR="$HOME/.local/share/node"
linux_node() {
    local major=0 arch file
    has node && major="$(node --version | sed 's/^v//; s/\..*//')"
    if [ "$major" -ge 20 ] && [ ! -x "$NODE_DIR/bin/node" ]; then
        note "Node.js $(node --version) is new enough"
        return
    fi
    case "$ARCH" in
        x86_64) arch=x64 ;;
        aarch64 | arm64) arch=arm64 ;;
        *) fail "no Node.js for $ARCH; install Node.js 20 or newer yourself" ;;
    esac
    say "Node.js: installing the newest 22.x into $NODE_DIR"
    local base=https://nodejs.org/dist/latest-v22.x
    file="$(curl -fsSL "$base/SHASUMS256.txt" | grep -o "node-v[0-9.]*-linux-$arch.tar.xz" | head -n 1)"
    [ -n "$file" ] || fail "cannot find Node.js on nodejs.org"
    rm -rf "$NODE_DIR"
    mkdir -p "$NODE_DIR"
    curl -fsSL "$base/$file" | tar -xJ -C "$NODE_DIR" --strip-components 1 ||
        fail "cannot download Node.js"
    for program in node npm npx; do ln -sf "$NODE_DIR/bin/$program" "$BIN/$program"; done
}

linux_basics() {
    say "Git, curl: installing or updating with the system's packages"
    # xz unpacks Node.js; Debian and Ubuntu call it xz-utils.
    if has apt-get; then linux_packages git curl xz-utils; else linux_packages git curl xz; fi
    linux_node
    # Zed's own installer puts it in ~/.local and also updates it.
    say "Zed: installing the newest"
    run_installer https://zed.dev/install.sh
}

# --- The three agents ----------------------------------------------------

agents() {
    if has claude; then
        say "Claude Code: updating"
        claude update || note "could not update; it also updates itself"
    else
        say "Claude Code: installing"
        run_installer https://claude.ai/install.sh
    fi

    # Codex: into Homebrew's Node on a Mac, into ~/.local on Linux (no sudo).
    say "Codex CLI: installing the newest"
    if [ "$SYSTEM" = mac ]; then
        npm install -g --loglevel=error @openai/codex@latest >/dev/null || fail "npm could not install Codex CLI"
    else
        npm install -g --loglevel=error --prefix "$HOME/.local" @openai/codex@latest >/dev/null ||
            fail "npm could not install Codex CLI"
    fi

    say "Antigravity CLI: installing the newest"
    run_installer https://antigravity.google/cli/install.sh
}

# --- The harness ---------------------------------------------------------

release_target() {
    case "$SYSTEM-$ARCH" in
        mac-arm64) echo aarch64-apple-darwin ;;
        mac-x86_64) echo x86_64-apple-darwin ;;
        linux-x86_64) echo x86_64-unknown-linux-musl ;;
        linux-aarch64 | linux-arm64) echo aarch64-unknown-linux-musl ;;
        *) echo "" ;;
    esac
}

harness_ready() {
    local target="$1" tmp
    say "harness: downloading the newest version"
    tmp="$(mktemp -d)"
    curl -fsSL "https://github.com/$REPO/releases/latest/download/harness-$target.tar.gz" |
        tar -xz -C "$tmp" || fail "cannot download the harness for $target"
    install -m 755 "$tmp/harness" "$BIN/harness"
    mkdir -p "$HOME/.local/share/ai-harness"
    cp "$tmp/ai-harness-256.png" "$HOME/.local/share/ai-harness/" 2>/dev/null || true
    rm -rf "$tmp"
}

harness_dev() {
    local code="$HOME/code/ai-harness"
    if has rustup; then
        say "Rust: updating"
        rustup update stable >/dev/null
    else
        say "Rust: installing"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path >/dev/null
    fi
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
    if [ -d "$code/.git" ]; then
        say "harness: updating the code in $code"
        git -C "$code" pull --ff-only || fail "cannot update $code (changes of your own there?)"
    else
        say "harness: getting the code into $code"
        mkdir -p "$HOME/code"
        git clone "https://github.com/$REPO.git" "$code"
    fi
    say "harness: building (a few minutes the first time)"
    cargo install --path "$code/crates/harness-cli" --locked --root "$HOME/.local" ||
        fail "the build failed"
    mkdir -p "$HOME/.local/share/ai-harness"
    cp "$code/assets/icons/ai-harness-256.png" "$HOME/.local/share/ai-harness/"
}

# --- PATH and the application icon --------------------------------------

remember_path() {
    local line='export PATH="$HOME/.local/bin:$PATH"' file
    for file in "$HOME/.zprofile" "$HOME/.bashrc" "$HOME/.profile"; do
        case "$file" in
            *.zprofile) [ "$SYSTEM" = mac ] || continue ;;
            *.bashrc) [ -f "$file" ] || continue ;;
        esac
        grep -qsF "$line" "$file" || printf '\n%s\n' "$line" >>"$file"
    done
}

shortcut() {
    if [ "$SYSTEM" = mac ]; then
        say "«AI Harness» in your Applications"
        mkdir -p "$HOME/Applications"
        rm -rf "$HOME/Applications/AI Harness.app"
        osacompile -o "$HOME/Applications/AI Harness.app" \
            -e "tell application \"Terminal\" to do script \"'$BIN/harness' tui\"" \
            -e 'tell application "Terminal" to activate' >/dev/null
    else
        say "«AI Harness» in the applications menu"
        local menu="$HOME/.local/share/applications"
        mkdir -p "$menu"
        cat >"$menu/ai-harness.desktop" <<ENTRY
[Desktop Entry]
Type=Application
Name=AI Harness
Comment=Tasks, roles and projects of the AI harness
Exec=$BIN/harness tui
Terminal=true
Icon=$HOME/.local/share/ai-harness/ai-harness-256.png
Categories=Development;
ENTRY
    fi
}

# --- Everything, in order ------------------------------------------------

if [ "$ONLY_HARNESS" = 0 ]; then
    if [ "$SYSTEM" = mac ]; then mac_basics; else linux_basics; fi
    agents
fi

target="$(release_target)"
if [ "$DEV" = 1 ] || [ -z "$target" ]; then
    [ -z "$target" ] && note "no ready program for $OS $ARCH: building it with Rust"
    harness_dev
else
    harness_ready "$target"
fi
remember_path
shortcut

echo
say "Done. Versions (was -> now):"
for tool in $TOOLS; do
    printf '    %-8s %s  ->  %s\n' "$tool" "$(before_of "$tool")" "$(version_of "$tool")"
done
cat <<'NEXT'

Last step, once: sign in to the agents. Open a NEW Terminal window and run:
    harness login claude        (it asks for a token: run 'claude setup-token' in another window)
    harness login codex         (a browser opens)
    harness login antigravity   (sign in with Google, then type /quit)
Then start «AI Harness» from your applications.
NEXT
