<#
  Session Signals — install the self-healing VM bridge (host side, Windows).

  For NAT WSL and full Hyper-V/VirtualBox VMs (NOT needed with WSL mirrored
  networking). One-time, elevated. It:
    1. opens a private-scoped firewall hole on the bridge port,
    2. registers a scheduled task that re-points the portproxy to the current
       vEthernet IP(s) at every boot and logon (see bridge-refresh.ps1),
    3. runs that re-point once now,
    4. prints the ready-to-run guest command with your real token.

  The listener stays loopback-only (the portproxy forwards to 127.0.0.1); the
  X-Beacon-Token remains the integrity control on the VM subnet.

  Run in an ELEVATED PowerShell:  .\bridge-install.ps1
#>
[CmdletBinding()]
param(
  [int]$ListenPort = 4318,
  [int]$TargetPort = 4317,
  # Private ranges allowed to reach the bridge. 172.16.0.0/12 covers both the
  # WSL and Hyper-V Default Switch subnets; the token still gates state.
  [string[]]$AllowFrom = @('172.16.0.0/12'),
  # Cooldown (seconds) the refresh enforces between successful re-points, so a
  # burst of network-connect events doesn't re-run it repeatedly.
  [int]$CooldownSeconds = 20,
  # Settle timer (ISO-8601 duration) on the network-connect event trigger — waits
  # for the adapter's IP/gateway to come up before re-pointing.
  [string]$EventSettleDelay = 'PT8S'
)
$ErrorActionPreference = 'Stop'

$isAdmin = ([Security.Principal.WindowsPrincipal] `
  [Security.Principal.WindowsIdentity]::GetCurrent()
).IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)
if (-not $isAdmin) { throw "Run this in an elevated (Administrator) PowerShell." }

$here    = Split-Path -Parent $MyInvocation.MyCommand.Path
$refresh = Join-Path $here 'bridge-refresh.ps1'
if (-not (Test-Path $refresh)) { throw "bridge-refresh.ps1 not found next to this script." }

# --- 1. firewall: private-scoped inbound on the bridge port -------------------
$ruleName = "SessionSignals VM bridge $ListenPort"
Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow `
  -Protocol TCP -LocalPort $ListenPort -RemoteAddress $AllowFrom | Out-Null
Write-Host "firewall: allow inbound TCP $ListenPort from $($AllowFrom -join ', ')" -ForegroundColor Green

# --- 2. scheduled task: re-point at boot + logon + on network connect ---------
$taskName = 'SessionSignals-BridgeRefresh'
Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
$action = New-ScheduledTaskAction -Execute 'powershell.exe' `
  -Argument "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$refresh`" -ListenPort $ListenPort -TargetPort $TargetPort -CooldownSeconds $CooldownSeconds"

# Settle timers before each run (the "timer before the first run"): a fixed Delay
# on each trigger so the adapter's IP/gateway is up before we re-point. Longer at
# cold boot (networking + the Hyper-V vSwitch take time to initialize).
$tStartup = New-ScheduledTaskTrigger -AtStartup;                     $tStartup.Delay = 'PT20S'
$tLogon   = New-ScheduledTaskTrigger -AtLogOn;                       $tLogon.Delay   = 'PT10S'

# Network-connect/disconnect event trigger. Task Scheduler has no event-trigger
# cmdlet, so build it via CIM. Watches Microsoft-Windows-NetworkProfile 10000
# (Connected) / 10001 (Disconnected) — the WSL vSwitch coming back after
# `wsl --shutdown` (often with a new gateway IP) raises 10000. Delay = settle.
$evtClass = Get-CimClass -Namespace Root/Microsoft/Windows/TaskScheduler -ClassName MSFT_TaskEventTrigger
$tEvent   = New-CimInstance -CimClass $evtClass -ClientOnly
$tEvent.Enabled = $true
$tEvent.Delay   = $EventSettleDelay
$tEvent.Subscription = @"
<QueryList><Query Id="0" Path="Microsoft-Windows-NetworkProfile/Operational"><Select Path="Microsoft-Windows-NetworkProfile/Operational">*[System[Provider[@Name='Microsoft-Windows-NetworkProfile'] and (EventID=10000 or EventID=10001)]]</Select></Query></QueryList>
"@

$triggers  = @($tStartup, $tLogon, $tEvent)
$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
# IgnoreNew = no overlapping runs (the other half of the cooldown: a burst of
# connect events can't stack up while one re-point is in flight). The per-run
# minimum interval is enforced by -CooldownSeconds inside bridge-refresh.ps1.
$settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
  -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $triggers `
  -Principal $principal -Settings $settings -Description 'Re-point the Session Signals VM bridge to the current vEthernet IP.' | Out-Null
Write-Host "scheduled task '$taskName' registered (AtStartup+20s, AtLogOn+10s, NetworkProfile connect+$EventSettleDelay; cooldown ${CooldownSeconds}s, no-overlap)." -ForegroundColor Green

# --- 3. re-point once now (bypass cooldown for this deliberate run) ------------
& $refresh -ListenPort $ListenPort -TargetPort $TargetPort -CooldownSeconds $CooldownSeconds -Force

# --- 4. print the guest command with the real token ---------------------------
$beacon = Join-Path $env:APPDATA 'com.beacon.cc\beacon.json'
$token  = (Get-Content $beacon -Raw | ConvertFrom-Json).auth_token
Write-Host ""
Write-Host "Bridge ready. In the VM/WSL:" -ForegroundColor Cyan
Write-Host "  1) run the forwarder (needs socat):  GATEWAY=<host-ip> PORT_BRIDGE=$ListenPort ./forwarder.sh" -ForegroundColor Yellow
Write-Host "     (or install session-signals-forwarder.service — see guest/README section)" -ForegroundColor DarkGray
Write-Host "  2) install real hooks:               TOKEN=$token PORT=$TargetPort ./install-hooks.sh" -ForegroundColor Yellow
Write-Host ""
Write-Host "If mid-session 'wsl --shutdown' changes the IP, re-run bridge-refresh.ps1 (boot/logon do it automatically)."
