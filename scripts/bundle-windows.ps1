# Build the Windows release zip for unhosted.
#
# What it produces:
#   dist\unhosted-<target>.zip   containing:
#     unhosted.exe          (CLI)
#     unhosted-desktop.exe  (GUI; icon embedded at compile time via build.rs)
#     README.txt
#
# install.ps1 on a user's machine downloads this, extracts it, drops the
# .exes into %LOCALAPPDATA%\unhosted, and creates a Start Menu shortcut.
#
# Defaults to the host triple. Override with -Target, e.g.:
#   .\scripts\bundle-windows.ps1 -Target aarch64-pc-windows-msvc
#
# Cross-compile note: needs the matching rustup target installed
# (`rustup target add aarch64-pc-windows-msvc`). The MSVC toolchain
# itself must be on PATH (run from a Developer PowerShell, or have
# `link.exe` reachable).
#
# Usage:
#   pwsh -File .\scripts\bundle-windows.ps1
#   pwsh -File .\scripts\bundle-windows.ps1 -Target x86_64-pc-windows-msvc

[CmdletBinding()]
param(
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"

# Run from repo root.
$repoRoot = (& git rev-parse --show-toplevel).Trim()
Set-Location $repoRoot

# Detect host target if none provided. `rustc -vV` exposes the host
# triple in a `host: <triple>` line; cheaper than parsing $env:PROCESSOR_ARCHITECTURE.
if (-not $Target) {
    $hostLine = (& rustc -vV) | Where-Object { $_ -like "host:*" }
    $Target = ($hostLine -split "\s+")[1]
}
if ($Target -notlike "*windows*") {
    Write-Error "bundle-windows.ps1: target '$Target' is not a Windows triple."
    exit 1
}

Write-Host "→ target: $Target"

$dist  = Join-Path $repoRoot "dist"
$stage = Join-Path $dist     "stage-windows-$Target"
$zip   = Join-Path $dist     "unhosted-$Target.zip"

New-Item -ItemType Directory -Force -Path $dist  | Out-Null
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

# ----- build binaries -------------------------------------------------------

Write-Host "→ cargo build --release --target $Target (cli + desktop)"
& cargo build --release --target $Target -p unhosted-cli -p unhosted-desktop
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$binDir = Join-Path $repoRoot "target\$Target\release"
$cliBin = Join-Path $binDir "unhosted.exe"
$guiBin = Join-Path $binDir "unhosted-desktop.exe"

if (-not (Test-Path $cliBin)) { throw "missing $cliBin" }
if (-not (Test-Path $guiBin)) { throw "missing $guiBin" }

Copy-Item $cliBin (Join-Path $stage "unhosted.exe")         -Force
Copy-Item $guiBin (Join-Path $stage "unhosted-desktop.exe") -Force

# ----- README ---------------------------------------------------------------

@"
unhosted — local AI mesh
========================

Binaries:
  unhosted.exe          CLI daemon + helpers      (unhosted --help)
  unhosted-desktop.exe  Native window for the UI  (loads http://127.0.0.1:7777)

Quick start:
  unhosted.exe serve       (in one PowerShell — starts the daemon on :7777)
  unhosted-desktop.exe     (in another — opens the native window)

You'll want a local LLM runtime:
  llama.cpp:   https://github.com/ggerganov/llama.cpp/releases (Windows .zip)
  Ollama:      https://ollama.com/download (auto-detected on :11434)
  LM Studio:   https://lmstudio.ai/download

Full docs: https://github.com/unhosted-ai/unhosted-core
"@ | Out-File -FilePath (Join-Path $stage "README.txt") -Encoding utf8

# ----- zip ------------------------------------------------------------------

Write-Host "→ packing $zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
# Compress-Archive's `-Path $stage\*` flattens into the zip root, which is
# what install.ps1 expects (it Get-ChildItem -Recurse -Filter unhosted.exe).
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal

$zipSize = "{0:N1} MB" -f ((Get-Item $zip).Length / 1MB)
Write-Host ""
Write-Host "Done."
Write-Host "  $zip"
Write-Host "  $zipSize"
Write-Host ""
Write-Host "  smoke-test on this host:"
Write-Host "    Expand-Archive $zip C:\Temp\unhosted-test; C:\Temp\unhosted-test\unhosted.exe --version"
