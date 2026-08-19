# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 SerenIT ApS
# Copyright 2026 Prompted EV

<#
.SYNOPSIS
  Build a Chaperone endpoint MCPB bundle for a given client OS (generic, reusable).

.DESCRIPTION
  Compiles the chapr-endpoint release binary, assembles a bundle directory from a
  manifest, and (if the mcpb CLI is available) validates + packs it into a .mcpb.
  This is the reusable builder — customer/OS-specific bundles
  instantiate the manifest template and call this, or copy its steps.

  Runs on Windows PowerShell 5.1 and on pwsh 7 (Linux/macOS). It does NOT
  cross-compile: it packages the binary cargo just built for THIS host, and
  refuses to label that binary as some other platform. The release workflow
  therefore builds each OS's bundle on that OS's runner.

  MCPB spec: https://github.com/modelcontextprotocol/mcpb

.PARAMETER Manifest
  Path to an already-instantiated manifest.json. Mutually exclusive with -Template.

.PARAMETER Template
  Path to manifest.template.json. The {{PLACEHOLDERS}} are filled from -Version,
  -Author, -BundleName, -DisplayName and the target platform. Mutually exclusive
  with -Manifest.

.PARAMETER Platform
  MCPB platform id the bundle declares: win32 | linux | darwin. Defaults to the
  host, and must match it.

.PARAMETER OutDir
  Where to assemble the bundle (default: ./build).

.PARAMETER ChaperoneRoot
  Path to the Chaperone workspace root (default: two levels up from this script).

.PARAMETER Pack
  If set, run `mcpb validate` + `mcpb pack` (requires Node + @anthropic-ai/mcpb).

.EXAMPLE
  ./build-mcpb.ps1 -Manifest ./manifest.json -Pack

.EXAMPLE
  ./build-mcpb.ps1 -Template ./manifest.template.json -Version 0.1.0 -Pack `
                   -Output ./chaperone-endpoint.mcpb
#>
[CmdletBinding()]
param(
  [string]$Manifest,
  [string]$Template,
  [string]$Version,
  [string]$Author      = "SerenIT ApS / Prompted EV",
  [string]$BundleName  = "chaperone-endpoint",
  [string]$DisplayName = "Chaperone",
  [ValidateSet("win32", "linux", "darwin")][string]$Platform,
  [string]$OutDir = "./build",
  [string]$ChaperoneRoot,
  [string]$Output,   # optional .mcpb output path (e.g. the committed deliverable location)
  [switch]$Pack
)
$ErrorActionPreference = "Stop"

# Resolved here, not as a param default: $PSScriptRoot is EMPTY inside param()
# when the script is invoked as `powershell -File ...`, which is how CI runs it.
# As a default it therefore blew up before the first line of real work.
if (-not $ChaperoneRoot) {
  $scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
  $ChaperoneRoot = (Resolve-Path (Join-Path $scriptDir "../..")).Path
}

# Exactly one manifest source. Quietly preferring one over the other is how a
# release ends up shipping last month's hand-edited manifest.
if ($Manifest -and $Template) { throw "-Manifest and -Template are mutually exclusive." }
if (-not $Manifest -and -not $Template) {
  throw "Pass either -Manifest <instantiated.json> or -Template <manifest.template.json>."
}

# $IsWindows does not exist in Windows PowerShell 5.1, where it evaluates to
# $null — hence the PSEdition test first.
$onWindows = ($PSVersionTable.PSEdition -eq "Desktop") -or $IsWindows
$hostPlatform = if ($onWindows) { "win32" } elseif ($IsMacOS) { "darwin" } else { "linux" }
if (-not $Platform) { $Platform = $hostPlatform }
if ($Platform -ne $hostPlatform) {
  throw ("Cannot build a '$Platform' bundle on '$hostPlatform'. This script packages the binary " +
         "cargo built for THIS host; a bundle claiming one platform while carrying another's " +
         "binary installs cleanly and then fails to start. Build it on the target OS.")
}
$exeSuffix = if ($Platform -eq "win32") { ".exe" } else { "" }
$binName   = "chapr-endpoint$exeSuffix"

$cargoCandidate = Join-Path $HOME ".cargo/bin/cargo$exeSuffix"
$cargo = if (Test-Path $cargoCandidate) { $cargoCandidate } else { "cargo" }

Write-Host "==> Building $BundleName for $Platform in $ChaperoneRoot"
& $cargo build --release -p chapr-endpoint --manifest-path (Join-Path $ChaperoneRoot "Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Ask cargo where it actually put the binary instead of assuming ./target.
# The README tells you to set CARGO_TARGET_DIR when building alongside another
# checkout — and with it set, a hardcoded ./target/release either does not exist
# or, worse, still holds an OLD binary from a build before you set it. That path
# ships a stale endpoint to a customer and nothing catches it.
$targetDir = $null
try {
  $meta = & $cargo metadata --format-version 1 --no-deps `
    --manifest-path (Join-Path $ChaperoneRoot "Cargo.toml") | ConvertFrom-Json
  $targetDir = $meta.target_directory
} catch {
  Write-Warning "cargo metadata failed ($_); falling back to CARGO_TARGET_DIR / ./target"
}
if (-not $targetDir) {
  $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $ChaperoneRoot "target" }
}
Write-Host "==> Target directory: $targetDir"

