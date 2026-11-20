$ErrorActionPreference = 'Stop'

# #266 (Pester 3.4.0, child processes: Windows PowerShell 5.1, plus pwsh 7 when installed).
# Added 2026-10-16 20:40 +03:00.
# Bug: step 3 of repo-maintenance.ps1 force-deletes (`git branch -D`) every local branch whose upstream is [gone]
# and keeps no ref first, so an UNMERGED branch (a PR closed unmerged, remote branch deleted) loses its only ref.
# Frozen criteria (criteria_266.md): before deleting a gone branch that is not merged (the script's Is-Merged rule)
# a backup tag  maint-backup/<date>/gone-<branch with '/' -> '-'>-<sha9>  pointing at the branch head must exist; if
# it cannot be created the branch is KEPT with a Warn; an existing tag of that name is never overwritten (same
# head => reuse, different head => keep + Warn); a failed `git for-each-ref` branch query warns with its exit code
# and deletes nothing; merged gone branches are deleted without a tag; dry-run creates no tag; the script header
# describes the real backups. <date> may be yyyyMMdd or yyyy-MM-dd: the tests accept both and, where a tag must be
# pre-created to collide, create both. Outcome-based: refs, tags, branches, script output lines, exit code.
Describe 'repo-maintenance gone-branch backup tag (#266)' {
    BeforeAll {
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')

        # Same-name collision tags for both plausible date formats (the exact format is not dictated).
        function Fx-NewCollisionTags($Fx, [string]$Branch, [string]$Target) {
            $names = @()
            foreach ($fmt in 'yyyyMMdd', 'yyyy-MM-dd') {
                $name = "maint-backup/$(Get-Date -Format $fmt)/gone-$Branch-$($Fx.Head.Substring(0, 9))"
                Fx-Git -C $Fx.Repo tag $name $Target
                $names += $name
            }
            return $names
        }
        function Fx-AssertKeptWithWarn($Fx, $Run, [string]$Branch) {
            Fx-AssertCompleted $Run
            (Fx-BranchExists $Fx.Repo $Branch) | Should Be $true
            (Fx-GitOut -C $Fx.Repo rev-parse "refs/heads/$Branch") | Should Be $Fx.Head
            $Run.Text | Should Not Match "\[APPLY\] delete gone branch: $Branch(\s|`$)"
            (Fx-Lines $Run.Text "\[maint\]\[WARN\].*\b$Branch\b").Count | Should Not Be 0
        }

        # ---- scenarios (each builds a fixture, runs the real script, asserts) ----
        function Fx-ScenarioUnmergedBackedUp([string]$Exe) {
            $fx = Fx-NewGoneBranch "g266-ok-$(Fx-ExeTag $Exe)" 'topic/x'
            Fx-Git -C $fx.Repo branch stays-local           # not gone: must be untouched and get no tag
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertCompleted $r
            (Fx-BranchExists $fx.Repo 'topic/x') | Should Be $false
            (Fx-BranchExists $fx.Repo 'stays-local') | Should Be $true
            $tags = @(Fx-GoneTags $fx)
            $tags.Count | Should Be 1
            $tags[0] | Should Match "^maint-backup/\d{4}-?\d{2}-?\d{2}/gone-topic-x-$($fx.Head.Substring(0, 9))`$"
            (Fx-TagTarget $fx $tags[0]) | Should Be $fx.Head
        }
        function Fx-ScenarioTagNamespaceBlocked([string]$Exe) {
            $fx = Fx-NewGoneBranch "g266-block-$(Fx-ExeTag $Exe)" 'wip'
            Fx-Git -C $fx.Repo tag maint-backup main       # a TAG named maint-backup: no maint-backup/... tag can be created
            Fx-AssertKeptWithWarn $fx (Fx-RunMaint $fx $Exe @('-Apply')) 'wip'
            @(Fx-GoneTags $fx).Count | Should Be 0
        }
        function Fx-ScenarioForEachRefFails([string]$Exe) {
            $fx = Fx-NewGoneBranch "g266-fer-$(Fx-ExeTag $Exe)" 'wip'
            $real = (Get-Command git -CommandType Application | Select-Object -First 1).Source
            $bin = Join-Path $TestDrive "g266-fer-$(Fx-ExeTag $Exe)\gitshim"
            New-Item -ItemType Directory -Force -Path $bin | Out-Null
            # git.cmd shim first on PATH: for-each-ref fails with 128, everything else is forwarded to the real git.
            $shim = @('@echo off', 'if /i "%~1"=="for-each-ref" goto fail', 'if /i "%~3"=="for-each-ref" goto fail',
                "`"$real`" %*", 'exit /b %errorlevel%', ':fail', 'echo fatal: simulated for-each-ref failure 1>&2', 'exit /b 128')
            [System.IO.File]::WriteAllText((Join-Path $bin 'git.cmd'), (($shim -join "`r`n") + "`r`n"), [System.Text.Encoding]::ASCII)
            $r = Fx-RunMaint $fx $Exe @('-Apply') @{ PATH = "$bin;$env:PATH" }
            Fx-AssertCompleted $r
            (Fx-BranchExists $fx.Repo 'wip') | Should Be $true
            $r.Text | Should Not Match '\[APPLY\] delete gone branch'
            @(Fx-GoneTags $fx).Count | Should Be 0
            (Fx-Lines $r.Text '\[maint\]\[WARN\].*\b128\b').Count | Should Not Be 0
        }
        $script:noPwsh = [bool](-not $script:FxPwsh)
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_fail_unmerged_gone_branch_gets_a_backup_tag_at_its_old_head_then_is_deleted' {
        Fx-ScenarioUnmergedBackedUp $script:FxPs51
    }
    It 'must_fail_pwsh7_unmerged_gone_branch_gets_a_backup_tag_at_its_old_head_then_is_deleted' -Skip:$script:noPwsh {
        Fx-ScenarioUnmergedBackedUp $script:FxPwsh
    }
    It 'must_pass_merged_gone_branch_is_deleted_without_a_backup_tag' {
        $fx = Fx-NewGoneBranch 'g266-merged' 'done' -Merged
        $r = Fx-RunMaint $fx $script:FxPs51 @('-Apply')
        Fx-AssertCompleted $r
        (Fx-BranchExists $fx.Repo 'done') | Should Be $false
        @(Fx-GoneTags $fx).Count | Should Be 0
        (Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*') | Should BeNullOrEmpty
    }
    It 'must_fail_gone_branch_is_kept_with_a_warn_when_no_backup_tag_can_be_created' {
        Fx-ScenarioTagNamespaceBlocked $script:FxPs51
    }
    It 'must_fail_pwsh7_gone_branch_is_kept_with_a_warn_when_no_backup_tag_can_be_created' -Skip:$script:noPwsh {
        Fx-ScenarioTagNamespaceBlocked $script:FxPwsh
    }
    It 'must_fail_existing_backup_tag_of_the_same_name_at_a_different_commit_is_never_overwritten_branch_kept' {
        $fx = Fx-NewGoneBranch 'g266-collide' 'wip'
        $other = Fx-GitOut -C $fx.Repo rev-parse main
        $names = Fx-NewCollisionTags $fx 'wip' $other
        Fx-AssertKeptWithWarn $fx (Fx-RunMaint $fx $script:FxPs51 @('-Apply')) 'wip'
        foreach ($n in $names) { (Fx-TagTarget $fx $n) | Should Be $other }
        @(Fx-GoneTags $fx).Count | Should Be $names.Count
    }
    It 'must_pass_existing_backup_tag_of_the_same_name_at_the_same_head_is_reused_and_branch_deleted' {
        $fx = Fx-NewGoneBranch 'g266-reuse' 'wip'
        $names = Fx-NewCollisionTags $fx 'wip' $fx.Head
        $r = Fx-RunMaint $fx $script:FxPs51 @('-Apply')
        Fx-AssertCompleted $r
        (Fx-BranchExists $fx.Repo 'wip') | Should Be $false
        foreach ($n in $names) { (Fx-TagTarget $fx $n) | Should Be $fx.Head }
        @(Fx-GoneTags $fx).Count | Should Be $names.Count
    }
    It 'must_fail_for_each_ref_failure_warns_with_the_exit_code_and_deletes_nothing' {
        Fx-ScenarioForEachRefFails $script:FxPs51
    }
    It 'must_fail_pwsh7_for_each_ref_failure_warns_with_the_exit_code_and_deletes_nothing' -Skip:$script:noPwsh {
        Fx-ScenarioForEachRefFails $script:FxPwsh
    }
    It 'must_pass_dry_run_creates_no_tag_and_deletes_nothing' {
        $fx = Fx-NewGoneBranch 'g266-dry' 'wip'
        Fx-Git -C $fx.Repo fetch --prune --quiet           # a dry-run never fetches: make the branch [gone] up front
        $r = Fx-RunMaint $fx $script:FxPs51 @()
        Fx-AssertCompleted $r
        (Fx-BranchExists $fx.Repo 'wip') | Should Be $true
        (Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*') | Should BeNullOrEmpty
        (Fx-Lines $r.Text '\bwip\b').Count | Should Not Be 0
    }

    # ---- header text guard: it must describe the real backups ----
    It 'must_fail_header_no_longer_claims_an_up_front_bundle_backup_step' {
        $h = Fx-HeaderText
        $h | Should Not Match '(?i)bundle every branch'
        $h | Should Not Match '(?i)worktrees-<date>\.bundle'
        $h | Should Not Match '(?is)BEFORE touching\s+anything'
        $h | Should Not Match '(?i)preceded by the bundle backup in step 1'
    }
    It 'must_fail_header_describes_the_tag_backup_of_unmerged_gone_branches' {
        $h = Fx-HeaderText
        $h | Should Match 'maint-backup'
        $step3 = [regex]::Match($h, '(?s)3\.\s+BRANCHES(.*?)4\.\s+SWEEP').Groups[1].Value
        $step3 | Should Not BeNullOrEmpty
        $step3 | Should Match '(?i)\btag\b'
        $step3 | Should Match '(?i)unmerged'
    }
}
