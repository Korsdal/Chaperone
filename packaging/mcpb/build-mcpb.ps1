<#
.SYNOPSIS
  Build a Chaperone endpoint MCPB bundle for a given client OS (generic, reusable).

.DESCRIPTION
  Compiles the chapr-endpoint release binary, assembles a bundle directory from a
  manifest, and (if the mcpb CLI is available) validates + packs it into a .mcpb.
  This is the reusable builder — customer/OS-specific bundles
  instantiate the manifest template and call this, or copy its steps.

  MCPB spec: https://github.com/modelcontextprotocol/mcpb

.PARAMETER Manifest
  Path to an instantiated manifest.json (from manifest.template.json).

.PARAMETER OutDir
  Where to assemble the bundle (default: ./build).

.PARAMETER ChaperoneRoot
  Path to the Chaperone workspace root (default: two levels up from this script).

.PARAMETER Pack
  If set, run `mcpb validate` + `mcpb pack` (requires Node + @anthropic-ai/mcpb).

.EXAMPLE
  ./build-mcpb.ps1 -Manifest ./manifest.json -Pack
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Manifest,
  [string]$OutDir = "./build",
  [string]$ChaperoneRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path,
  [string]$Output,   # optional .mcpb output path (e.g. the committed deliverable location)
  [switch]$Pack
)
$ErrorActionPreference = "Stop"

$cargo = if (Test-Path "$HOME/.cargo/bin/cargo.exe") { "$HOME/.cargo/bin/cargo.exe" } else { "cargo" }

Write-Host "==> Compiling chapr-endpoint (release) in $ChaperoneRoot"
& $cargo build --release -p chapr-endpoint --manifest-path (Join-Path $ChaperoneRoot "Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exe = Join-Path $ChaperoneRoot "target/release/chapr-endpoint.exe"
if (-not (Test-Path $exe)) { throw "release binary not found at $exe" }

$server = Join-Path $OutDir "server"
New-Item -ItemType Directory -Force -Path $server | Out-Null
Copy-Item $exe (Join-Path $server "chapr-endpoint.exe") -Force
Copy-Item $Manifest (Join-Path $OutDir "manifest.json") -Force
Write-Host "==> Assembled bundle at $OutDir"

if ($Pack) {
  Write-Host "==> Validating + packing with @anthropic-ai/mcpb"
  npx --yes @anthropic-ai/mcpb validate (Join-Path $OutDir "manifest.json")
  if ($Output) {
    npx --yes @anthropic-ai/mcpb pack $OutDir $Output
    Write-Host "==> Packed -> $Output"
  } else {
    npx --yes @anthropic-ai/mcpb pack $OutDir
  }
  Write-Host "==> Install the .mcpb via Claude Desktop → Settings → Extensions."
} else {
  Write-Host "==> Next: mcpb pack `"$OutDir`" <output.mcpb>   (install: npm i -g @anthropic-ai/mcpb)"
}
