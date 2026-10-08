# Removes the AI harness from Windows 10/11, with one command in PowerShell:
#
#   irm https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/uninstall.ps1 | iex
#
# It removes what install.ps1 put there for the harness itself: the folder
# %LOCALAPPDATA%\Programs\harness (harness.exe and its icon), that folder in
# the PATH, and «AI Harness» on the desktop and in the Start menu. Afterwards
# install.ps1 installs the newest version again. Useful for an old harness
# that has no «Update Harness» button yet.
#
# Kept, so a new install finds everything as it was:
#   %USERPROFILE%\.harness   the project list, logins, keys, plugin catalogs, settings
#   the projects             with their own .harness\ folders and tasks
#   Git, Node.js, Zed and the agents, which other programs may use as well
#
# Also remove %USERPROFILE%\.harness (all saved logins and keys; there is no undo):
#   $env:HARNESS_PURGE = "1"; irm ... | iex

function Uninstall-AiHarness {
    $ErrorActionPreference = "Stop"
    try { [Console]::OutputEncoding = [Text.Encoding]::UTF8 } catch {}

    $installDir = Join-Path $env:LOCALAPPDATA "Programs\harness"
    $harnessHome = Join-Path $env:USERPROFILE ".harness"
    $purge = $env:HARNESS_PURGE -eq "1"

    function Say($text) { Write-Host "==> $text" -ForegroundColor Cyan }
    function Note($text) { Write-Host "    $text" }

    function Remove-IfThere($path) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Recurse -Force
            Note "removed $path"
        }
    }

    Say "harness: removing the program"
    try {
        Remove-IfThere $installDir
    } catch {
        throw "cannot remove harness.exe: close the AI Harness window and run this command again"
    }

    # Only the harness's own folder leaves the PATH: the agents' folders stay.
    $user = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($user) {
        $parts = $user.Split(";") | Where-Object { $_ -and ($_.TrimEnd("\") -ne $installDir) }
        $joined = $parts -join ";"
        if ($joined -ne $user) {
            [Environment]::SetEnvironmentVariable("Path", $joined, "User")
            Note "removed $installDir from the PATH"
        }
    }

    Say "«AI Harness» on the desktop and in the Start menu"
    Remove-IfThere (Join-Path ([Environment]::GetFolderPath("Desktop")) "AI Harness.lnk")
    Remove-IfThere (Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\AI Harness.lnk")

    if ($purge) {
        Say ".harness: removing logins, keys and settings"
        Remove-IfThere $harnessHome
    }

    Write-Host ""
    Say "Done."
    if ((-not $purge) -and (Test-Path -LiteralPath $harnessHome)) {
        Note "Kept $harnessHome (projects, logins, settings): a new install uses it again."
    }
    Write-Host ""
    Write-Host "To install the newest version again:" -ForegroundColor Green
    Write-Host "  irm https://raw.githubusercontent.com/aeygenson/ai-harness/main/install/install.ps1 | iex"
}

try {
    Uninstall-AiHarness
} catch {
    Write-Host ""
    Write-Host "Removal stopped: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host "Fix it and run the same command again."
}
