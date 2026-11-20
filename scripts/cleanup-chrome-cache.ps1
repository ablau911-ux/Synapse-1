param([switch]$Apply)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'cleanup-cache-common.ps1')
$profile = Join-Path $env:LOCALAPPDATA 'Google\Chrome\User Data\Default'
$targets = @('Cache','Code Cache','GPUCache','Service Worker\CacheStorage','DawnCache','GrShaderCache','ShaderCache') |
    ForEach-Object { Join-Path $profile $_ }
Invoke-SynapseCacheCleanup -Targets $targets -Mode Contents -Apply:$Apply -BusyProcesses @('chrome') |
    ConvertTo-Json -Depth 5
