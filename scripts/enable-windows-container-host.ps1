<#
.SYNOPSIS
Enable Windows container prerequisites without restarting Windows or Docker.
.DESCRIPTION
Run in an elevated Windows PowerShell. Records feature state before and after
servicing. Does not switch Docker engines, stop containers, or reboot the host.
#>
[CmdletBinding()]
param(
    [string]$ReportPath = "$env:LOCALAPPDATA\synapse\windows-container-prerequisites.json"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Windows feature servicing requires an elevated Windows PowerShell.'
}

$features = @('Microsoft-Hyper-V-All', 'Containers')
$report = [ordered]@{
    recorded_at = [DateTimeOffset]::UtcNow.ToString('o')
    computer = $env:COMPUTERNAME
    before = @()
    operations = @()
    after = @()
    restart_needed = $false
    error = $null
}
$reportDirectory = Split-Path -Parent ([IO.Path]::GetFullPath($ReportPath))
New-Item -ItemType Directory -Path $reportDirectory -Force | Out-Null
try {
    foreach ($feature in $features) {
        $state = Get-WindowsOptionalFeature -Online -FeatureName $feature
        $report.before += @{ feature = $feature; state = $state.State.ToString() }
    }
    foreach ($entry in $report.before) {
        if ($entry.state -eq 'EnablePending') {
            $report.restart_needed = $true
        } elseif ($entry.state -ne 'Enabled') {
            $result = Enable-WindowsOptionalFeature -Online -FeatureName $entry.feature -All -NoRestart
            $report.operations += @{ feature = $entry.feature; restart_needed = [bool]$result.RestartNeeded }
            $report.restart_needed = $report.restart_needed -or [bool]$result.RestartNeeded
        }
    }
    foreach ($feature in $features) {
        $state = Get-WindowsOptionalFeature -Online -FeatureName $feature
        $report.after += @{ feature = $feature; state = $state.State.ToString() }
        if ($state.State.ToString() -notin @('Enabled', 'EnablePending')) {
            throw "Feature $feature remains $($state.State)."
        }
    }
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    $report | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $ReportPath -Encoding UTF8
    Write-Output "Feature report: $ReportPath"
}
