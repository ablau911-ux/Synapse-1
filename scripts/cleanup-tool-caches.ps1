param([switch]$Apply)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'cleanup-cache-common.ps1')
$targets = @(
    (Join-Path $env:USERPROFILE '.cache\bazel-repo-contents-cache'),
    (Join-Path $env:USERPROFILE '.cache\bazel-repo-cache'),
    (Join-Path $env:USERPROFILE '.cache\bazel-disk-cache'),
    (Join-Path $env:USERPROFILE '.cache\chrome-devtools-mcp'),
    (Join-Path $env:USERPROFILE '_bazel_Adam')
)
Invoke-SynapseCacheCleanup -Targets $targets -Mode Tree -Apply:$Apply -BusyProcesses @('bazel','bazelisk') |
    ConvertTo-Json -Depth 5
