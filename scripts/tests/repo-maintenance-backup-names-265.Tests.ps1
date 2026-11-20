$ErrorActionPreference = 'Stop'

# #265 (Pester 3.4.0, child processes: Windows PowerShell 5.1, plus pwsh 7 when installed).
# Added 2026-10-16 19:40 +03:00.
# Bug: the -Force backup names the tag `maint-backup/<date>/<folder>` and the bundle `<repo>-<folder>-<date>.bundle`.
# A second forced removal of a worktree with the SAME folder name on the same day (another path, or the worktree
# recreated) overwrites the first tag (`tag -f`) and bundle, so the first worktree's unmerged commits lose their
# only ref. Expected: every backup has its own name (the tests do not dictate it), nothing is overwritten, and each
# bundle verifies. All assertions are outcome-based: bundle files, `git bundle verify`, tags and their commits.
Describe 'repo-maintenance -Force backups never collide (#265)' {
    BeforeAll {
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')

        # Clean unmerged worktree whose FOLDER is always named 'feat' (the collision key), under $Parent.
        function Fx-NewFeatWorktree($Fx, [string]$Parent, [string]$Branch, [string]$File) {
            $wt = Join-Path (Join-Path $Fx.Wts $Parent) 'feat'
            Fx-Git -C $Fx.Repo worktree add -q -b $Branch $wt
            Fx-Commit $wt $File
            return @{ Path = $wt; Head = (Fx-GitOut -C $wt rev-parse HEAD) }
        }
        function Fx-HeadsInBundles($Fx) {
            $heads = @()
            foreach ($b in @(Get-ChildItem -LiteralPath $Fx.Backup -Filter *.bundle)) {
                (Fx-GitOut -C $Fx.Repo bundle verify $b.FullName) | Should Match 'okay'
                $heads += @((Fx-GitOut -C $Fx.Repo bundle list-heads $b.FullName) -split '\r?\n' | Where-Object { $_ } | ForEach-Object { ($_ -split ' ')[0] })
            }
            return $heads
        }
        function Fx-AssertEveryHeadBackedUp($Fx, [string[]]$Heads) {
            @(Get-ChildItem -LiteralPath $Fx.Backup -Filter *.bundle).Count | Should Be $Heads.Count
            $inBundles = @(Fx-HeadsInBundles $Fx)
            foreach ($h in $Heads) { ($inBundles -contains $h) | Should Be $true }
            $tags = @((Fx-GitOut -C $Fx.Repo tag --list 'maint-backup/*') -split '\r?\n' | Where-Object { $_ })
            $tags.Count | Should Be $Heads.Count
            $tagHeads = @($tags | ForEach-Object { Fx-GitOut -C $Fx.Repo rev-parse "${_}^{commit}" })
            foreach ($h in $Heads) { ($tagHeads -contains $h) | Should Be $true }
        }
        # Two same-named worktrees forced in ONE run.
        function Fx-ScenarioSameRun([string]$Exe) {
            $fx = Fx-NewRepo "names-run-$(Fx-ExeTag $Exe)" -WithOrigin
            $a = Fx-NewFeatWorktree $fx 'a' 'br-a' 'a.txt'
            $b = Fx-NewFeatWorktree $fx 'b' 'br-b' 'b.txt'
            $r = Fx-RunMaint $fx $Exe @('-Apply', '-Force')
            Fx-AssertCompleted $r
            Test-Path -LiteralPath $a.Path | Should Be $false
            Test-Path -LiteralPath $b.Path | Should Be $false
            $r.Text | Should Match 'worktrees removed = 2'
            Fx-AssertEveryHeadBackedUp $fx @($a.Head, $b.Head)
        }
        # The worktree is recreated at the same path later the same day: run 1's backup must survive run 2 untouched.
        function Fx-ScenarioRecreated([string]$Exe) {
            $fx = Fx-NewRepo "names-recreate-$(Fx-ExeTag $Exe)" -WithOrigin
            $a = Fx-NewFeatWorktree $fx 'a' 'br-a' 'a.txt'
            Fx-AssertCompleted (Fx-RunMaint $fx $Exe @('-Apply', '-Force'))
            $firstFile = @(Get-ChildItem -LiteralPath $fx.Backup -Filter *.bundle)[0]
            $firstHash = (Get-FileHash -LiteralPath $firstFile.FullName).Hash
            $b = Fx-NewFeatWorktree $fx 'a' 'br-b' 'b.txt'   # same path, same folder name, new commits
            Fx-AssertCompleted (Fx-RunMaint $fx $Exe @('-Apply', '-Force'))
            Test-Path -LiteralPath $b.Path | Should Be $false
            Test-Path -LiteralPath $firstFile.FullName | Should Be $true
            (Get-FileHash -LiteralPath $firstFile.FullName).Hash | Should Be $firstHash
            Fx-AssertEveryHeadBackedUp $fx @($a.Head, $b.Head)
        }
        $script:noPwsh = [bool](-not $script:FxPwsh)
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_fail_two_same_named_worktrees_forced_in_one_run_both_keep_a_verifiable_backup' {
        Fx-ScenarioSameRun $script:FxPs51
    }
    It 'must_fail_pwsh7_two_same_named_worktrees_forced_in_one_run_both_keep_a_verifiable_backup' -Skip:$script:noPwsh {
        Fx-ScenarioSameRun $script:FxPwsh
    }
    It 'must_fail_recreated_same_named_worktree_forced_same_day_never_overwrites_the_first_backup' {
        Fx-ScenarioRecreated $script:FxPs51
    }
}
