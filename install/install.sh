#!/usr/bin/env bash
# Installs the AI harness and everything it needs on macOS or Linux, with one
# command in the Terminal:
#
#   curl -fsSL https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.sh | bash
#
# What is missing is installed, what is old is updated, what is new is left
# alone, so running the same command again is safe. The harness itself
# updates from inside later («Update Harness» in the TUI, `harness update`);
# uninstall.sh removes it.
#
#   Git, Node.js      Homebrew on a Mac, the system's packages on Linux
#   Zed               Homebrew on a Mac, Zed's own installer on Linux
#   harness           the ready program from the Releases page
#
# The agents (Claude Code, Codex CLI, Antigravity CLI and the others) are not
# installed here: each person installs and signs in to the ones they have a
# subscription for, on the harness's «Agents» tab.
#
# Developer mode builds the harness from source with Rust instead (and keeps
# the code in ~/code/ai-harness):
#   curl -fsSL .../install.sh | bash -s -- --dev
#
# At the end it adds «AI Harness» to the applications (on Linux to the
# desktop too). It never asks for or prints a key.
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

# A long, quiet step (Homebrew, a download) runs with a spinner and the
# seconds it has taken, so the window never looks frozen. Its own output goes
# to a file and is shown only when the step fails. The step must not ask
# anything: its keyboard is closed.
spin() { # spin <message> <command> [arguments...]
    local text="$1" log pid start code=0 frame=0
    local frames=('|' '/' '-' '\')
    shift
    log="$(mktemp)"
    start=$SECONDS
    "$@" >"$log" 2>&1 </dev/null &
    pid=$!
    # Without a terminal (CI, a log file) there is nothing to animate.
    if [ -t 1 ]; then
        while kill -0 "$pid" 2>/dev/null; do
            printf '\r    %s %s (%ds) ' "${frames[frame % 4]}" "$text" $((SECONDS - start))
            frame=$((frame + 1))
            sleep 0.2
        done
        printf '\r\033[K'
    fi
    wait "$pid" || code=$?
    if [ "$code" = 0 ]; then
        printf '    \033[32m✓\033[0m %s (%ds)\n' "$text" $((SECONDS - start))
    else
        printf '    \033[31m✗\033[0m %s (%ds)\n' "$text" $((SECONDS - start))
        tail -n 20 "$log" | sed 's/^/      /'
    fi
    rm -f "$log"
    return "$code"
}

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
TOOLS="git node harness"
BEFORE=""
for tool in $TOOLS; do BEFORE="$BEFORE$tool|$(version_of "$tool")
"; done
before_of() { printf '%s' "$BEFORE" | grep "^$1|" | cut -d'|' -f2-; }

# --- Git, Node.js, Zed ---------------------------------------------------

brew_package() { # brew_package <name> [--cask]
    if brew list "$@" >/dev/null 2>&1; then
        # Not 0 also when there is simply nothing newer.
        spin "$1: checking for a newer version" brew upgrade "$@" || true
    else
        spin "$1: installing" brew install "$@" || fail "Homebrew could not install $1"
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
    say "Git, Node.js, Zed: installing or updating with Homebrew (a few minutes)"
    # The first `brew update` in a while can take minutes.
    spin "Homebrew: updating its list of programs" brew update || true
    brew_package git
    brew_package node
    brew_package zed --cask
}

# The system's package manager; `sudo` asks for the password once.
linux_packages() {
    if has apt-get; then
        from_keyboard sudo apt-get update -q
        from_keyboard sudo apt-get install -y -q "$@"
    elif has dnf; then
        from_keyboard sudo dnf install -y "$@"
    elif has pacman; then
        from_keyboard sudo pacman -S --needed --noconfirm "$@"
    elif has zypper; then
        from_keyboard sudo zypper --non-interactive install "$@"
    else
        fail "no known package manager (apt, dnf, pacman, zypper); install $* yourself"
    fi
}

# Node.js 22.19 or newer (DeepSeek Harness needs it). Many distributions
# ship an older one, so when the system's is missing or old, the current
# Node 22 goes into ~/.local.
NODE_DIR="$HOME/.local/share/node"
linux_node() {
    local major=0 minor=0 arch file
    if has node; then
        major="$(node --version | sed 's/^v//; s/\..*//')"
        minor="$(node --version | sed 's/^v[0-9]*\.//; s/\..*//')"
    fi
    if { [ "$major" -gt 22 ] || { [ "$major" -eq 22 ] && [ "$minor" -ge 19 ]; }; } &&
        [ ! -x "$NODE_DIR/bin/node" ]; then
        note "Node.js $(node --version) is new enough"
        return
    fi
    case "$ARCH" in
        x86_64) arch=x64 ;;
        aarch64 | arm64) arch=arm64 ;;
        *) fail "no Node.js for $ARCH; install Node.js 22.19 or newer yourself" ;;
    esac
    say "Node.js: installing the newest 22.x into $NODE_DIR"
    local base=https://nodejs.org/dist/latest-v22.x
    file="$(curl -fsSL "$base/SHASUMS256.txt" | grep -o "node-v[0-9.]*-linux-$arch.tar.xz" | head -n 1)"
    [ -n "$file" ] || fail "cannot find Node.js on nodejs.org"
    rm -rf "$NODE_DIR"
    mkdir -p "$NODE_DIR"
    spin "Node.js: downloading" download "$base/$file" "$NODE_DIR" -J --strip-components 1 ||
        fail "cannot download Node.js"
    for program in node npm npx; do ln -sf "$NODE_DIR/bin/$program" "$BIN/$program"; done
}

