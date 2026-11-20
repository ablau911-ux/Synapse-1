param(
    [switch]$Apply,
    [ValidateNotNullOrEmpty()][string]$LocalDataRoot = (Join-Path $env:LOCALAPPDATA 'synapse')
)
$ErrorActionPreference = 'Stop'
$LocalDataRoot = [IO.Path]::GetFullPath($LocalDataRoot).TrimEnd('\', '/')
$targets = @(
    (Join-Path $LocalDataRoot 'build-target\debug'),
    (Join-Path $LocalDataRoot 'codex-target-syn31-20260913\debug')
)
$allowed = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach ($t in $targets) { [void]$allowed.Add($t) }
function TreeStats([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{ Path = $Path; Exists = $false; GiB = 0; Files = 0 }
    }
    $stats = Get-ChildItem -LiteralPath $Path -Recurse -Force -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum
    [pscustomobject]@{ Path = $Path; Exists = $true; GiB = [math]::Round($stats.Sum / 1GB, 3); Files = $stats.Count }
}
function Assert-NoReparsePoints([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    while ($null -ne $item) {
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Refusing reparse path: $($item.FullName)"
        }
        $item = $item.Parent
    }
    $linked = Get-ChildItem -LiteralPath $Path -Recurse -Force -ErrorAction Stop |
        Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 } |
        Select-Object -First 1
    if ($linked) { throw "Refusing reparse descendant: $($linked.FullName)" }
}
$validated = @(
    foreach ($target in $targets) {
        if (-not (Test-Path -LiteralPath $target)) { continue }
        $resolved = (Resolve-Path -LiteralPath $target).Path
        if (-not $allowed.Contains($resolved)) { throw "Refusing unexpected path: $resolved" }
        if (-not $resolved.StartsWith($LocalDataRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing outside Synapse local data: $resolved" }
        Assert-NoReparsePoints $resolved
        $resolved
    }
)
$before = @($targets | ForEach-Object { TreeStats $_ })
$deleted = @()
if ($Apply) {
    $busy = @(Get-Process -Name @('cargo', 'rustc', 'rust-analyzer') -ErrorAction SilentlyContinue)
    if ($busy.Count -gt 0) { throw "Refusing cleanup while process is running: $(($busy.ProcessName | Sort-Object -Unique) -join ',')" }
    foreach ($resolved in $validated) {
        Remove-Item -LiteralPath $resolved -Recurse -Force -ErrorAction Stop
        $deleted += $resolved
    }
}
$after = @($targets | ForEach-Object { TreeStats $_ })
$drive = Get-PSDrive -Name C
[pscustomobject]@{
    Apply = [bool]$Apply
    Before = $before
    Deleted = $deleted
    After = $after
    CFreeGiB = [math]::Round($drive.Free / 1GB, 3)
    CUsedGiB = [math]::Round($drive.Used / 1GB, 3)
} | ConvertTo-Json -Depth 5
