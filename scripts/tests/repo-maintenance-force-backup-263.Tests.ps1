$ErrorActionPreference = 'Stop'

# #263 (Pester 3.4.0, child processes: Windows PowerShell 5.1, plus pwsh 7 when installed).
# Added 2026-10-16 19:10 +03:00.
# Bug: repo-maintenance.ps1 -Force backs up unmerged work with `git bundle create <file> <raw sha> --not ...`.
# A bundle records REFS; a bare sha names none, so git stops with "Refusing to create empty bundle"
# and the -Force removal can never succeed. The tag `maint-backup/<date>/<name>` is created right before
# and is the ref the bundle must be built from.
# The tests assert outcomes only (files, tags, worktree list, exit code, the script's own Die message),
# never how git stderr is suppressed, so they stay valid whatever the #262 fix does.
Describe 'repo-maintenance -Force backup bundle (#263)' {
    BeforeAll {
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')

        # Clean worktree on branch br-unmerged with one commit that is on neither main nor origin.
        function Fx-NewUnmergedWorktree([string]$Case) {
            $fx = Fx-NewRepo $Case -WithOrigin
            $fx.Wt = Fx-NewWorktree $fx 'unmerged'
            Fx-Commit $fx.Wt 'unique.txt'
            $fx.Head = Fx-GitOut -C $fx.Wt rev-parse HEAD
            return $fx
        }
        function Fx-ScenarioBackupSucceeds([string]$Exe) {
            $fx = Fx-NewUnmergedWorktree "bk-ok-$(Fx-ExeTag $Exe)"
            $r = Fx-RunMaint $fx $Exe @('-Apply', '-Force')
            Fx-AssertCompleted $r
            $bundles = @(Get-ChildItem -LiteralPath $fx.Backup -Filter *.bundle)
            $bundles.Count | Should Be 1
            (Fx-GitOut -C $fx.Repo bundle verify $bundles[0].FullName) | Should Match 'okay'
            (Fx-GitOut -C $fx.Repo bundle list-heads $bundles[0].FullName) | Should Match $fx.Head
            $tag = Fx-GitOut -C $fx.Repo tag --list 'maint-backup/*'
            $tag | Should Match '^maint-backup/\d{4}-\d{2}-\d{2}/'
            (Fx-GitOut -C $fx.Repo rev-parse "${tag}^{commit}") | Should Be $fx.Head
            Test-Path -LiteralPath $fx.Wt | Should Be $false
            (Fx-GitOut -C $fx.Repo worktree list --porcelain) | Should Not Match 'unmerged'
            $r.Text | Should Match 'worktrees removed = 1'
        }
        # A regular file sits where the backup directory should be, so no bundle can be written.
        function Fx-ScenarioBackupFails([string]$Exe) {
            $fx = Fx-NewUnmergedWorktree "bk-fail-$(Fx-ExeTag $Exe)"
            [System.IO.File]::WriteAllText($fx.Backup, 'a file where the backup directory should be')
            $r = Fx-RunMaint $fx $Exe @('-Apply', '-Force')
            $r.Code | Should Not Be 0
            $r.Text | Should Match 'Refusing to remove unmerged worktree'
            Test-Path -LiteralPath $fx.Wt | Should Be $true
            (Fx-GitOut -C $fx.Repo worktree list --porcelain) | Should Match 'unmerged'
            (Fx-GitOut -C $fx.Wt rev-parse HEAD) | Should Be $fx.Head
        }
        # Round 2 (T3): a leftover bundle from an earlier run (same leaf, same head, same day => same name under
        # any per-head naming) must not mask a FAILED create. Run 1 makes the real backup and removes the
        # worktree; its bundle is then corrupted, the worktree is recreated at the same head, origin is removed so
        # `--not origin/main` is a bad revision, and run 2 must still refuse before removal.
        function Fx-ScenarioStaleBundleCannotMask([string]$Exe) {
            $fx = Fx-NewUnmergedWorktree "bk-stale-$(Fx-ExeTag $Exe)"
            Fx-AssertCompleted (Fx-RunMaint $fx $Exe @('-Apply', '-Force'))
            $bundles = @(Get-ChildItem -LiteralPath $fx.Backup -Filter *.bundle)
            $bundles.Count | Should Be 1
            [System.IO.File]::WriteAllText($bundles[0].FullName, 'stale leftover, not a bundle')
            Fx-Git -C $fx.Repo worktree add -q $fx.Wt br-unmerged
            Fx-Git -C $fx.Repo remote remove origin
            $r = Fx-RunMaint $fx $Exe @('-Apply', '-Force')
            $r.Code | Should Not Be 0
            $r.Text | Should Match 'FATAL: .*Refusing to remove unmerged worktree'
            Test-Path -LiteralPath $fx.Wt | Should Be $true
            (Fx-GitOut -C $fx.Wt rev-parse HEAD) | Should Be $fx.Head
            (Fx-GitOut -C $fx.Repo worktree list --porcelain) | Should Match 'unmerged'
        }
        $script:noPwsh = [bool](-not $script:FxPwsh)
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_pass_force_backup_bundles_a_head_ref_verifies_tags_and_then_removes_the_worktree' {
        Fx-ScenarioBackupSucceeds $script:FxPs51
    }
    It 'must_pass_pwsh7_force_backup_bundles_a_head_ref_verifies_tags_and_then_removes_the_worktree' -Skip:$script:noPwsh {
        Fx-ScenarioBackupSucceeds $script:FxPwsh
    }
    It 'must_fail_force_backup_failure_stops_before_removal_with_the_scripts_own_message' {
        Fx-ScenarioBackupFails $script:FxPs51
    }
    It 'must_fail_pwsh7_force_backup_failure_stops_before_removal_with_the_scripts_own_message' -Skip:$script:noPwsh {
        Fx-ScenarioBackupFails $script:FxPwsh
    }
    It 'must_fail_stale_same_day_bundle_cannot_mask_a_failed_create_worktree_is_kept' {
        Fx-ScenarioStaleBundleCannotMask $script:FxPs51
    }
}
