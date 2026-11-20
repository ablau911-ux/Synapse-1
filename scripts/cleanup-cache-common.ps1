Set-StrictMode -Version Latest

function Assert-SynapseCleanupTree([string]$Path) {
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

function Invoke-SynapseCacheCleanup {
    param(
        [Parameter(Mandatory)][string[]]$Targets,
        [Parameter(Mandatory)][ValidateSet('Contents', 'Tree', 'OldFiles')][string]$Mode,
        [switch]$Apply,
        [int]$OlderThanDays = 1,
        [string[]]$BusyProcesses = @()
    )

    $existing = @($Targets | Where-Object { Test-Path -LiteralPath $_ -PathType Container })
    # Preflight every target before deleting from any of them.
    foreach ($target in $existing) {
        $resolved = (Resolve-Path -LiteralPath $target -ErrorAction Stop).Path
        $expected = [IO.Path]::GetFullPath($target).TrimEnd('\')
        if (-not [string]::Equals($resolved.TrimEnd('\'), $expected, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing redirected cleanup target: $target -> $resolved"
        }
        Assert-SynapseCleanupTree $resolved
    }

    $cutoff = (Get-Date).AddDays(-$OlderThanDays)
    $candidates = @()
    foreach ($target in $existing) {
        if ($Mode -eq 'Tree') {
            $candidates += Get-Item -LiteralPath $target -Force -ErrorAction Stop
        } elseif ($Mode -eq 'Contents') {
            $candidates += @(Get-ChildItem -LiteralPath $target -Force -ErrorAction Stop)
        } else {
            $candidates += @(Get-ChildItem -LiteralPath $target -Recurse -Force -File -ErrorAction Stop |
                Where-Object { $_.LastWriteTime -lt $cutoff })
        }
    }

    if ($Apply -and $candidates.Count -gt 0 -and $BusyProcesses.Count -gt 0) {
        $busy = @(Get-Process -Name $BusyProcesses -ErrorAction SilentlyContinue)
        if ($busy.Count -gt 0) { throw "Refusing cleanup while process is running: $(($busy.ProcessName | Sort-Object -Unique) -join ',')" }
    }

    $deleted = @()
    if ($Apply) {
        foreach ($candidate in $candidates) {
            Remove-Item -LiteralPath $candidate.FullName -Recurse -Force -ErrorAction Stop
            $deleted += $candidate.FullName
        }
        if ($Mode -eq 'OldFiles') {
            foreach ($target in $existing) {
                Get-ChildItem -LiteralPath $target -Recurse -Force -Directory -ErrorAction Stop |
                    Sort-Object FullName -Descending | ForEach-Object {
                        if (@(Get-ChildItem -LiteralPath $_.FullName -Force -ErrorAction Stop).Count -eq 0) {
                            Remove-Item -LiteralPath $_.FullName -Force -ErrorAction Stop
                        }
                    }
            }
        }
    }
    [pscustomobject]@{
        Apply = [bool]$Apply
        Mode = $Mode
        Targets = $Targets
        CandidateCount = $candidates.Count
        Deleted = $deleted
    }
}
