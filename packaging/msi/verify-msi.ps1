# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 SerenIT ApS
# Copyright 2026 Prompted EV

<#
.SYNOPSIS
  Install the coordinator MSI on THIS machine and check that what it produced is
  a working coordinator. Must run elevated.

.DESCRIPTION
  The point of this script is that "the MSI built" and "the MSI works" are
  different claims, and only the second one is worth anything. It asserts the
  things that have never executed outside compilation: service registration,
  service start, the data directory setup actually wrote, both tokens, the
  firewall rule, and a live /healthz.

  With -Uninstall it then removes the package and asserts the other half of
  D-048's promise: the service and the binary go, the data stays.

  It refuses to run if a chapr-coord service or data directory already exists,
  because this is a smoke test and not a migration.

.PARAMETER Share
  UNC path of the coordinated share, passed as COORD_SHARE. Optional - the
  install succeeds without it, and a coordinator with no share is still a
  coordinator that answers.

.PARAMETER Msi
  The package to install (default: the newest chapr-coord-*.msi under build/).

.PARAMETER Uninstall
  Also uninstall afterwards and verify the data survived.

.EXAMPLE
  ./verify-msi.ps1 -Share \\CHAPR-FS\share -Uninstall
#>
[CmdletBinding()]
param(
  [string]$Msi,
  [string]$Share,
  [string]$Url,
  [string]$DataDir = "C:\ProgramData\Chaperone",
  [int]$Port = 8787,
  [switch]$Uninstall,
  [string]$ChaperoneRoot
)

$ErrorActionPreference = "Stop"
$script:failures = @()

function Check($name, [scriptblock]$test, $detail = "") {
  $ok = $false
  try { $ok = [bool](& $test) } catch { $detail = "$detail $_" }
  if ($ok) { Write-Host ("  ok    {0}" -f $name) }
  else {
    Write-Host ("  FAIL  {0} {1}" -f $name, $detail) -ForegroundColor Red
    $script:failures += $name
  }
}

if (-not ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
        ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw "not elevated. A per-machine MSI that registers a service cannot be installed from an ordinary shell. Open an administrator terminal and rerun."
}

