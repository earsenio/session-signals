<#
  Session Signals — remove the VM bridge (host side). Elevated PowerShell.
  Deletes the portproxy entries, the firewall rule, and the refresh task.
#>
[CmdletBinding()]
param([int]$ListenPort = 4318)
$ErrorActionPreference = 'SilentlyContinue'

# Drop every portproxy entry on the bridge port.
$show = netsh interface portproxy show v4tov4 2>$null
foreach ($line in $show) {
  if ($line -match '^\s*(\d{1,3}(?:\.\d{1,3}){3})\s+(\d+)\s' -and [int]$Matches[2] -eq $ListenPort) {
    netsh interface portproxy delete v4tov4 listenaddress=$Matches[1] listenport=$ListenPort | Out-Null
  }
}
Unregister-ScheduledTask -TaskName 'SessionSignals-BridgeRefresh' -Confirm:$false
Get-NetFirewallRule -DisplayName "SessionSignals VM bridge $ListenPort" | Remove-NetFirewallRule
Write-Host "Removed bridge portproxy entries, firewall rule, and refresh task." -ForegroundColor Green
netsh interface portproxy show v4tov4
