<#
.SYNOPSIS
  ┌──────────────────────────────────────────────────────────────────────────┐
  │  REPO MAINTENANCE  -  stop git-worktree and Cargo target/ disk buildup.    │
  │  Backup -> prune merged/stale worktrees -> sweep stale build artifacts.    │
  └──────────────────────────────────────────────────────────────────────────┘

  WHY THIS EXISTS
  ----------------
  Parallel agent / per-issue development creates throwaway git worktrees (e.g.
  `Synapse-issue931-clean`), and EACH worktree gets its own multi-GB Cargo
  `target/`. Git never removes a worktree for you, and Cargo never garbage-
  collects `target/`. Left alone they accumulate without bound - on 2026-06-14
  this host had 46 stale Synapse worktrees consuming ~200 GB and pushing the
  system disk to 94% full, which in turn made every build crawl (disk I/O
  starvation + antivirus rescanning hundreds of thousands of artifact files).

  This script makes that buildup self-correcting. Run on a schedule (see
  scripts/install-maintenance-task.ps1) it keeps disk usage flat forever.

  WHAT IT DOES (per repo, for every git repo found under -Root)
  ------------------------------------------------------------
    1. BACKUP   Lazy, only for unmerged work that is removed: a -Force worktree gets a
                tag maint-backup/<date>/<name>-<sha9> plus a bundle <BackupDir>\
                <repo>-<name>-<sha9>-<date>.bundle first. Nothing removed is unrecoverable.
    2. PRUNE    `git fetch --prune`, then remove worktrees that are SAFE to
                remove: their head is an ancestor of the default branch (local or
                origin/<default>; git ancestry only, no GitHub lookup), or is
                already on a remote branch. NEVER removes: the primary
                checkout, the currently-active worktree, a protected branch
                (main/master/develop), a DIRTY worktree (even with -Force), or
                a worktree with commits not reachable from the default branch and
                not yet on any remote (genuinely unmerged work) - unless -Force.
    3. BRANCHES Delete local branches marked `[gone]` (their remote was pruned),
                except protected / checked-out ones. An UNMERGED gone branch first
                gets a tag maint-backup/<date>/gone-<branch>-<sha9>; no tag, no delete.
    4. SWEEP    Run `cargo-sweep` when available, then report local size caps.
                Whole-directory removal requires explicit -PurgeGeneratedArtifacts
                and -ArtifactRepoPath for one repository; the weekly task does
                not pass these switches. `target\release` is preserved.
    5. REPORT   Print disk free before/after and bytes reclaimed.

  SAFETY MODEL
  ------------
    * DRY-RUN BY DEFAULT. Without -Apply nothing is deleted; it prints exactly
      what it WOULD do. Add -Apply to actually act.
    * Fail-safe: a handled git failure (status, tag, delete, ...) is WARNed and that
      item is kept; a failed -Force backup stops the run before removing anything.
    * Every removal of unmerged work is preceded by its backup (steps 1 and 3).

.PARAMETER Root
  Directory to scan for git repositories (non-recursive: each immediate child
  that is a git repo is processed, plus -Root itself if it is one).
  Default: the parent of this repo (so all sibling checkouts under C:\code).

.PARAMETER Apply
  Actually perform removals/sweeps. Omit for a dry-run preview.

.PARAMETER SweepDays
  Delete build artifacts not accessed in this many days. Default 10.

.PARAMETER MinFreeGB
  If the system drive has less than this many GB free, escalate: sweep with a
  0-day threshold (everything not needed by the very latest build) on non-active
  worktrees. Default 80.

.PARAMETER MaxTargetDebugGB
  Report a repo's `target\debug` when it grows past this size. Default 20.

.PARAMETER MaxGeneratedEvidenceGB
  Report generated evidence/report directories such as `target\fsv`,
  `coverage`, `playwright-report`, `test-results`, and `blob-report` when each
  directory grows past this size. Default 5.

.PARAMETER MaxTargetReleaseGB
  Warn when `target\release` exceeds this size. It is not deleted because the
  installed daemon may be traced back to the shipped release binary. Default 8.

.PARAMETER PurgeGeneratedArtifacts
  Opt in to deleting size-capped generated directories. Requires the exact
  repository path in -ArtifactRepoPath. Never passed by the weekly task.