if (-not $ChaperoneRoot) { $ChaperoneRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path }
if (-not $Msi) {
  $Msi = (Get-ChildItem (Join-Path $ChaperoneRoot "build") -Filter "chapr-coord-*.msi" -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
}
if (-not $Msi -or -not (Test-Path $Msi)) { throw "no MSI found; build one with ./build-msi.ps1 -Version 0.1.4" }
# Absolute, always. msiexec hands the package to a server process that does not
# share this shell's working directory, so a relative path becomes 1619 - "the
# package could not be opened" - which reads like a corrupt or missing file.
$Msi = (Resolve-Path $Msi).Path

# Pass -DataDir to prove the claim F1 makes: one property places a coordinator.
# Everything below - config, database, blobs, both tokens - is asserted under it.
$dataDir = $DataDir
$exePath = "C:\Program Files\Chaperone\chapr-coord.exe"
$config  = Join-Path $dataDir "coord.toml"

if (Get-Service chapr-coord -ErrorAction SilentlyContinue) {
  throw "a chapr-coord service already exists. This is a smoke test, not an upgrade test; remove it first."
}
if (Test-Path $dataDir) {
  throw "$dataDir already exists. Move it aside; this script asserts what a first install creates."
}

$log = Join-Path $env:TEMP "chapr-msi-install.log"
# COORD_PORT drives the bind address and the firewall rule together, so the health
# check below and the rule the laptops need can never disagree about which port.
$msiArgs = @("/i", "`"$Msi`"", "/qn", "/l*v", "`"$log`"",
             "COORD_DATA_DIR=`"$dataDir`"", "COORD_PORT=$Port")
if ($Share) { $msiArgs += "COORD_SHARE=`"$Share`"" }
if ($Url)   { $msiArgs += "COORD_URL=`"$Url`"" }

Write-Host "Installing $Msi"
Write-Host "  msiexec $($msiArgs -join ' ')"
$p = Start-Process msiexec.exe -ArgumentList $msiArgs -Wait -PassThru
Write-Host "  msiexec exit code $($p.ExitCode); log at $log"
if ($p.ExitCode -ne 0) {
  # msiexec reports a number and nothing else, and the numbers are not guessable.
  # The four below are the ones this package can realistically produce.
  $meaning = switch ($p.ExitCode) {
    1619 { "the package could not be opened - usually a path msiexec cannot resolve, or one it cannot read as SYSTEM" }
    1603 { "a fatal error during install - the setup custom action most likely failed; see RunCoordSetup in the log" }
    1923 { "the service could not be installed - a chapr-coord service is already registered. Remove it (sc.exe delete chapr-coord) and rerun" }
    1638 { "another version of this product is already installed - uninstall it first" }
    default { "see the log" }
  }
  Write-Host "Install failed ($($p.ExitCode)): $meaning" -ForegroundColor Red
  if (Test-Path $log) {
    Write-Host "The interesting lines:" -ForegroundColor Red
    Select-String -Path $log -Pattern "RunCoordSetup|Error|error|return value 3" |
        Select-Object -Last 25 | ForEach-Object { $_.Line }
  } else {
    Write-Host "No log was written, which itself says msiexec never opened the package." -ForegroundColor Red
  }
  throw "install failed with $($p.ExitCode): $meaning"
}

Write-Host ""
Write-Host "What the install produced:"
Check "binary in Program Files"        { Test-Path $exePath }
Check "config written by setup"        { Test-Path $config }
Check "database created"               { Test-Path (Join-Path $dataDir "coord.db") }
Check "blob root created"              { Test-Path (Join-Path $dataDir "blobs") }
Check "admin token created"            { Test-Path (Join-Path $dataDir "admin-token") }
# A silent install has no console, so this file is the only place the URL and
# both tokens survive. Without it the install works and tells nobody anything.
Check "handover.txt written"           { (Get-Item (Join-Path $dataDir "handover.txt") -ErrorAction SilentlyContinue).Length -gt 0 }
Check "endpoint token created"         { Test-Path (Join-Path $dataDir "endpoint-token") }
# I-017 in one assertion: the database URL must carry its `sqlite:` prefix, or
# the strict reader finds no data directory and neither token above is written.
Check "db_url is a sqlite URL"         { (Get-Content $config -Raw) -match 'db_url\s*=\s*["'']sqlite:' }
Check "data_dir written explicitly"    { (Get-Content $config -Raw) -match 'data_dir\s*=' }
Check "service registered"             { [bool](Get-Service chapr-coord -ErrorAction SilentlyContinue) }
Check "service auto-start"             { (Get-CimInstance Win32_Service -Filter "Name='chapr-coord'").StartMode -eq "Auto" }
Check "service binPath names the config" {
  (Get-CimInstance Win32_Service -Filter "Name='chapr-coord'").PathName -like "*run-service*coord.toml*"
}
Check "service running"                { (Get-Service chapr-coord).Status -eq "Running" }
Check "firewall rule present"          { [bool](Get-NetFirewallRule -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq "Chaperone coordination service" -or $_.Name -eq "Chaperone coordination service" }) }

# The binding, not just the port: a coordinator listening on 127.0.0.1 passes a
# local health check and is still unreachable from every laptop, which is the
# failure this package's COORD_ADDR default exists to prevent.
Check "bound to all interfaces" { (Get-Content $config -Raw) -match "addr\s*=\s*['`"]0\.0\.0\.0:" }

# The service reports RUNNING only once its listener is up, so this should answer
# on the first try. The retries are for the socket, not for the service: a port
# still in TIME_WAIT from an earlier run refuses briefly and says nothing useful.
$port = $Port
Check "/healthz answers" {
  foreach ($try in 1..5) {
    try {
      if ((Invoke-WebRequest "http://127.0.0.1:$port/healthz" -UseBasicParsing -TimeoutSec 5).StatusCode -eq 200) { return $true }
    } catch { Start-Sleep -Seconds 1 }
  }
  $false
}

Write-Host ""
Write-Host "Config as written:"
Get-Content $config | ForEach-Object { "    $_" }

Write-Host ""
Write-Host "Handover (this is what an administrator must be able to get back):"
& $exePath handover --config $config 2>&1 | ForEach-Object { "    $_" }

if ($Uninstall) {
  Write-Host ""
  Write-Host "Uninstalling."
  # Existence and non-emptiness, not an exact byte count: stopping the service
  # closes SQLite, which checkpoints the WAL back into the database file and
  # legitimately changes its size.
  $dbBefore = (Get-Item (Join-Path $dataDir "coord.db")).Length
  $p = Start-Process msiexec.exe -ArgumentList @("/x", "`"$Msi`"", "/qn") -Wait -PassThru
  Write-Host "  msiexec exit code $($p.ExitCode)"
  Check "service removed"        { -not (Get-Service chapr-coord -ErrorAction SilentlyContinue) }
  Check "binary removed"         { -not (Test-Path $exePath) }
  Check "firewall rule removed"  { -not (Get-NetFirewallRule -ErrorAction SilentlyContinue |
      Where-Object { $_.DisplayName -eq "Chaperone coordination service" -or $_.Name -eq "Chaperone coordination service" }) }
  Check "DATA KEPT: database"    { $dbBefore -gt 0 -and (Test-Path (Join-Path $dataDir "coord.db")) -and (Get-Item (Join-Path $dataDir "coord.db")).Length -gt 0 }
  Check "DATA KEPT: config"      { Test-Path $config }
  Check "DATA KEPT: tokens"      { (Test-Path (Join-Path $dataDir "admin-token")) -and (Test-Path (Join-Path $dataDir "endpoint-token")) }
}

Write-Host ""
if ($script:failures.Count -eq 0) {
  Write-Host "All checks passed." -ForegroundColor Green
  exit 0
} else {
  Write-Host ("{0} check(s) failed: {1}" -f $script:failures.Count, ($script:failures -join ", ")) -ForegroundColor Red
  Write-Host "Install log: $log"
  exit 1
}
