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

# ---- helpers -----------------------------------------------------------------

function Write-Header { param($msg)
    Write-Host ""
    Write-Host "  $msg" -ForegroundColor Cyan -NoNewline
    Write-Host ""
}
function Write-Ok   { param($msg) Write-Host "  " -NoNewline; Write-Host "v" -ForegroundColor Green -NoNewline; Write-Host "  $msg" }
function Write-Info { param($msg) Write-Host "  $msg" -ForegroundColor DarkGray }
function Write-Warn { param($msg) Write-Host "  ! $msg" -ForegroundColor Yellow }
function Write-Step { param($msg) Write-Host ""; Write-Host $msg -ForegroundColor White }
function Write-Fail { param($msg) Write-Host "error: $msg" -ForegroundColor Red; exit 1 }

# ---- config ------------------------------------------------------------------

$Repo       = "unhosted-ai/unhosted-core"
$InstallDir = if ($env:UNHOSTED_INSTALL_DIR) { $env:UNHOSTED_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "unhosted" }
$Version    = if ($env:UNHOSTED_VERSION) { $env:UNHOSTED_VERSION } else { "latest" }
$NoDesktop  = ($env:UNHOSTED_NO_DESKTOP -eq "1")

# ---- detect arch -------------------------------------------------------------

$arch = (Get-CimInstance -ClassName Win32_Processor).Architecture
switch ($arch) {
    9       { $Target = "x86_64-pc-windows-msvc" }
    12      { Write-Fail "ARM64 Windows builds aren't published yet. Build from source." }
    default { Write-Fail "unsupported architecture code '$arch'" }
}

Write-Host ""
Write-Host "  unhosted" -ForegroundColor Cyan -NoNewline
Write-Host "  —  local AI mesh"
Write-Info "platform  windows / $Target"
Write-Info "install   $InstallDir\unhosted.exe"

# ---- find release ------------------------------------------------------------

Write-Step "Downloading"

if ($Version -eq "latest") {
    $api = "https://api.github.com/repos/$Repo/releases/latest"
} else {
    $api = "https://api.github.com/repos/$Repo/releases/tags/$Version"
}

$rel = Invoke-RestMethod -Uri $api -UserAgent "unhosted-installer"
$asset = $rel.assets | Where-Object { $_.name -eq "unhosted-$Target.zip" } | Select-Object -First 1
if (-not $asset) { Write-Fail "no release asset found for $Target in $Version." }

Write-Info $asset.browser_download_url

$tmp = Join-Path $env:TEMP ("unhosted-install-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

$zip = Join-Path $tmp "unhosted.zip"
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip -UseBasicParsing
Expand-Archive -Path $zip -DestinationPath $tmp -Force

$cliExe = Get-ChildItem -Path $tmp -Recurse -Filter "unhosted.exe" `
    | Where-Object { $_.Name -eq "unhosted.exe" } | Select-Object -First 1
$desktopExe = Get-ChildItem -Path $tmp -Recurse -Filter "unhosted-desktop.exe" | Select-Object -First 1

if (-not $cliExe) { Write-Fail "archive did not contain unhosted.exe" }

# ---- install binaries --------------------------------------------------------

Write-Step "Installing"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Path $cliExe.FullName -Destination (Join-Path $InstallDir "unhosted.exe") -Force
Write-Ok "$InstallDir\unhosted.exe"

$desktopPath = $null
if ($desktopExe -and (-not $NoDesktop)) {
    $desktopPath = Join-Path $InstallDir "unhosted-desktop.exe"
    Copy-Item -Path $desktopExe.FullName -Destination $desktopPath -Force
    Write-Ok $desktopPath
}

# PATH registration
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Warn "added $InstallDir to your user PATH — restart your shell to pick it up"
}

# ---- Start Menu shortcut for the desktop shell ------------------------------

if ($desktopPath -and (Test-Path $desktopPath)) {
    $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $lnk = Join-Path $startMenu "Unhosted.lnk"
    try {
        $shell = New-Object -ComObject WScript.Shell
        $shortcut = $shell.CreateShortcut($lnk)
        $shortcut.TargetPath      = $desktopPath
        $shortcut.WorkingDirectory = $InstallDir
        $shortcut.Description     = "Unhosted — your local AI mesh"
        $shortcut.IconLocation    = "$desktopPath,0"
        $shortcut.Save()
        Write-Ok $lnk
    } catch {
        Write-Warn "could not create Start Menu shortcut: $_"
    }
}

Remove-Item -Path $tmp -Recurse -Force

# ---- verify ------------------------------------------------------------------

Write-Step "Installed"
$ver = & (Join-Path $InstallDir "unhosted.exe") --version
Write-Host "  $ver" -ForegroundColor Green

# ---- next steps --------------------------------------------------------------

Write-Step "Next steps"
Write-Info "1. install llama.cpp   https://github.com/ggerganov/llama.cpp/releases"
Write-Info "2. pull a model        unhosted pull llama3.2:1b"
Write-Info "3. run the daemon      unhosted serve"
if ($desktopPath) {
    Write-Info "4. open the app        unhosted-desktop   (or click Unhosted in the Start Menu)"
} else {
    Write-Info "4. open the app        start http://127.0.0.1:7777"
}
Write-Host ""