.PARAMETER ArtifactRepoPath
  Exact repository whose size-capped generated directories may be removed.

.PARAMETER MinEvidenceAgeDays
  Preserve an evidence/report directory if it contains a file modified within
  this many days, even with explicit purge. Default 30.

.PARAMETER Force
  Also remove clean worktrees with unmerged/unpushed commits after bundling
  their committed history. Dirty worktrees are always preserved because Git
  bundles cannot preserve uncommitted or untracked files.

.PARAMETER DisposableIgnoredPatterns
  Directory-name wildcards for git-ignored build output that may be deleted with a
  merged worktree. Any OTHER ignored file or directory (for example `.env`) is
  treated as data: the worktree is kept, because `git status --porcelain` hides
  ignored files and `git worktree remove` would silently delete them.

.PARAMETER BackupDir
  Where bundle backups are written. Default C:\code\_repo_maintenance_backups.

.EXAMPLE
  # Preview (safe):
  powershell -ExecutionPolicy Bypass -File .\scripts\repo-maintenance.ps1
.EXAMPLE
  # Actually clean up everything that is merged/stale across C:\code:
  .\scripts\repo-maintenance.ps1 -Apply
#>
[CmdletBinding()]
param(
    [string]$Root,
    [switch]$Apply,
    [int]$SweepDays = 10,
    [int]$MinFreeGB = 80,
    [int]$MaxTargetDebugGB = 20,
    [int]$MaxGeneratedEvidenceGB = 5,
    [int]$MaxTargetReleaseGB = 8,
    [switch]$PurgeGeneratedArtifacts,
    [string]$ArtifactRepoPath,
    [int]$MinEvidenceAgeDays = 30,
    [switch]$Force,
    [string[]]$DisposableIgnoredPatterns = @('target', 'target-*', 'node_modules', 'storybook-static', 'coverage', '.nyc_output', 'playwright-report', 'test-results', 'blob-report'),
    [string]$BackupDir = 'C:\code\_repo_maintenance_backups'
)

$ErrorActionPreference = 'Stop'
$script:DidApply = $Apply.IsPresent

function Info($m) { Write-Host "[maint] $m" }
function Act ($m) { if ($script:DidApply) { Write-Host "[maint][APPLY] $m" -ForegroundColor Green } else { Write-Host "[maint][dry-run] $m" -ForegroundColor Yellow } }
function Warn($m) { Write-Host "[maint][WARN] $m" -ForegroundColor DarkYellow }
function Die ($m) { throw "[maint] FATAL: $m" }

# #262: Windows PowerShell 5.1 turns redirected native stderr into a terminating error under
# $ErrorActionPreference='Stop' before the caller can read $LASTEXITCODE. Run git with a local
# non-Stop preference, drop stderr, return stdout; $LASTEXITCODE stays git's exit code.
function Invoke-GitQuiet {
    $ErrorActionPreference = 'Continue'
    & git @args 2>&1 | Where-Object { $_ -isnot [System.Management.Automation.ErrorRecord] }
}

