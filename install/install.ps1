# Installs the AI harness and everything it needs on Windows 10/11, with one
# command in PowerShell:
#
#   irm https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.ps1 | iex
#
# What is missing is installed, what is old is updated, what is new is left
# alone, so running the same command again is safe. The harness itself
# updates from inside later («Update Harness» in the TUI, `harness update`);
# uninstall.ps1 removes it.
#
#   Git, Node.js, Zed      through winget (comes with Windows)
#   harness                the ready program from the Releases page
#
# The agents (Claude Code, Codex CLI, Antigravity CLI and the others) are not
# installed here: each person installs and signs in to the ones they have a
# subscription for, on the harness's «Agents» tab.
#
# At the end it puts «AI Harness» on the desktop and in the Start menu. It
# never asks for or prints a key.
#
# Without winget's packages (only the harness itself):
#   $env:HARNESS_ONLY = "harness"; irm ... | iex

function Install-AiHarness {
    $ErrorActionPreference = "Stop"
    $ProgressPreference = "SilentlyContinue"   # Invoke-WebRequest is slow with it
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch {}

    $repo = "aeygenson/ai-harness"
    $archive = "harness-x86_64-pc-windows-msvc.zip"
    $installDir = Join-Path $env:LOCALAPPDATA "Programs\harness"
    $onlyHarness = $env:HARNESS_ONLY -eq "harness"

    function Say($text) { Write-Host "==> $text" -ForegroundColor Cyan }
    function Warn($text) { Write-Host "    $text" -ForegroundColor Yellow }

    # Programs installed a moment ago are not in this window's PATH yet.
    function Update-Path {
        $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
        $user = [Environment]::GetEnvironmentVariable("Path", "User")
        $env:Path = "$machine;$user"
    }

    function Add-UserPath($dir) {
        $user = [Environment]::GetEnvironmentVariable("Path", "User")
        $parts = @()
        if ($user) { $parts = $user.Split(";") | Where-Object { $_ } }
        if ($parts -notcontains $dir) {
            [Environment]::SetEnvironmentVariable("Path", (($parts + $dir) -join ";"), "User")
        }
        if (($env:Path.Split(";")) -notcontains $dir) { $env:Path = "$env:Path;$dir" }
    }

    function Has($command) { [bool](Get-Command $command -ErrorAction SilentlyContinue) }

    # The first line of `<command> --version`, or "-" when it is not installed.
    function Version-Of($command) {
        if (-not (Has $command)) { return "-" }
        try {
            $line = & $command --version 2>$null | Select-Object -First 1
            if ($line) { return "$line".Trim() } else { return "?" }
        } catch { return "?" }
    }

    # Another installer runs in its own PowerShell: if it ends with `exit`,
    # only that one ends, not this script.
    function Run-Installer($url) {
        & powershell -NoProfile -ExecutionPolicy Bypass -Command "irm '$url' | iex"
        if ($LASTEXITCODE -ne 0) { throw "the installer from $url failed" }
    }

    function Winget-Package($id, $name) {
        & winget list --id $id --exact --accept-source-agreements *> $null
        if ($LASTEXITCODE -eq 0) {
            Say "${name}: checking for a newer version"
            & winget upgrade --id $id --exact --silent --accept-package-agreements --accept-source-agreements *> $null
            # Not 0 also when there is simply nothing newer.
            if ($LASTEXITCODE -eq 0) { Warn "updated" } else { Warn "already the newest" }
        } else {
            Say "${name}: installing"
            & winget install --id $id --exact --silent --accept-package-agreements --accept-source-agreements
            if ($LASTEXITCODE -ne 0) { throw "winget could not install $name ($id)" }
        }
    }

    $tools = @("git", "node", "harness")
    $before = @{}
    foreach ($tool in $tools) { $before[$tool] = Version-Of $tool }

    if (-not $onlyHarness) {
        if (-not (Has "winget")) {
            throw "winget was not found. Install «App Installer» from the Microsoft Store and run this command again."
        }
        Winget-Package "Git.Git" "Git"
        Winget-Package "OpenJS.NodeJS.LTS" "Node.js"
        Winget-Package "ZedIndustries.Zed" "Zed"
        Update-Path
    }

    # Where the agents' installers put them (Claude Code and Antigravity in
    # their own folders, npm in %APPDATA%\npm): in PATH already, so an agent
    # installed later on the Agents tab works in every new window.
    Add-UserPath (Join-Path $env:USERPROFILE ".local\bin")
    Add-UserPath (Join-Path $env:APPDATA "npm")
    Add-UserPath (Join-Path $env:LOCALAPPDATA "agy\bin")

    # The harness: the newest ready program from the Releases page.
    Say "harness: downloading the newest version"
    $zip = Join-Path $env:TEMP $archive
    $url = "https://github.com/$repo/releases/latest/download/$archive"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    try {
        Expand-Archive -Path $zip -DestinationPath $installDir -Force
    } catch {
        throw "cannot replace harness.exe: close the AI Harness window and run this command again"
    }
    Remove-Item $zip -Force
    Add-UserPath $installDir

    # «AI Harness» on the desktop and in the Start menu: opens the TUI with
    # the project opened last.
    Say "Shortcuts: desktop and Start menu"
    $shell = New-Object -ComObject WScript.Shell
    $places = @(
        [Environment]::GetFolderPath("Desktop"),
        (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs")
    )
    foreach ($place in $places) {
        $link = $shell.CreateShortcut((Join-Path $place "AI Harness.lnk"))
        $link.TargetPath = Join-Path $installDir "harness.exe"
        $link.Arguments = "tui"
        $link.WorkingDirectory = $env:USERPROFILE
        $link.IconLocation = Join-Path $installDir "ai-harness.ico"
        $link.Description = "AI Harness"
        $link.Save()
    }

    Write-Host ""
    Say "Done. Versions (was -> now):"
    foreach ($tool in $tools) {
        $now = Version-Of $tool
        Write-Host ("    {0,-8} {1}  ->  {2}" -f $tool, $before[$tool], $now)
    }
    Write-Host ""
    Write-Host "Next: start «AI Harness» from the desktop and open the «Agents» tab (key 8)." -ForegroundColor Green
    Write-Host "Install the agents you have a subscription for and press «Sign in»."
}

try {
    Install-AiHarness
} catch {
    Write-Host ""
    Write-Host "Installation stopped: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host "Fix it and run the same command again; what is already installed is kept."
}
