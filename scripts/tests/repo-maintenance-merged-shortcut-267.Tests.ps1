$ErrorActionPreference = 'Stop'

# #267 (Pester 3.4.0, child processes: Windows PowerShell 5.1, plus pwsh 7 when installed).
# Added 2026-10-16 21:10 +03:00.
# Bug: Is-Merged falls back to `gh pr list --head <branch> --state merged` run in the SCRIPT's working directory
# (not the repo being processed) and matched by branch NAME only. A fake gh.cmd first on PATH plays "wrong repo,
# merged PR for that name, different head sha". Frozen criteria (criteria_267.md): a branch/worktree counts as merged
# (no backup) only when its head is an ancestor of local <default> or origin/<default>; any PR shortcut must target
# the processed repo AND require the PR head sha == branch head. So with the fake gh an UNMERGED gone branch or
# worktree must still be backed up (or kept). -Apply names the created gone-branch tag; dry-run says "would tag <name>".
# Outcome-based: refs, tags, bundles, worktree dirs, output lines, header text. The tests do not dictate whether the
# fix drops the gh shortcut or pins it; the fake answers "merged" to every lookup that does not carry the real head sha.
Describe 'repo-maintenance merged shortcut and backup reporting (#267)' {
    BeforeAll {
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')

        # gh.cmd: `repo view` -> another repo; a lookup carrying the real head sha (a pinned, sha-exact query) -> no
        # match; any other `pr list` -> a merged PR whose head sha is 000... (name-only match, wrong sha).
        function Fx-NewFakeGh([string]$Case, [string]$Head) {
            $bin = Join-Path $TestDrive "$Case\fakegh"
            New-Item -ItemType Directory -Force -Path $bin | Out-Null
            $lines = @('@echo off', "echo %* >> `"$(Join-Path $bin 'gh-calls.log')`"",
                "echo %* | findstr /c:`"$Head`" >nul", 'if not errorlevel 1 exit /b 0',
                'echo %* | findstr /c:"repo view" >nul', 'if not errorlevel 1 goto repoview',
                'echo %* | findstr /c:"headRefOid" >nul', 'if not errorlevel 1 goto json',
                'echo 1', 'exit /b 0', ':repoview', 'echo other-owner/other-repo', 'exit /b 0',
                ':json', 'echo [{"number":1,"headRefOid":"0000000000000000000000000000000000000000"}]', 'exit /b 0')
            [System.IO.File]::WriteAllText((Join-Path $bin 'gh.cmd'), (($lines -join "`r`n") + "`r`n"), [System.Text.Encoding]::ASCII)
            return $bin
        }
        function Fx-NewUnmergedWorktree267([string]$Case) {
            $fx = Fx-NewRepo $Case -WithOrigin
            $fx.Wt = Fx-NewWorktree $fx 'unmerged'
            Fx-Commit $fx.Wt 'unique.txt'
            $fx.Head = Fx-GitOut -C $fx.Wt rev-parse HEAD
            return $fx
        }
        function Fx-ScenarioGhGoneBranch([string]$Exe) {
            $case = "g267-gone-$(Fx-ExeTag $Exe)"
            $fx = Fx-NewGoneBranch $case 'wip'
            $bin = Fx-NewFakeGh $case $fx.Head
            Fx-AssertCompleted (Fx-RunMaint $fx $Exe @('-Apply') @{ PATH = "$bin;$env:PATH" })
            # Whatever the fix decides (tag then delete, or keep), the unmerged head must stay referenced.
            (Fx-GitOut -C $fx.Repo for-each-ref --points-at $fx.Head) | Should Not BeNullOrEmpty
        }
        function Fx-ScenarioGhWorktreeKept([string]$Exe) {
            $case = "g267-wtkeep-$(Fx-ExeTag $Exe)"
            $fx = Fx-NewUnmergedWorktree267 $case
            $bin = Fx-NewFakeGh $case $fx.Head
            $r = Fx-RunMaint $fx $Exe @('-Apply') @{ PATH = "$bin;$env:PATH" }
            Fx-AssertCompleted $r
            Test-Path -LiteralPath $fx.Wt | Should Be $true
            $r.Text | Should Match 'keep \(UNMERGED work, not on remote\)'
            $r.Text | Should Match 'worktrees removed = 0'
        }
        function Fx-ScenarioGhWorktreeForced([string]$Exe) {
            $case = "g267-wtforce-$(Fx-ExeTag $Exe)"
            $fx = Fx-NewUnmergedWorktree267 $case
            $bin = Fx-NewFakeGh $case $fx.Head
            Fx-AssertCompleted (Fx-RunMaint $fx $Exe @('-Apply', '-Force') @{ PATH = "$bin;$env:PATH" })
            Test-Path -LiteralPath $fx.Wt | Should Be $false
            $bundles = @(Get-ChildItem -LiteralPath $fx.Backup -Filter *.bundle -ErrorAction SilentlyContinue)
            $bundles.Count | Should Be 1
            (Fx-GitOut -C $fx.Repo bundle verify $bundles[0].FullName) | Should Match 'okay'
            $tags = @((Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*') -split '\r?\n' | Where-Object { $_ })
            @($tags | Where-Object { (Fx-TagTarget $fx $_) -eq $fx.Head }).Count | Should Be 1
        }
        # ---- r2 (review): ambiguous / self-compared names ----
        # Gone local branch named $Name holding a never-pushed unique commit; its upstream origin/foo is deleted on
        # the origin, so `fetch --prune` makes it [gone]. $fx.Head is that commit.
        function Fx-NewGoneNamed([string]$Case, [string]$Name) {
            $fx = Fx-NewRepo $Case -WithOrigin
            Fx-Git -C $fx.Repo push -q origin main:foo
            Fx-Git -C $fx.Repo checkout -q -b $Name main
            Fx-Commit $fx.Repo 'unique.txt'
            $fx.Head = Fx-GitOut -C $fx.Repo rev-parse "refs/heads/$Name"
            Fx-Git -C $fx.Repo config "branch.$Name.remote" origin
            Fx-Git -C $fx.Repo config "branch.$Name.merge" refs/heads/foo
            Fx-Git -C $fx.Repo checkout -q main
            Fx-Git -C $fx.Origin branch -D foo
            return $fx
        }
        # origin/HEAD points at a default branch ($Name) that no longer exists on the remote; fetch must not repair it.
        function Fx-SetStaleOriginHead($Fx, [string]$Name) {
            Fx-Git -C $Fx.Repo config remote.origin.followRemoteHEAD never
            Fx-Git -C $Fx.Repo symbolic-ref refs/remotes/origin/HEAD "refs/remotes/origin/$Name"
        }
        # Apply: the unmerged commit must stay referenced (tag then delete, or keep). Dry-run: "would tag" or a keep WARN.
        function Fx-AssertBackedUpOrKept($Fx, $Run, [string]$Name, [bool]$Apply) {
            Fx-AssertCompleted $Run
            $sha9 = $Fx.Head.Substring(0, 9)
            if ($Apply) { (Fx-GitOut -C $Fx.Repo for-each-ref --points-at $Fx.Head) | Should Not BeNullOrEmpty; return }
            (Fx-BranchExists $Fx.Repo $Name) | Should Be $true
            (Fx-GitOut -C $Fx.Repo tag --list 'maint-backup/*') | Should BeNullOrEmpty
            $slug = $Name -replace '/', '-'
            $said = @(Fx-Lines $Run.Text "would tag\s+maint-backup/\S+/gone-$([regex]::Escape($slug))-$sha9").Count + @(Fx-Lines $Run.Text "\[WARN\].*$([regex]::Escape($Name))").Count
            $said | Should Not Be 0
        }
        function Fx-ScenarioOriginMainName([string]$Exe, [bool]$Apply) {
            $fx = Fx-NewGoneNamed "g267-ambig-$(Fx-ExeTag $Exe)-$Apply" 'origin/main'
            if (-not $Apply) { Fx-Git -C $fx.Repo fetch --prune --quiet }
            Fx-AssertBackedUpOrKept $fx (Fx-RunMaint $fx $Exe $(if ($Apply) { @('-Apply') } else { @() })) 'origin/main' $Apply
        }
        function Fx-ScenarioStaleOriginHead([string]$Exe, [bool]$Apply) {
            $fx = Fx-NewGoneNamed "g267-stale-$(Fx-ExeTag $Exe)-$Apply" 'trunk'
            Fx-SetStaleOriginHead $fx 'trunk'
            if (-not $Apply) { Fx-Git -C $fx.Repo fetch --prune --quiet }
            Fx-AssertBackedUpOrKept $fx (Fx-RunMaint $fx $Exe $(if ($Apply) { @('-Apply') } else { @() })) 'trunk' $Apply
        }
        $script:noPwsh = [bool](-not $script:FxPwsh)
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    # ---- name-only / wrong-repo PR match must never count as merged ----
    It 'must_fail_fake_gh_merged_pr_for_the_branch_name_does_not_skip_the_backup_of_an_unmerged_gone_branch' {
        Fx-ScenarioGhGoneBranch $script:FxPs51
    }
    It 'must_fail_pwsh7_fake_gh_merged_pr_for_the_branch_name_does_not_skip_the_backup_of_an_unmerged_gone_branch' -Skip:$script:noPwsh {
        Fx-ScenarioGhGoneBranch $script:FxPwsh
    }
    It 'must_fail_fake_gh_merged_pr_for_the_branch_name_does_not_make_an_unmerged_worktree_removable' {
        Fx-ScenarioGhWorktreeKept $script:FxPs51
    }
    It 'must_fail_fake_gh_merged_pr_for_the_branch_name_does_not_skip_the_bundle_and_tag_of_a_forced_worktree' {
        Fx-ScenarioGhWorktreeForced $script:FxPs51
    }
    It 'must_fail_pwsh7_fake_gh_merged_pr_for_the_branch_name_does_not_skip_the_bundle_and_tag_of_a_forced_worktree' -Skip:$script:noPwsh {
        Fx-ScenarioGhWorktreeForced $script:FxPwsh
    }

    # ---- ancestry-merged work is still removed without a backup ----
    It 'must_pass_ancestry_merged_gone_branch_and_worktree_are_removed_without_any_backup' {
        $fx = Fx-NewGoneBranch 'g267-anc' 'done' -Merged
        $wt = Fx-NewWorktree $fx 'merged-wt'            # created at main's tip: an ancestor, nothing unique
        $r = Fx-RunMaint $fx $script:FxPs51 @('-Apply')
        Fx-AssertCompleted $r
        (Fx-BranchExists $fx.Repo 'done') | Should Be $false
        Test-Path -LiteralPath $wt | Should Be $false
        $r.Text | Should Match 'worktrees removed = 1'
        (Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*') | Should BeNullOrEmpty
        @(Get-ChildItem -LiteralPath $fx.Backup -Filter *.bundle -ErrorAction SilentlyContinue).Count | Should Be 0
    }

    # ---- the created tag is reported ----
    It 'must_fail_apply_output_names_the_created_gone_branch_backup_tag' {
        $fx = Fx-NewGoneBranch 'g267-print' 'wip'
        $r = Fx-RunMaint $fx $script:FxPs51 @('-Apply')
        Fx-AssertCompleted $r
        $tags = @(Fx-GoneTags $fx)
        $tags.Count | Should Be 1
        $r.Text | Should Match ([regex]::Escape($tags[0]))
    }
    It 'must_fail_dry_run_says_would_tag_the_backup_tag_name_and_creates_no_tag' {
        $fx = Fx-NewGoneBranch 'g267-would' 'wip'
        Fx-Git -C $fx.Repo fetch --prune --quiet           # a dry-run never fetches: make the branch [gone] up front
        $r = Fx-RunMaint $fx $script:FxPs51 @()
        Fx-AssertCompleted $r
        (Fx-BranchExists $fx.Repo 'wip') | Should Be $true
        (Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*') | Should BeNullOrEmpty
        $r.Text | Should Match "would tag\s+maint-backup/\S+/gone-wip-$($fx.Head.Substring(0, 9))"
    }

    # ---- r2 (review): a local branch can shadow the bare names used for the default-branch comparison ----
    It 'must_fail_gone_branch_named_origin_main_with_a_unique_commit_is_backed_up_before_deletion' {
        Fx-ScenarioOriginMainName $script:FxPs51 $true
    }
    It 'must_fail_pwsh7_gone_branch_named_origin_main_with_a_unique_commit_is_backed_up_before_deletion' -Skip:$script:noPwsh {
        Fx-ScenarioOriginMainName $script:FxPwsh $true
    }
    It 'must_fail_dry_run_gone_branch_named_origin_main_says_would_tag_or_warns_and_changes_nothing' {
        Fx-ScenarioOriginMainName $script:FxPs51 $false
    }
    It 'must_fail_gone_branch_named_like_the_default_branch_with_stale_origin_head_is_not_deleted_untagged' {
        Fx-ScenarioStaleOriginHead $script:FxPs51 $true
    }
    It 'must_fail_dry_run_gone_branch_named_like_the_default_branch_with_stale_origin_head_says_would_tag_or_warns' {
        Fx-ScenarioStaleOriginHead $script:FxPs51 $false
    }
    It 'must_pass_gone_branch_merged_into_origin_default_only_is_still_deleted_without_a_tag' {
        $fx = Fx-NewGoneBranch 'g267-origin-only' 'done'
        Fx-Git -C $fx.Repo push -q origin "$($fx.Head):refs/heads/main"   # origin/main now contains it; local main does not
        $r = Fx-RunMaint $fx $script:FxPs51 @('-Apply')
        Fx-AssertCompleted $r
        (Fx-BranchExists $fx.Repo 'done') | Should Be $false
        (Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*') | Should BeNullOrEmpty
    }

    # ---- header text guards ----
    It 'must_fail_header_step_2_no_longer_claims_worktrees_are_removed_because_their_upstream_is_gone' {
        $step2 = [regex]::Match((Fx-HeaderText), '(?s)2\.\s+PRUNE(.*?)3\.\s+BRANCHES').Groups[1].Value
        $step2 | Should Not BeNullOrEmpty
        $step2 | Should Not Match '(?i)upstream remote branch is gone'
    }
    It 'must_fail_header_fail_loud_line_describes_the_warn_and_continue_behaviour' {
        $safety = [regex]::Match((Fx-HeaderText), '(?s)SAFETY MODEL(.*?)\.PARAMETER').Groups[1].Value
        $safety | Should Not BeNullOrEmpty
        $safety | Should Not Match '(?i)any unexpected git/tool error stops the script'
        $safety | Should Match '(?i)\bwarn'
    }
}
