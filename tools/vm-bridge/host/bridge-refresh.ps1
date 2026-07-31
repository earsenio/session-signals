<#
  Session Signals — re-point the VM bridge to the CURRENT vEthernet IP(s).

  WSL/Hyper-V vEthernet addresses change across host reboots and `wsl --shutdown`.
  This script is idempotent and self-healing: it clears any stale portproxy
  entries on the bridge port and adds a fresh one for every live vEthernet (WSL*
  / Default Switch) address, each forwarding to the app's loopback listener. Run
  by the scheduled task registered by bridge-install.ps1 (at boot / logon / on a
  network-connect event), and safe to run by hand any time the bridge stops
  working (pass -Force to bypass the cooldown).

  Forwarding to 127.0.0.1 keeps the app's loopback-only guarantee intact — the
  listener still sees a loopback peer; the X-Beacon-Token still gates state.

  Cooldown: a network-connect trigger can fire in bursts, so a successful
  re-point stamps a marker and a run within -CooldownSeconds of it no-ops. Only
  *successful* re-points stamp the marker, so a run that finds no IP yet (VM
  still coming up) never suppresses the real re-point that follows.
#>
[CmdletBinding()]
param(
  [int]$ListenPort = 4318,
  [int]$TargetPort = 4317,
  # Minimum seconds between successful re-points; bursts within this window skip.
  [int]$CooldownSeconds = 20,
  # Bypass the cooldown (hand-runs and the one-time install run).
  [switch]$Force
)
$ErrorActionPreference = 'Stop'

$marker = Join-Path $env:ProgramData 'SessionSignals\bridge-refresh.last'

# --- cooldown gate ------------------------------------------------------------
if (-not $Force -and (Test-Path $marker)) {
  $age = ((Get-Date) - (Get-Item $marker).LastWriteTime).TotalSeconds
  if ($age -lt $CooldownSeconds) {
    Write-Host ("cooldown: last re-point {0:N0}s ago (< {1}s) — skipping." -f $age, $CooldownSeconds)
    return
  }
}

# --- find the current VM-facing host address(es) ------------------------------
# The connect event can arrive a beat before the adapter's IP settles, so poll
# briefly rather than give up on the first empty read.
$vmIps = @()
for ($i = 0; $i -lt 6; $i++) {
  $vmIps = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object { $_.InterfaceAlias -like 'vEthernet (WSL*' -or $_.InterfaceAlias -like 'vEthernet (Default Switch*' } |
    Select-Object -ExpandProperty IPAddress -Unique
  if ($vmIps) { break }
  Start-Sleep -Milliseconds 600
}

# --- drop every existing portproxy entry on our listen port (any address) -----
# so a vanished vEthernet IP never lingers.
$show = netsh interface portproxy show v4tov4 2>$null
foreach ($line in $show) {
  if ($line -match '^\s*(\d{1,3}(?:\.\d{1,3}){3})\s+(\d+)\s') {
    $addr = $Matches[1]; $lport = [int]$Matches[2]
    if ($lport -eq $ListenPort) {
      netsh interface portproxy delete v4tov4 listenaddress=$addr listenport=$ListenPort 2>$null | Out-Null
    }
  }
}

if (-not $vmIps) {
  Write-Warning "No vEthernet (WSL / Default Switch) address found. Is the VM/WSL running? Nothing re-pointed (cooldown not started)."
  return
}

foreach ($ip in $vmIps) {
  netsh interface portproxy add v4tov4 listenaddress=$ip listenport=$ListenPort `
                                       connectaddress=127.0.0.1 connectport=$TargetPort | Out-Null
  Write-Host "bridge: ${ip}:${ListenPort} -> 127.0.0.1:${TargetPort}" -ForegroundColor Green
}

# Stamp the cooldown marker only after a real re-point.
New-Item -ItemType Directory -Force -Path (Split-Path $marker) | Out-Null
Set-Content -Path $marker -Value (Get-Date -Format o)

netsh interface portproxy show v4tov4