# ---- preflight ------------------------------------------------------------
foreach ($t in 'git') {
    if (-not (Get-Command $t -ErrorAction SilentlyContinue)) { Die "'$t' not on PATH." }
}
$HaveSweep = [bool](Get-Command cargo-sweep -ErrorAction SilentlyContinue)
if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot | Split-Path -Parent }  # parent of repo
if (-not (Test-Path $Root)) { Die "Root '$Root' does not exist." }
if ($PurgeGeneratedArtifacts -and [string]::IsNullOrWhiteSpace($ArtifactRepoPath)) {
    Die '-PurgeGeneratedArtifacts requires -ArtifactRepoPath naming one exact repository.'
}
if ($ArtifactRepoPath -and -not $PurgeGeneratedArtifacts) {
    Die '-ArtifactRepoPath requires -PurgeGeneratedArtifacts.'
}
if ($MinEvidenceAgeDays -lt 0) { Die '-MinEvidenceAgeDays must be nonnegative.' }
$artifactRepoResolved = if ($PurgeGeneratedArtifacts) {
    if (-not (Test-Path -LiteralPath $ArtifactRepoPath -PathType Container)) {
        Die "ArtifactRepoPath '$ArtifactRepoPath' is not an existing directory."
    }
    (Resolve-Path -LiteralPath $ArtifactRepoPath).Path.TrimEnd('\')
} else { $null }
if ($script:DidApply -and -not (Test-Path $BackupDir)) { New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null }

$drive = (Get-Item $Root).PSDrive.Name
function Free-GB { [math]::Round((Get-PSDrive $drive).Free / 1GB, 1) }
$freeBefore = Free-GB
Info "Root=$Root  Apply=$($script:DidApply)  SweepDays=$SweepDays  MaxTargetDebugGB=$MaxTargetDebugGB  MaxGeneratedEvidenceGB=$MaxGeneratedEvidenceGB  cargo-sweep=$HaveSweep"
Info "Disk $drive`: free before = $freeBefore GB"
$stampDate = (Get-Date -Format 'yyyy-MM-dd')

function Get-TreeBytes($path) {
    if (-not (Test-Path -LiteralPath $path)) { return [int64]0 }
    $sum = Get-ChildItem -LiteralPath $path -Force -Recurse -File -ErrorAction Stop |
        Measure-Object -Property Length -Sum
    return [int64]$sum.Sum
}

function Test-PathBusy($path) {
    $resolved = Resolve-Path -LiteralPath $path -ErrorAction Stop
    $prefix = $resolved.Path
    $busy = Get-CimInstance Win32_Process -ErrorAction Stop |
        Where-Object {
            $_.Name -match '^(cargo|rustc|rust-analyzer)\.exe$' -or
            ($_.ExecutablePath -and $_.ExecutablePath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase))
        } |
        Select-Object -First 1
    return [bool]$busy
}