# Freshness needs no check of its own: cargo build ran immediately above, and
# cargo rebuilds whenever the binary is older than its sources. The only real
# staleness risk was reading a DIFFERENT directory than cargo wrote to, which the
# metadata lookup above removes. (Verified empirically: back-dating the exe just
# makes cargo relink it.)
$exe = Join-Path $targetDir "release/$binName"
if (-not (Test-Path $exe)) { throw "release binary not found at $exe" }

$server = Join-Path $OutDir "server"
New-Item -ItemType Directory -Force -Path $server | Out-Null
$stagedBin = Join-Path $server $binName
Copy-Item $exe $stagedBin -Force
if (-not $onWindows) {
  # Copy-Item does not carry the mode across, and a bundle whose server binary is
  # not executable installs happily and then cannot be launched.
  & chmod +x $stagedBin
  if ($LASTEXITCODE -ne 0) { throw "chmod +x failed on $stagedBin" }
}

$OutDirFull  = (Resolve-Path $OutDir).Path
$manifestOut = Join-Path $OutDirFull "manifest.json"
if ($Template) {
  if (-not $Version) { throw "-Template requires -Version (it fills {{VERSION}})." }
  $text = (Get-Content -Raw -Path $Template).
    Replace("{{BUNDLE_NAME}}",  $BundleName).
    Replace("{{DISPLAY_NAME}}", $DisplayName).
    Replace("{{VERSION}}",      $Version).
    Replace("{{AUTHOR}}",       $Author).
    Replace("{{PLATFORM}}",     $Platform).
    Replace("{{EXE_SUFFIX}}",   $exeSuffix)
  # A leftover placeholder is a silent mis-build: `mcpb validate` is perfectly
  # happy with the literal string "{{VERSION}}" as a version.
  if ($text -match '\{\{[A-Z_]+\}\}') { throw "unfilled placeholder in manifest: $($Matches[0])" }
  # Not Set-Content -Encoding utf8: on PowerShell 5.1 that writes a BOM, and
  # node's JSON.parse rejects a BOM — so `mcpb validate` fails on a manifest that
  # looks correct in every editor.
  [System.IO.File]::WriteAllText($manifestOut, $text, (New-Object System.Text.UTF8Encoding($false)))
} else {
  Copy-Item $Manifest $manifestOut -Force
}

# Apache-2.0 sections 4(a) and 4(d): a redistributed bundle carries the licence
# text and the NOTICE file. The .mcpb IS a redistribution - it lands on a laptop
# that never sees this repo.
Copy-Item (Join-Path $ChaperoneRoot "LICENSE") (Join-Path $OutDirFull "LICENSE") -Force
Copy-Item (Join-Path $ChaperoneRoot "NOTICE")  (Join-Path $OutDirFull "NOTICE")  -Force
Write-Host "==> Assembled bundle at $OutDirFull"

if ($Pack) {
  Write-Host "==> Validating + packing with @anthropic-ai/mcpb"
  # Check the exit code: validate's failure used to be ignored, so an invalid
  # manifest was packed and shipped anyway, failing at install time on the
  # user's laptop instead of here.
  npx --yes @anthropic-ai/mcpb validate $manifestOut
  if ($LASTEXITCODE -ne 0) { throw "mcpb validate failed - not packing" }
  if ($Output) {
    npx --yes @anthropic-ai/mcpb pack $OutDirFull $Output
    if ($LASTEXITCODE -ne 0) { throw "mcpb pack failed" }
    Write-Host "==> Packed -> $Output"
  } else {
    npx --yes @anthropic-ai/mcpb pack $OutDirFull
    if ($LASTEXITCODE -ne 0) { throw "mcpb pack failed" }
  }
  Write-Host "==> Install the .mcpb via Claude Desktop -> Settings -> Extensions."
} else {
  Write-Host "==> Next: mcpb pack `"$OutDirFull`" <output.mcpb>   (install: npm i -g @anthropic-ai/mcpb)"
}
