# unhosted Windows installer (PowerShell)
#
# Usage:
#   irm https://raw.githubusercontent.com/unhosted-ai/unhosted-core/main/scripts/install.ps1 | iex
#
# Env vars:
#   $env:UNHOSTED_INSTALL_DIR   override install directory (default %LOCALAPPDATA%\unhosted)
#   $env:UNHOSTED_VERSION       pin a specific version (default: latest)
#   $env:UNHOSTED_NO_DESKTOP    set to "1" to skip the desktop shell (CLI only)

$ErrorActionPreference = "Stop"

$Repo       = "unhosted-ai/unhosted-core"
$InstallDir = if ($env:UNHOSTED_INSTALL_DIR) { $env:UNHOSTED_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "unhosted" }
$Version    = if ($env:UNHOSTED_VERSION) { $env:UNHOSTED_VERSION } else { "latest" }
$NoDesktop  = ($env:UNHOSTED_NO_DESKTOP -eq "1")

# ---- detect arch -------------------------------------------------------------

$arch = (Get-CimInstance -ClassName Win32_Processor).Architecture
switch ($arch) {
    9       { $Target = "x86_64-pc-windows-msvc" }   # AMD64
    12      { Write-Error "unhosted: ARM64 Windows builds aren't published yet. Build from source."; exit 1 }
    default { Write-Error "unhosted: unsupported architecture code '$arch'"; exit 1 }
}

Write-Host "unhosted installer"
Write-Host "  platform: windows / $Target"
Write-Host "  install:  $InstallDir\unhosted.exe"
Write-Host ""

# ---- find release ------------------------------------------------------------

if ($Version -eq "latest") {
    $api = "https://api.github.com/repos/$Repo/releases/latest"
} else {
    $api = "https://api.github.com/repos/$Repo/releases/tags/$Version"
}

$rel = Invoke-RestMethod -Uri $api -UserAgent "unhosted-installer"
$asset = $rel.assets | Where-Object { $_.name -eq "unhosted-$Target.zip" } | Select-Object -First 1
if (-not $asset) {
    Write-Error "unhosted: no release asset found for $Target in $Version."
    exit 1
}

# ---- download + extract ------------------------------------------------------

$tmp = Join-Path $env:TEMP ("unhosted-install-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

$zip = Join-Path $tmp "unhosted.zip"
Write-Host "  downloading $($asset.browser_download_url) ..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip -UseBasicParsing

Expand-Archive -Path $zip -DestinationPath $tmp -Force

# Find both binaries inside the extracted "unhosted-<target>/" directory.
$cliExe = Get-ChildItem -Path $tmp -Recurse -Filter "unhosted.exe" `
    | Where-Object { $_.Name -eq "unhosted.exe" } | Select-Object -First 1
$desktopExe = Get-ChildItem -Path $tmp -Recurse -Filter "unhosted-desktop.exe" | Select-Object -First 1

if (-not $cliExe) {
    Write-Error "unhosted: archive did not contain unhosted.exe"
    exit 1
}

# ---- install binaries --------------------------------------------------------

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $cliExe.FullName -Destination (Join-Path $InstallDir "unhosted.exe") -Force

$desktopPath = $null
if ($desktopExe -and (-not $NoDesktop)) {
    $desktopPath = Join-Path $InstallDir "unhosted-desktop.exe"
    Copy-Item -Path $desktopExe.FullName -Destination $desktopPath -Force
    Write-Host "  installed: $desktopPath"
}

# Add to user PATH if not already there.
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Host "  added $InstallDir to your user PATH (restart your shell)"
}

# ---- Start Menu shortcut for the desktop shell ------------------------------

if ($desktopPath -and (Test-Path $desktopPath)) {
    $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $lnk = Join-Path $startMenu "Unhosted.lnk"
    try {
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($lnk)
        $shortcut.TargetPath  = $desktopPath
        $shortcut.WorkingDirectory = $InstallDir
        $shortcut.Description = "Unhosted — your local AI mesh"
        $shortcut.Save()
        Write-Host "  installed: $lnk"
    } catch {
        Write-Host "  could not create Start Menu shortcut ($_)" -ForegroundColor Yellow
    }
}

Remove-Item -Path $tmp -Recurse -Force

# ---- verify ------------------------------------------------------------------

Write-Host ""
& (Join-Path $InstallDir "unhosted.exe") --version

Write-Host ""
Write-Host "next:"
Write-Host "  1. install llama.cpp: see https://github.com/ggerganov/llama.cpp/releases (windows builds)"
Write-Host "  2. pull a model:      unhosted pull llama3.2:1b"
Write-Host "  3. start the backend: llama-server.exe -m <model.gguf> --port 8080"
Write-Host "  4. run the daemon:    unhosted serve"
if ($desktopPath) {
    Write-Host "  5. open the app:      unhosted-desktop   (or click Unhosted in the Start Menu)"
} else {
    Write-Host "  5. open the app:      start http://127.0.0.1:7777"
}