linux_basics() {
    say "Git, curl, bubblewrap: installing or updating with the system's packages"
    # xz unpacks Node.js; Debian and Ubuntu call it xz-utils. Bubblewrap lets
    # Claude Code hide its login from the commands the agent runs.
    if has apt-get; then
        linux_packages git curl xz-utils bubblewrap
    else
        linux_packages git curl xz bubblewrap
    fi
    linux_node
    # Zed's own installer puts it in ~/.local and also updates it.
    say "Zed: installing the newest"
    run_installer https://zed.dev/install.sh
}

# --- The harness ---------------------------------------------------------

# Downloads an archive and unpacks it into a folder (extra arguments go to tar).
download() { # download <url> <folder> [tar arguments...]
    local url="$1" folder="$2"
    shift 2
    curl -fsSL "$url" | tar -x "$@" -C "$folder"
}

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
    say "harness: the newest version"
    tmp="$(mktemp -d)"
    spin "harness: downloading" \
        download "https://github.com/$REPO/releases/latest/download/harness-$target.tar.gz" "$tmp" -z ||
        fail "cannot download the harness for $target"
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
        say "«AI Harness» in the applications menu and on the desktop"
        local share="$HOME/.local/share/ai-harness"
        local menu="$HOME/.local/share/applications"
        mkdir -p "$share" "$menu"
        # Started from the desktop, nothing from ~/.bashrc or ~/.profile is
        # loaded, so the roles would not find the tools installed there
        # (dotnet, node, anything). This starts the harness inside the
        # person's own shell, the way a terminal does.
        cat >"$share/start.sh" <<'START'
#!/usr/bin/env bash
case "${SHELL:-}" in
    */bash | */zsh | */ksh) user_shell="$SHELL" ;;
    *) user_shell=/bin/bash ;; # fish and others take other arguments
esac
# A login bash reads ~/.profile but not ~/.bashrc, so read that too.
exec "$user_shell" -l -i -c \
    '[ -n "${BASH_VERSION:-}" ] && [ -f ~/.bashrc ] && . ~/.bashrc; exec "$0" tui' \
    "$1"
START
        chmod +x "$share/start.sh"
        local desktop
        desktop="$(xdg-user-dir DESKTOP 2>/dev/null || echo "$HOME/Desktop")"
        for dir in "$menu" "$desktop"; do
            [ -d "$dir" ] || continue
            cat >"$dir/ai-harness.desktop" <<ENTRY
[Desktop Entry]
Type=Application
Name=AI Harness
Comment=Tasks, roles and projects of the AI harness
Exec="$share/start.sh" "$BIN/harness"
Terminal=true
Icon=$share/ai-harness-256.png
Categories=Development;
ENTRY
            chmod +x "$dir/ai-harness.desktop"
        done
        # GNOME asks before it runs an icon it does not trust yet.
        gio set "$desktop/ai-harness.desktop" metadata::trusted true 2>/dev/null || true
    fi
}

# --- Everything, in order ------------------------------------------------

if [ "$ONLY_HARNESS" = 0 ]; then
    if [ "$SYSTEM" = mac ]; then mac_basics; else linux_basics; fi
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

Next: start «AI Harness» from your applications and open the «Agents» tab
(key 8). Install the agents you have a subscription for and press «Sign in».
NEXT
