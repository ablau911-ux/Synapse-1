param([switch]$Apply)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'cleanup-cache-common.ps1')
$targets = @((Join-Path $env:USERPROFILE '.codex\.tmp'))
Invoke-SynapseCacheCleanup -Targets $targets -Mode OldFiles -OlderThanDays 1 -BusyProcesses @('codex') -Apply:$Apply |
    ConvertTo-Json -Depth 5
