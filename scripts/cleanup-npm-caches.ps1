param([switch]$Apply)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'cleanup-cache-common.ps1')
$targets = @((Join-Path $env:LOCALAPPDATA 'npm-cache'), (Join-Path $env:USERPROFILE 'npm-cache'))
Invoke-SynapseCacheCleanup -Targets $targets -Mode Contents -BusyProcesses @('node','npm','npx') -Apply:$Apply |
    ConvertTo-Json -Depth 5
