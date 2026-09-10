# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 SerenIT ApS
# Copyright 2026 Prompted EV

<#
.SYNOPSIS
  Build the Windows coordinator MSI (D-049).

.DESCRIPTION
  Wraps `wix build`. It does not compile Rust unless -Build is given, and it does
  not invent a version: -Version is mandatory, because versioning is a human
  decision in this project and an installer that picks its own would make the
  Add/Remove Programs entry and the upgrade path lie about what is installed.

  Prerequisites (once per machine):
      dotnet tool install --global wix --version 6.0.2
      wix extension add --global WixToolset.Firewall.wixext/6.0.2
      wix extension add --global WixToolset.UI.wixext/6.0.2

  license.rtf beside this script is the installer's licence page. Regenerate it
  from the repo LICENSE rather than editing it by hand; it is a mechanical
  RTF-escaped copy, not a separate document.

.PARAMETER Version
  Product version, e.g. 0.1.4. Must match the workspace version; the script
  refuses if it does not, so a stale binary cannot be shipped under a new label.

.PARAMETER CoordExe
  The chapr-coord.exe to package (default: <root>/build/chapr-coord.exe).

.PARAMETER Build
  Run `cargo build --release -p chapr-coord` first and package that binary.

.PARAMETER OutDir
  Where the .msi lands (default: <root>/build).

.EXAMPLE
  ./build-msi.ps1 -Version 0.1.4 -Build
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$CoordExe,
  [switch]$Build,
  [string]$OutDir,
  [string]$ChaperoneRoot
)

$ErrorActionPreference = "Stop"

if (-not $ChaperoneRoot) { $ChaperoneRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path }
if (-not $OutDir)        { $OutDir = Join-Path $ChaperoneRoot "build" }
if (-not $CoordExe)      { $CoordExe = Join-Path $ChaperoneRoot "build\chapr-coord.exe" }

# The version in the package must be the version in the workspace. Two numbers
# that can disagree eventually do, and the one an administrator sees in Add/Remove
# Programs is the one they will quote back when something is wrong.
$cargoToml = Get-Content (Join-Path $ChaperoneRoot "Cargo.toml") -Raw
if ($cargoToml -match '(?m)^version\s*=\s*"([^"]+)"') {
  $workspaceVersion = $Matches[1]
  if ($workspaceVersion -ne $Version) {
    throw "-Version $Version does not match the workspace version $workspaceVersion. Bump one or the other deliberately; this script will not choose."
  }
}

if ($Build) {
  Write-Host "Building chapr-coord (release)..."
  & cargo build --release -p chapr-coord --manifest-path (Join-Path $ChaperoneRoot "Cargo.toml")
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
  $CoordExe = Join-Path $ChaperoneRoot "target\release\chapr-coord.exe"
}

if (-not (Test-Path $CoordExe)) { throw "coordinator binary not found: $CoordExe (pass -CoordExe, or -Build)" }
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
  throw "the wix CLI is not on PATH. Install it with: dotnet tool install --global wix --version 6.0.2"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$out = Join-Path $OutDir "chapr-coord-$Version.msi"
$wxs = Join-Path $PSScriptRoot "chapr-coord.wxs"

Write-Host "Packaging $CoordExe -> $out"
& wix build $wxs -arch x64 -d "Version=$Version" -d "CoordExe=$CoordExe" `
      -ext WixToolset.Firewall.wixext -ext WixToolset.UI.wixext -o $out
if ($LASTEXITCODE -ne 0) { throw "wix build failed" }

Write-Host ""
Write-Host "Built $out"
Write-Host "Install (elevated):"
Write-Host "  msiexec /i `"$out`" COORD_SHARE=\\FS01\share COORD_URL=http://FS01:8787 /qn /l*v install.log"
Write-Host "Then read the values every laptop needs:"
Write-Host "  & 'C:\Program Files\Chaperone\chapr-coord.exe' handover --config C:\ProgramData\Chaperone\coord.toml"