function Assert-DisposableChildPath($repo, $path) {
    $repoResolved = (Resolve-Path -LiteralPath $repo).Path
    $pathResolved = (Resolve-Path -LiteralPath $path).Path
    $repoPrefix = $repoResolved.TrimEnd('\') + '\'
    if (-not $pathResolved.StartsWith($repoPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        Die "Refusing cleanup outside repo: $pathResolved"
    }
    $current = $pathResolved
    while ($current) {
        $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Die "Refusing cleanup: reparse point on target path: $current"
        }
        if ([string]::Equals($current, $repoResolved, [StringComparison]::OrdinalIgnoreCase)) { break }
        $current = Split-Path -Parent $current
    }
    $reparse = Get-ChildItem -LiteralPath $pathResolved -Force -Recurse -Attributes ReparsePoint -ErrorAction Stop |
        Select-Object -First 1
    if ($null -ne $reparse) {
        Die "Refusing cleanup: reparse point inside target path: $($reparse.FullName)"
    }
    return $pathResolved
}

function Remove-DisposableDirectory($repo, $path, $reason, [switch]$PreserveRecentEvidence) {
    if (-not (Test-Path -LiteralPath $path)) { return }
    $bytes = Get-TreeBytes $path
    $gb = [math]::Round($bytes / 1GB, 1)
    $repoResolved = (Resolve-Path -LiteralPath $repo).Path.TrimEnd('\')
    $optedIn = $PurgeGeneratedArtifacts -and
        [string]::Equals($repoResolved, $artifactRepoResolved, [StringComparison]::OrdinalIgnoreCase)
    if (-not $optedIn) {
        Warn "size cap exceeded ($reason, ~$gb GB); preserve $path (requires -PurgeGeneratedArtifacts -ArtifactRepoPath '$repo')"
        return
    }
    if ($PreserveRecentEvidence) {
        $cutoff = (Get-Date).AddDays(-$MinEvidenceAgeDays)
        $recent = Get-ChildItem -LiteralPath $path -Force -Recurse -File -ErrorAction Stop |
            Where-Object { $_.LastWriteTime -ge $cutoff } | Select-Object -First 1
        if ($recent) {
            Warn "preserve recent evidence ($($recent.FullName)): $path"
            return
        }
    }
    if ($script:DidApply) {
        $resolved = Assert-DisposableChildPath $repo $path
        if (Test-PathBusy $resolved) {
            Warn "skip busy path: $resolved"
            return
        }
        Act "remove generated directory ($reason, ~$gb GB): $resolved"
        Remove-Item -LiteralPath $resolved -Recurse -Force
    } else {
        Act "remove generated directory ($reason, ~$gb GB): $path"
    }
}

function Invoke-GeneratedArtifactHygiene($repo) {
    $targetRoots = @()
    $rootTarget = Join-Path $repo 'target'
    if (Test-Path -LiteralPath $rootTarget) {
        $targetRoots += (Get-Item -LiteralPath $rootTarget -Force)
    }
    foreach ($target in $targetRoots) {
        $runtimePath = Join-Path $target.FullName 'agent-seats-runtime'
        if (Test-Path -LiteralPath $runtimePath) {
            $runtimeGB = [math]::Round((Get-TreeBytes $runtimePath) / 1GB, 1)
            Warn "runtime state is under Cargo target (~$runtimeGB GB): $runtimePath. Move durable VM disks, databases, and media to %LOCALAPPDATA%\synapse\runtime; this script will not delete runtime state automatically."
        }

        $debugPath = Join-Path $target.FullName 'debug'
        if (Test-Path -LiteralPath $debugPath) {
            $debugGB = (Get-TreeBytes $debugPath) / 1GB
            if ($debugGB -gt $MaxTargetDebugGB -or (Free-GB) -lt $MinFreeGB) {
                Remove-DisposableDirectory $repo $debugPath "target debug cap exceeded"
            } else {
                Info "keep ${debugPath}: $([math]::Round($debugGB,1)) GB <= $MaxTargetDebugGB GB"
            }
        }

        $releasePath = Join-Path $target.FullName 'release'
        if (Test-Path -LiteralPath $releasePath) {
            $releaseGB = (Get-TreeBytes $releasePath) / 1GB
            if ($releaseGB -gt $MaxTargetReleaseGB) {
                Warn "$releasePath is $([math]::Round($releaseGB,1)) GB > $MaxTargetReleaseGB GB; preserving shipped release artifacts."
            }
        }

        $fsvPath = Join-Path $target.FullName 'fsv'
        if (Test-Path -LiteralPath $fsvPath) {
            $fsvGB = (Get-TreeBytes $fsvPath) / 1GB
            if ($fsvGB -gt $MaxGeneratedEvidenceGB -or (Free-GB) -lt $MinFreeGB) {
                Remove-DisposableDirectory $repo $fsvPath "generated FSV/evidence cap exceeded" -PreserveRecentEvidence
            }
        }
    }

    foreach ($relative in 'coverage', '.nyc_output', 'playwright-report', 'test-results', 'blob-report') {
        $path = Join-Path $repo $relative
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $gb = (Get-TreeBytes $path) / 1GB
        if ($gb -gt $MaxGeneratedEvidenceGB -or (Free-GB) -lt $MinFreeGB) {
            Remove-DisposableDirectory $repo $path "generated test report cap exceeded" -PreserveRecentEvidence
        }
    }
}

# ---- discover repos -------------------------------------------------------
# A "repo" here = a directory whose git common-dir is itself (the primary
# checkout), so we don't treat each linked worktree as a separate repo.
$repos = @()
$candidates = @($Root) + (Get-ChildItem -Path $Root -Directory -ErrorAction SilentlyContinue | ForEach-Object FullName)
foreach ($c in $candidates) {
    Push-Location $c -ErrorAction SilentlyContinue
    try {
        # Non-repository scan roots are expected: git fails with no stdout, scanning continues.
        $inside = (Invoke-GitQuiet rev-parse --is-inside-work-tree)
        if ($inside -eq 'true') {
            $top = (Invoke-GitQuiet rev-parse --show-toplevel)
            $commonAbs = (Invoke-GitQuiet rev-parse --path-format=absolute --git-common-dir)
            # primary checkout: git-common-dir is <top>\.git
            if ($top -and $commonAbs -and ((Resolve-Path $commonAbs).Path -eq (Join-Path (Resolve-Path $top).Path '.git'))) {
                if ($repos -notcontains $top) { $repos += $top }
            }
        }
    } finally { Pop-Location -ErrorAction SilentlyContinue }
}
Info "Found $($repos.Count) primary repo checkout(s): $($repos -join ', ')"

function Default-Branch($repo) {
    $h = (Invoke-GitQuiet -C $repo symbolic-ref --quiet refs/remotes/origin/HEAD)
    if ($h) { return ($h -replace '^refs/remotes/origin/','') }
    foreach ($b in 'main','master','develop') {
        $null = Invoke-GitQuiet -C $repo show-ref --verify --quiet "refs/heads/$b"
        if ($LASTEXITCODE -eq 0) { return $b }
    }
    return 'main'
}

# "Is this head merged?" = git ancestry only (#267: no GitHub PR lookup; a squash-merged branch counts as unmerged and is
# backed up). Refs are fully qualified: a bare "origin/main" would resolve to a LOCAL branch of that name first.
function Is-Merged($repo, $sha, $branch, $def) {
    $null = Invoke-GitQuiet -C $repo merge-base --is-ancestor $sha "refs/heads/$def"
    if ($LASTEXITCODE -eq 0) { return $true }
    $null = Invoke-GitQuiet -C $repo merge-base --is-ancestor $sha "refs/remotes/origin/$def"
    return ($LASTEXITCODE -eq 0)
}

# #266: before force-deleting an UNMERGED gone branch keep a tag at its head. Returns $true when deleting is safe
# (merged, or tag already at the same head / newly created); an existing tag is never overwritten. Else Warns, $false.
function Backup-GoneBranch($repo, $name, $def, $stamp) {
    $sha = Invoke-GitQuiet -C $repo rev-parse --verify --quiet "refs/heads/$name"
    if ($LASTEXITCODE -ne 0 -or -not $sha) { Warn "keep gone branch ${name}: head cannot be resolved (git exit $LASTEXITCODE)"; return $false }
    if (Is-Merged $repo $sha $name $def) { return $true }
    $tag = "maint-backup/$stamp/gone-$($name -replace '/','-')-$($sha.Substring(0,9))"
    if (-not $script:DidApply) { Act "would tag $tag"; return $true }
    $null = Invoke-GitQuiet -C $repo tag $tag $sha   # no -f: an existing tag is never overwritten
    $code = $LASTEXITCODE
    if ($code -eq 0 -or (Invoke-GitQuiet -C $repo rev-parse --verify --quiet "refs/tags/$tag^{commit}") -eq $sha) { Act "backup tag: $tag"; return $true }
    Warn "keep gone branch ${name}: backup tag $tag is at another commit or could not be created (git exit $code)"; return $false
}

# #125: `git status --porcelain` omits ignored files, and `git worktree remove` deletes
# them. Return the ignored entries (relative paths) that are NOT disposable build output
# so the caller can keep the worktree. Throws when git cannot inventory (fail closed).
function Get-WorktreeNonDisposableIgnored {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$DisposablePatterns
    )
    $ErrorActionPreference = 'Continue'   # self-contained (tests load this function alone): same as Invoke-GitQuiet, #262
    $lines = @(git -C $Path status --porcelain --ignored=matching 2>&1 | Where-Object { $_ -isnot [System.Management.Automation.ErrorRecord] })
    if ($LASTEXITCODE -ne 0) { throw "git status --ignored failed for $Path" }
    $keep = New-Object System.Collections.Generic.List[string]
    foreach ($line in $lines) {
        if ($line -notmatch '^!! (.+)$') { continue }
        $entry = $Matches[1].Trim().Trim('"')
        $isDirectory = $entry.EndsWith('/')
        $leaf = ($entry.TrimEnd('/') -split '/')[-1]
        $disposable = $isDirectory -and (@($DisposablePatterns | Where-Object { $leaf -like $_ }).Count -gt 0)
        if (-not $disposable) { $keep.Add($entry) }
    }
    return $keep.ToArray()
}

$totalRemoved = 0
foreach ($repo in $repos) {
    Write-Host "`n=== $repo ===" -ForegroundColor Cyan
    $def = Default-Branch $repo
    $active = (Resolve-Path (git -C $repo rev-parse --show-toplevel)).Path
    Info "default branch: $def"
    if ($script:DidApply) {
        $null = Invoke-GitQuiet -C $repo fetch --prune --quiet; if ($LASTEXITCODE -ne 0) { Warn "fetch failed for $repo (offline?). Continuing with local refs." }
    }

    # --- step 1: per-repo context. Backups are created LAZILY (below), only for
    #     worktrees that carry genuinely unmerged work being force-removed. A
    #     merged or already-pushed worktree needs NO backup - its commits are
    #     already on the default branch and/or the remote - so normal runs write
    #     no bundles at all (avoids the cleanup tool itself causing buildup).
    $repoName = Split-Path $repo -Leaf

    # --- step 2: classify + remove worktrees ------------------------------
    $wt = git -C $repo worktree list --porcelain
    $entries = @(); $cur = @{}
    foreach ($line in $wt) {
        if ($line -match '^worktree (.+)$') { if ($cur.Count) { $entries += [pscustomobject]$cur }; $cur = @{ Path = $Matches[1].Trim() } }
        elseif ($line -match '^HEAD (.+)$') { $cur.Head = $Matches[1].Trim() }
        elseif ($line -match '^branch (.+)$') { $cur.Branch = ($Matches[1].Trim() -replace '^refs/heads/','') }
        elseif ($line -match '^detached') { $cur.Branch = $null; $cur.Detached = $true }
    }
    if ($cur.Count) { $entries += [pscustomobject]$cur }

    foreach ($e in $entries) {
        $rp = Resolve-Path $e.Path -ErrorAction SilentlyContinue
        $p = if ($rp) { $rp.Path } else { $null }
        if (-not $p) { continue }
        if ($p -eq $active) { continue }                      # never the active checkout
        if ($p -eq (Resolve-Path $repo).Path) { continue }    # never the primary checkout
        $bn = $e.Branch
        if ($bn -and ($bn -in 'main','master','develop')) { Info "keep (protected): $(Split-Path $p -Leaf) [$bn]"; continue }

        $statusLines = @(Invoke-GitQuiet -C $p status --porcelain)
        if ($LASTEXITCODE -ne 0) {
            Warn "keep (status failed): $p"
            continue
        }
        $dirty = $statusLines.Count
        $merged = Is-Merged $repo $e.Head $bn $def
        $reason = $null
        if ($merged) { $reason = 'merged' }
        elseif ($bn) {
            # unmerged named branch: safe only if its commits are on a remote (recoverable)
            $onRemote = (Invoke-GitQuiet -C $repo branch -r --contains $e.Head | Measure-Object).Count
            if ($onRemote -gt 0) { $reason = 'unmerged-but-pushed' }
        }

        if (-not $reason -and -not $Force) {
            Warn "keep (UNMERGED work, not on remote): $(Split-Path $p -Leaf) [$bn] HEAD=$($e.Head.Substring(0,9)) - use -Force to remove (backed up in bundle)"
            continue
        }
        if ($dirty -gt 0) {
            Warn "keep (DIRTY: $dirty uncommitted or untracked): $(Split-Path $p -Leaf) - commit or move the changes before cleanup"
            continue
        }
        try {
            $ignoredData = @(Get-WorktreeNonDisposableIgnored -Path $p -DisposablePatterns $DisposableIgnoredPatterns)
        } catch {
            Warn "keep (ignored-file inventory failed): $p - $($_.Exception.Message)"
            continue
        }
        if ($ignoredData.Count -gt 0) {
            Warn "keep (IGNORED files outside the disposable allowlist: $($ignoredData.Count), e.g. $($ignoredData[0])): $(Split-Path $p -Leaf) - move or back up them before cleanup"
            continue
        }
        $why = if ($reason) { $reason } else { 'forced' }
        $sizeGB = if (Test-Path $p) { [math]::Round((Get-ChildItem $p -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum/1GB,1) } else { 0 }
        Act "remove worktree ($why, ~$sizeGB GB): $p"
        if ($script:DidApply) {
            # Lazy targeted backup: ONLY for genuinely-unmerged, not-pushed work
            # (the -Force path). Bundle just this HEAD's unique commits (--not the
            # default branch) so it's tiny, and tag it so the objects survive.
            if ($why -eq 'forced') {
                # #265: the short head sha makes tag + bundle unique per head (same-named worktrees never collide).
                $safe = ((Split-Path $p -Leaf) -replace '[^A-Za-z0-9_-]','_') + '-' + $e.Head.Substring(0,9)
                $b = Join-Path $BackupDir "$repoName-$safe-$stampDate.bundle"
                $tag = "maint-backup/$stampDate/$safe"
                $null = Invoke-GitQuiet -C $repo tag -f $tag $e.Head
                # #263: a bundle records REFS; a raw sha names none, so bundle the backup tag.
                if ($LASTEXITCODE -eq 0) { $null = Invoke-GitQuiet -C $repo bundle create $b "refs/tags/$tag" --not "refs/heads/$def" "refs/remotes/origin/$def" }
                if ($LASTEXITCODE -eq 0 -and (Test-Path $b)) { Info "backed up unmerged work: $b ($([math]::Round((Get-Item $b).Length/1MB,1)) MB)" }
                else { Die "Refusing to remove unmerged worktree $p - backup bundle could not be created." }
            }
            $null = Invoke-GitQuiet -C $repo worktree remove $p
            if ($LASTEXITCODE -ne 0) {
                Warn "git refused worktree removal; preserving remaining files: $p"
                continue
            }
            $totalRemoved++
        }
    }
    if ($script:DidApply) { $null = Invoke-GitQuiet -C $repo worktree prune; if ($LASTEXITCODE -ne 0) { Warn "git worktree prune failed (exit $LASTEXITCODE): $repo" } }

    # --- step 3: delete [gone] local branches -----------------------------
    # #264: structured fields, never `branch -vv` text (a subject can imitate "[x: gone]"): gone = track exactly [gone]; checked out = worktreepath set.
    $refs = @(Invoke-GitQuiet -C $repo for-each-ref '--format=%(refname:lstrip=2)%09%(upstream:track)%09%(worktreepath)' refs/heads)
    if ($LASTEXITCODE -ne 0) { Warn "git for-each-ref failed (exit $LASTEXITCODE): no gone branches deleted in $repo"; $refs = @() }
    $gone = @($refs | ForEach-Object {
        $f = $_ -split "`t", 3
        if ($f[1] -eq '[gone]' -and $f[0] -notin 'main','master','develop') { [pscustomobject]@{ Name = $f[0]; CheckedOut = [bool]$f[2] } }
    })
    foreach ($g in $gone) {
        if ($g.CheckedOut -or $g.Name -eq $def) { Warn "skip delete gone branch: $($g.Name) (checked out in a worktree, or the default branch)"; continue }
        if (-not (Backup-GoneBranch $repo $g.Name $def $stampDate)) { continue }
        if ($script:DidApply) { $null = Invoke-GitQuiet -C $repo branch -D $g.Name }
        if (-not $script:DidApply -or $LASTEXITCODE -eq 0) { Act "delete gone branch: $($g.Name)" } else { Warn "git refused to delete gone branch: $($g.Name) (exit $LASTEXITCODE)" }
    }

    # --- step 4: cargo-sweep target/ --------------------------------------
    if (Test-Path (Join-Path $repo 'Cargo.toml')) {
        if ($HaveSweep) {
            $days = $SweepDays
            if ((Free-GB) -lt $MinFreeGB) { Warn "low disk (<$MinFreeGB GB) - escalating sweep to 0-day on $repoName"; $days = 0 }
            Act "cargo sweep --time $days (recursive) on $repo"
            # #262: local non-Stop scope, so 5.1 does not abort on the first stderr line; the exit code is checked below.
            if ($script:DidApply) { & { $ErrorActionPreference = 'Continue'; & cargo-sweep sweep --time $days --recursive $repo 2>&1 } | ForEach-Object { Info "  sweep: $_" }; if ($LASTEXITCODE -ne 0) { Warn "cargo-sweep exited $LASTEXITCODE on $repo" } }
        } else {
            Warn "cargo-sweep not installed - age-based artifact GC skipped. Native size caps still run below."
        }
    }
    Invoke-GeneratedArtifactHygiene $repo
}

# --- retention: never let the backup dir itself become buildup ------------
if ($script:DidApply -and (Test-Path $BackupDir)) {
    $cutoff = (Get-Date).AddDays(-30)
    $old = Get-ChildItem $BackupDir -Filter *.bundle -ErrorAction SilentlyContinue | Where-Object { $_.LastWriteTime -lt $cutoff }
    foreach ($o in $old) { Act "prune old backup bundle (>30d): $($o.Name)"; [System.IO.File]::Delete($o.FullName) }
}

$freeAfter = Free-GB
Write-Host ""
Info "Disk $drive`: free after = $freeAfter GB  (delta = $([math]::Round($freeAfter-$freeBefore,1)) GB; worktrees removed = $totalRemoved)"
if (-not $script:DidApply) { Info "DRY-RUN only. Re-run with -Apply to perform the actions above." }
