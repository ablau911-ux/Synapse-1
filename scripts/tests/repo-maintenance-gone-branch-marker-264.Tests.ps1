$ErrorActionPreference = 'Stop'

# #264 (Pester 3.4.0, child processes: Windows PowerShell 5.1, plus pwsh 7 when installed).
# Added 2026-10-16 19:10 +03:00; round 2 (F1, F2, T1) added 2026-10-16 19:40 +03:00.
# Bug: step 3 of repo-maintenance.ps1 reads `git branch -vv` and strips only a leading `* `. A branch that is
# checked out in ANOTHER worktree is printed with a `+ ` prefix, so the first word becomes '+' and the script
# runs `git branch -D +`. A real local branch literally named '+' (the decoy below) is then deleted, which is
# a physical, stderr-independent signal; the checked-out gone branch itself is never recognised.
# Round 2 (review F1/F2): free text of `branch -vv` (the commit subject) must never decide that a branch is gone,
# and a `branch -D` that git refuses must be WARNED about, never printed as a successful delete.
# The tests assert outcomes only (branch refs, worktree files, exit code, script output lines), never how git
# stderr is suppressed, so they stay valid whatever the #262 fix does.
Describe 'repo-maintenance gone-branch handling (#264)' {
    BeforeAll {
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')

        # Push $Branch with upstream, then delete it on the bare origin: fetch --prune makes it `[gone]`.
        function Fx-AddGoneBranch($Fx, [string]$Branch) {
            Fx-Git -C $Fx.Repo branch $Branch
            Fx-Git -C $Fx.Repo push -q -u origin $Branch
            Fx-Git -C $Fx.Origin branch -D $Branch
        }
        function Fx-ScenarioPlusMarker([string]$Exe) {
            $fx = Fx-NewRepo "plus-$(Fx-ExeTag $Exe)" -WithOrigin
            $wt = Join-Path $fx.Wts 'feat-wt'
            Fx-Git -C $fx.Repo branch feat
            Fx-Git -C $fx.Repo push -q -u origin feat
            Fx-Git -C $fx.Repo worktree add -q $wt feat   # feat is now "+ feat" in the primary's branch -vv
            [System.IO.File]::WriteAllText((Join-Path $wt 'wip.txt'), 'uncommitted')   # dirty: step 2 keeps it, marker survives to step 3
            Fx-Git -C $fx.Repo branch '+'                 # decoy local branch literally named '+'
            Fx-Git -C $fx.Origin branch -D feat
            Fx-AddGoneBranch $fx 'old'
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertCompleted $r
            $r.Text | Should Not Match 'delete gone branch: \+'
            Fx-AssertKeptNotDeleted $r.Text 'feat'
            (Fx-BranchExists $fx.Repo '+') | Should Be $true
            (Fx-BranchExists $fx.Repo 'feat') | Should Be $true
            [System.IO.File]::ReadAllText((Join-Path $wt 'wip.txt')) | Should Be 'uncommitted'
            (Fx-BranchExists $fx.Repo 'old') | Should Be $false
        }
        function Fx-ScenarioNotCheckedOut([string]$Exe) {
            $fx = Fx-NewRepo "plain-$(Fx-ExeTag $Exe)" -WithOrigin
            Fx-AddGoneBranch $fx 'old'
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertCompleted $r
            (Fx-BranchExists $fx.Repo 'old') | Should Be $false
            (Fx-BranchExists $fx.Repo 'main') | Should Be $true
            (Fx-Lines $r.Text '\[APPLY\] delete gone branch: old').Count | Should Not Be 0   # a real delete is reported as one
        }
        function Fx-ScenarioStarMarker([string]$Exe) {
            $fx = Fx-NewRepo "star-$(Fx-ExeTag $Exe)" -WithOrigin
            Fx-AddGoneBranch $fx 'feat'
            Fx-Git -C $fx.Repo checkout -q feat   # primary's own current branch: "* feat", git refuses to delete it
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertCompleted $r
            $r.Text | Should Not Match 'delete gone branch: \*'
            Fx-AssertKeptNotDeleted $r.Text 'feat'
            (Fx-BranchExists $fx.Repo 'feat') | Should Be $true
        }
        # F1: the commit SUBJECT that `branch -vv` prints imitates a gone marker. Neither a never-pushed branch
        # nor a branch whose upstream still exists is gone; only the real `old` (upstream deleted) is.
        function Fx-ScenarioMarkerInSubject([string]$Exe, [bool]$Apply) {
            $fx = Fx-NewRepo "subject-$(Fx-ExeTag $Exe)-$Apply" -WithOrigin
            Fx-Git -C $fx.Repo checkout -q -b localonly
            Fx-Commit $fx.Repo 'lo.txt' 'fix: parse [origin/x: gone] marker'
            Fx-Git -C $fx.Repo checkout -q -b tracked
            Fx-Git -C $fx.Repo push -q -u origin tracked   # upstream exists and stays: not gone
            Fx-Git -C $fx.Repo checkout -q main
            Fx-AddGoneBranch $fx 'old'
            $r = Fx-RunMaint $fx $Exe $(if ($Apply) { @('-Apply') } else { @() })
            Fx-AssertCompleted $r
            $r.Text | Should Not Match 'delete gone branch: localonly'
            $r.Text | Should Not Match 'delete gone branch: tracked'
            (Fx-BranchExists $fx.Repo 'localonly') | Should Be $true
            (Fx-BranchExists $fx.Repo 'tracked') | Should Be $true
            # Only -Apply fetches (--prune), so only then is `old` really gone and deleted; a dry-run changes nothing.
            (Fx-BranchExists $fx.Repo 'old') | Should Be (-not $Apply)
            if ($Apply) { (Fx-Lines $r.Text '\[APPLY\] delete gone branch: old').Count | Should Not Be 0 }
        }
        # F2: a ref lock (another git process) makes git refuse `branch -D old`. The script must warn with
        # git's exit code and must not print the success line.
        function Fx-ScenarioDeleteRefused([string]$Exe) {
            $fx = Fx-NewRepo "refused-$(Fx-ExeTag $Exe)" -WithOrigin
            Fx-AddGoneBranch $fx 'old'
            [System.IO.File]::WriteAllText((Join-Path $fx.Repo '.git\refs\heads\old.lock'), '')
            $code = Fx-GitCode -C $fx.Repo branch -D old
            $code | Should Not Be 0
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertCompleted $r
            (Fx-BranchExists $fx.Repo 'old') | Should Be $true
            $r.Text | Should Not Match '\[APPLY\] delete gone branch: old(\s|$)'
            $warn = @(Fx-Lines $r.Text '\[maint\]\[WARN\].*\bold\b')
            $warn.Count | Should Not Be 0
            $warn[0] | Should Match "\b$code\b"
        }
        # ---- source scanner for the prune guard: is $LASTEXITCODE read at/after the call in its block? ----
        function Fx-ExitCodeRead($Call) {
            $stmt = $Call
            while ($null -ne $stmt.Parent -and $stmt.Parent.GetType().Name -notin 'StatementBlockAst', 'NamedBlockAst') { $stmt = $stmt.Parent }
            if ($null -eq $stmt.Parent) { return $false }
            foreach ($s in $stmt.Parent.Statements) {
                if ($s.Extent.StartOffset -lt $stmt.Extent.StartOffset) { continue }
                $reads = $s.FindAll({ param($n) $n.GetType().Name -eq 'VariableExpressionAst' -and $n.VariablePath.UserPath -eq 'LASTEXITCODE' }, $true)
                if ($reads.Count -gt 0) { return $true }
            }
            return $false
        }
        function Fx-FindUncheckedPrune([string]$Source) {
            $tok = $null
            $err = $null
            $tree = [System.Management.Automation.Language.Parser]::ParseInput($Source, [ref]$tok, [ref]$err)
            if ($err.Count -gt 0) { throw ($err | Out-String) }
            $calls = $tree.FindAll({ param($n)
                    if ($n.GetType().Name -ne 'CommandAst') { return $false }
                    $words = @($n.CommandElements | ForEach-Object { $_.Extent.Text })
                    return (($words -contains 'worktree') -and ($words -contains 'prune') -and ($words[0] -in 'git', 'Invoke-GitQuiet')) }, $true)
            $bad = foreach ($c in $calls) { if (-not (Fx-ExitCodeRead $c)) { "line $($c.Extent.StartLineNumber): $($c.Extent.Text)" } }
            return @($bad)
        }
        $script:noPwsh = [bool](-not $script:FxPwsh)
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_fail_gone_branch_checked_out_in_another_worktree_is_kept_and_plus_marker_never_deleted' {
        Fx-ScenarioPlusMarker $script:FxPs51
    }
    It 'must_fail_pwsh7_gone_branch_checked_out_in_another_worktree_is_kept_and_plus_marker_never_deleted' -Skip:$script:noPwsh {
        Fx-ScenarioPlusMarker $script:FxPwsh
    }
    It 'must_pass_gone_branch_not_checked_out_is_still_deleted' {
        Fx-ScenarioNotCheckedOut $script:FxPs51
    }
    It 'must_pass_gone_branch_checked_out_in_primary_star_marker_is_kept_unchanged' {
        Fx-ScenarioStarMarker $script:FxPs51
    }
    It 'must_pass_pwsh7_gone_branch_checked_out_in_primary_star_marker_is_kept_unchanged' -Skip:$script:noPwsh {
        Fx-ScenarioStarMarker $script:FxPwsh
    }

    # ---- round 2 F1: free text must never make a branch "gone" ----
    It 'must_fail_local_only_branch_with_gone_marker_in_commit_subject_survives_apply' {
        Fx-ScenarioMarkerInSubject $script:FxPs51 $true
    }
    It 'must_fail_pwsh7_local_only_branch_with_gone_marker_in_commit_subject_survives_apply' -Skip:$script:noPwsh {
        Fx-ScenarioMarkerInSubject $script:FxPwsh $true
    }
    It 'must_fail_local_only_branch_with_gone_marker_in_commit_subject_is_not_reported_in_dry_run' {
        Fx-ScenarioMarkerInSubject $script:FxPs51 $false
    }

    # ---- round 2 F2: a refused delete is a warning, not a success line ----
    It 'must_fail_branch_delete_refused_by_git_is_warned_with_exit_code_and_not_reported_as_deleted' {
        Fx-ScenarioDeleteRefused $script:FxPs51
    }
    It 'must_fail_pwsh7_branch_delete_refused_by_git_is_warned_with_exit_code_and_not_reported_as_deleted' -Skip:$script:noPwsh {
        Fx-ScenarioDeleteRefused $script:FxPwsh
    }

    # ---- round 2 F2: `git worktree prune` exits 0 even when it fails to delete (verified with a held file
    # handle: "error: failed to delete ..." and exit 0), so no failing input can be provoked. Source guard:
    # the exit code of the prune call must be read, or the call must go through a helper that reports it.
    It 'must_pass_scanner_flags_a_prune_call_whose_exit_code_is_never_read' {
        function Fx-Prune([string]$Body) { return @(Fx-FindUncheckedPrune ("function F {`n" + $Body + "`n}")) }
        (Fx-Prune 'if ($a) { $null = Invoke-GitQuiet -C $r worktree prune }').Count | Should Be 1
        (Fx-Prune 'git -C $r worktree prune').Count | Should Be 1
        (Fx-Prune 'if ($a) { $null = Invoke-GitQuiet -C $r worktree prune; if ($LASTEXITCODE -ne 0) { Warn x } }').Count | Should Be 0
        (Fx-Prune '$null = Invoke-GitQuiet -C $r worktree prune
            if ($LASTEXITCODE) { Warn x }').Count | Should Be 0
        (Fx-Prune 'Invoke-GitReported -C $r worktree prune').Count | Should Be 0
        (Fx-Prune '$null = Invoke-GitQuiet -C $r worktree list').Count | Should Be 0
    }
    It 'must_fail_source_scan_worktree_prune_exit_code_is_checked' {
        $bad = @(Fx-FindUncheckedPrune ([System.IO.File]::ReadAllText($script:FxMaint)))
        ($bad -join ' | ') | Should BeNullOrEmpty
    }
}
