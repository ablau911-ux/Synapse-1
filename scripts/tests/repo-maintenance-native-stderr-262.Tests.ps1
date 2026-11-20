$ErrorActionPreference = 'Stop'

# #262 (Pester 3.4.0, run under Windows PowerShell 5.1: powershell.exe, NOT pwsh).
# Added 2026-10-16 18:50 +03:00.
# Root cause: repo-maintenance.ps1 sets $ErrorActionPreference='Stop'. On PS 5.1 a native
# command whose stderr is redirected (2>$null, 2>&1) turns that stderr into an ErrorRecord, and
# Stop makes it terminating: a FAILING git call throws git's raw text ("fatal: ...") before the
# script's own $LASTEXITCODE handling runs. Every git failure the script handles by exit code
# or empty output must reach that handling instead.
#
# Function-level tests dot-load EVERY function definition of the production script (so any
# helper the fix adds is available) and run in this host. Script-level tests run the real
# script as a child process (Windows PowerShell 5.1, plus pwsh 7 parity when installed) on
# throwaway git fixtures under $TestDrive. Fixture helpers use the Fx prefix so they cannot
# collide with production names. Nothing here touches a real repository.
Describe 'repo-maintenance native stderr under Stop (#262)' {
    BeforeAll {
        $script:maint = Join-Path (Split-Path -Parent $PSScriptRoot) 'repo-maintenance.ps1'
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        $script:ps51 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $pwshCmd = Get-Command pwsh -ErrorAction SilentlyContinue
        $script:pwsh = if ($pwshCmd) { $pwshCmd.Source } else { $null }

        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($script:maint, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $fnAsts = $ast.FindAll({ param($n) $n.GetType().Name -eq 'FunctionDefinitionAst' }, $false)
        foreach ($fn in $fnAsts) { Invoke-Expression $fn.Extent.Text }
        foreach ($need in 'Default-Branch', 'Is-Merged', 'Get-WorktreeNonDisposableIgnored') {
            if (-not (Get-Command $need -CommandType Function -ErrorAction SilentlyContinue)) { throw "repo-maintenance function is missing: $need" }
        }
        $HaveGh = $false   # Is-Merged reads this script-level flag; no gh in fixtures.

        function Fx-GitOut {
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $out = & git @args 2>&1; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $old }
            if ($code -ne 0) { throw "fixture git $args failed ($code): $out" }
            return (($out | ForEach-Object { "$_" }) -join "`n").Trim()
        }
        function Fx-Git { $null = Fx-GitOut @args }
        function Fx-Commit([string]$Repo, [string]$File) {
            [System.IO.File]::WriteAllText((Join-Path $Repo $File), $File)
            Fx-Git -C $Repo add -A
            Fx-Git -C $Repo commit -q -m $File
        }
        function Fx-NewRepo([string]$Case, [switch]$WithOrigin) {
            $root = Join-Path $TestDrive "$Case\root"
            $repo = Join-Path $root 'repo'
            New-Item -ItemType Directory -Force -Path $repo | Out-Null
            Fx-Git -C $repo init -q -b main
            Fx-Git -C $repo config user.email t@example.invalid
            Fx-Git -C $repo config user.name t
            Fx-Git -C $repo config core.autocrlf false   # global autocrlf=true makes git warn on stderr
            Fx-Commit $repo 'init.txt'
            $fx = @{ Root = $root; Repo = $repo; Backup = (Join-Path $TestDrive "$Case\backups"); Wts = (Join-Path $TestDrive "$Case\wts"); Origin = $null }
            if ($WithOrigin) {
                $fx.Origin = Join-Path $TestDrive "$Case\origin.git"
                Fx-Git init -q --bare -b main $fx.Origin
                Fx-Git -C $repo remote add origin $fx.Origin
                Fx-Git -C $repo push -q origin main
            }
            return $fx
        }
        function Fx-NewWorktree($Fx, [string]$Name) {
            $wt = Join-Path $Fx.Wts $Name
            Fx-Git -C $Fx.Repo worktree add -q -b "br-$Name" $wt
            return $wt
        }
        function Fx-NonRepoDir([string]$Name) {
            $dir = Join-Path $TestDrive $Name
            New-Item -ItemType Directory -Force -Path $dir | Out-Null
            return $dir
        }
        function Fx-AssertNoRawGitText([string]$Text) {
            # Lower-case "fatal: " / "error: " are git's own text; the script's Die prints upper-case FATAL.
            ($Text -cmatch 'fatal: |error: |NativeCommandError') | Should Be $false
        }
        function Fx-RunMaint($Fx, [string]$Exe, [string[]]$Extra) {
            $psi = New-Object System.Diagnostics.ProcessStartInfo
            $psi.FileName = $Exe
            $psi.UseShellExecute = $false
            $psi.RedirectStandardOutput = $true
            $psi.RedirectStandardError = $true
            $psi.CreateNoWindow = $true
            $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$($script:maint)`"", '-Root', "`"$($Fx.Root)`"", '-BackupDir', "`"$($Fx.Backup)`"", '-MinFreeGB', '0') + $Extra
            $psi.Arguments = $argv -join ' '
            $proc = [System.Diagnostics.Process]::Start($psi)
            $so = $proc.StandardOutput.ReadToEndAsync()
            $se = $proc.StandardError.ReadToEndAsync()
            if (-not $proc.WaitForExit(240000)) { $proc.Kill(); throw 'repo-maintenance child run timed out' }
            $proc.WaitForExit()
            return [pscustomobject]@{ Code = $proc.ExitCode; Text = ($so.Result + "`n" + $se.Result) }
        }

        # ---- script-level scenarios: each builds a fixture, runs the real script, asserts the outcome ----
        function Fx-ScenarioStatusFailed([string]$Exe, [string]$Mode) {
            $fx = Fx-NewRepo "status-$Mode-$([IO.Path]::GetFileNameWithoutExtension($Exe))"
            $wt = Fx-NewWorktree $fx 'broken'
            if ($Mode -eq 'gitfile') {
                $gitFile = Join-Path $wt '.git'
                Remove-Item -LiteralPath $gitFile -Force   # the .git pointer file is hidden: delete it before rewriting
                [System.IO.File]::WriteAllText($gitFile, "gitdir: $TestDrive/no/such/gitdir`n")
            }
            else { Get-ChildItem -LiteralPath $wt -Force | Remove-Item -Recurse -Force }
            $r = Fx-RunMaint $fx $Exe @()
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Be 0
            $r.Text | Should Match 'keep \(status failed\)'
            Test-Path -LiteralPath $wt | Should Be $true
        }
        function Fx-ScenarioMissingWorktreeDir([string]$Exe) {
            $fx = Fx-NewRepo "missing-$([IO.Path]::GetFileNameWithoutExtension($Exe))"
            $wt = Fx-NewWorktree $fx 'gone-dir'
            Remove-Item -LiteralPath $wt -Recurse -Force
            $r = Fx-RunMaint $fx $Exe @()
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Be 0
            $r.Text | Should Match 'Found 1 primary repo'
        }
        function Fx-ScenarioUnbornHead([string]$Exe) {
            $fx = Fx-NewRepo "unborn-$([IO.Path]::GetFileNameWithoutExtension($Exe))"
            $wt = Join-Path $fx.Wts 'orphan'
            Fx-Git -C $fx.Repo worktree add -q --orphan -b br-orphan $wt   # list --porcelain reports HEAD 0000...
            $r = Fx-RunMaint $fx $Exe @()
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Be 0
            $r.Text | Should Match 'keep \(UNMERGED work, not on remote\)'
            Test-Path -LiteralPath $wt | Should Be $true
        }
        function Fx-ScenarioLockedRemove([string]$Exe) {
            $fx = Fx-NewRepo "locked-$([IO.Path]::GetFileNameWithoutExtension($Exe))"
            $wt = Fx-NewWorktree $fx 'locked'
            Fx-Git -C $fx.Repo worktree lock $wt
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Be 0
            $r.Text | Should Match 'git refused worktree removal'
            $r.Text | Should Match 'worktrees removed = 0'
            Test-Path -LiteralPath $wt | Should Be $true
        }
        function Fx-NewGoneBranchFixture([string]$Case, [string]$Branch, [bool]$CheckedOut) {
            $fx = Fx-NewRepo $Case -WithOrigin
            Fx-Git -C $fx.Repo branch $Branch
            Fx-Git -C $fx.Repo push -q -u origin $Branch
            if ($CheckedOut) { Fx-Git -C $fx.Repo checkout -q $Branch }
            Fx-Git -C $fx.Origin branch -D $Branch   # remote branch vanishes: fetch --prune marks the local one [gone]
            return $fx
        }
        function Fx-ScenarioGoneBranchCheckedOut([string]$Exe) {
            $fx = Fx-NewGoneBranchFixture "gone-co-$([IO.Path]::GetFileNameWithoutExtension($Exe))" 'feat' $true
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Be 0
            # Round 2 (T1/T2): git refuses `branch -D` on a checked-out branch by itself, so the branch surviving
            # proves nothing. The script's OWN keep message must be present, and it must never claim a delete.
            $r.Text | Should Not Match '\[APPLY\] delete gone branch: feat(\s|$)'
            @($r.Text -split '\r?\n' | Where-Object { $_ -match '\[maint\]\[WARN\].*\bfeat\b.*(checked out|worktree)' }).Count | Should Not Be 0
            $r.Text | Should Match 'free after'
            (Fx-GitOut -C $fx.Repo rev-parse --verify --quiet refs/heads/feat) | Should Match '^[0-9a-f]{40}$'
        }
        # Round 2 (T4): the fetch failure is WARNED about on both shells, then the run continues.
        function Fx-ScenarioFetchFails([string]$Exe) {
            $fx = Fx-NewRepo "fetch-fail-$([IO.Path]::GetFileNameWithoutExtension($Exe))"
            Fx-Git -C $fx.Repo remote add origin (Join-Path $TestDrive "fetch-fail-$([IO.Path]::GetFileNameWithoutExtension($Exe))\no-such-origin.git")
            $r = Fx-RunMaint $fx $Exe @('-Apply')
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Be 0
            $r.Text | Should Match '\[maint\]\[WARN\] fetch failed for'
            $r.Text | Should Match 'worktrees removed = 0'
        }
        # Fail-closed path of the -Force backup: no origin ref, so the backup bundle cannot be built and
        # the script must stop with its OWN "Refusing to remove unmerged worktree" message, keeping the worktree.
        function Fx-ScenarioForceBundleNoOrigin([string]$Exe) {
            $fx = Fx-NewRepo "bundle-no-origin-$([IO.Path]::GetFileNameWithoutExtension($Exe))"
            $wt = Fx-NewWorktree $fx 'unmerged'
            Fx-Commit $wt 'unique.txt'
            $r = Fx-RunMaint $fx $Exe @('-Apply', '-Force')
            Fx-AssertNoRawGitText $r.Text
            $r.Code | Should Not Be 0
            $r.Text | Should Match 'Refusing to remove unmerged worktree'
            Test-Path -LiteralPath $wt | Should Be $true
            @(Get-ChildItem -LiteralPath $fx.Backup -Filter *.bundle -ErrorAction SilentlyContinue).Count | Should Be 0
        }

        # ---- source scanner (see the rule above the scan tests) ----
        function Fx-HasStderrRedirect($Command) {
            foreach ($r in $Command.Redirections) {
                $kind = $r.GetType().Name
                if (($kind -in 'FileRedirectionAst', 'MergingRedirectionAst') -and (([string]$r.FromStream) -in 'Error', 'All')) { return $true }
            }
            return $false
        }
        function Fx-IsProtectedGitCall($Call) {
            for ($p = $Call.Parent; $null -ne $p; $p = $p.Parent) {
                $kind = $p.GetType().Name
                if ($kind -eq 'TryStatementAst' -and $p.CatchClauses.Count -gt 0 -and
                    $p.Body.Extent.StartOffset -le $Call.Extent.StartOffset -and $Call.Extent.EndOffset -le $p.Body.Extent.EndOffset) { return $true }
                if ($kind -eq 'ScriptBlockAst') {
                    $relax = $p.FindAll({ param($n)
                            $n.GetType().Name -eq 'AssignmentStatementAst' -and $n.Left.Extent.Text -eq '$ErrorActionPreference' -and
                            $n.Right.Extent.Text -match 'Continue' -and $n.Extent.EndOffset -le $Call.Extent.StartOffset }, $false)
                    if ($relax.Count -gt 0) { return $true }
                }
            }
            return $false
        }
        function Fx-FindUnprotectedGitCalls([string]$Source) {
            $tok = $null
            $err = $null
            $tree = [System.Management.Automation.Language.Parser]::ParseInput($Source, [ref]$tok, [ref]$err)
            if ($err.Count -gt 0) { throw ($err | Out-String) }
            $stop = @($tree.EndBlock.Statements | Where-Object {
                    $_.GetType().Name -eq 'AssignmentStatementAst' -and $_.Left.Extent.Text -eq '$ErrorActionPreference' -and $_.Right.Extent.Text -match 'Stop' })
            if ($stop.Count -eq 0) { return @() }   # script no longer runs under Stop: nothing to guard
            $calls = $tree.FindAll({ param($n) $n.GetType().Name -eq 'CommandAst' -and $n.GetCommandName() -eq 'git' -and (Fx-HasStderrRedirect $n) }, $true)
            $bad = foreach ($c in $calls) { if (-not (Fx-IsProtectedGitCall $c)) { "line $($c.Extent.StartLineNumber): $($c.Extent.Text)" } }
            return @($bad)
        }
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    # ---- Get-WorktreeNonDisposableIgnored ---------------------------------------------------
    It 'must_pass_get_ignored_returns_nothing_for_a_clean_worktree' {
        $fx = Fx-NewRepo 'fn-ignored-clean'
        @(Get-WorktreeNonDisposableIgnored -Path $fx.Repo -DisposablePatterns @('target')).Count | Should Be 0
    }
    It 'must_fail_get_ignored_non_repo_dir_throws_own_message_not_git_text' {
        $dir = Fx-NonRepoDir 'fn-ignored-not-a-repo'
        $thrown = $null
        try { $null = @(Get-WorktreeNonDisposableIgnored -Path $dir -DisposablePatterns @('target')) } catch { $thrown = $_.Exception.Message }
        $thrown | Should Match 'git status --ignored failed for'
        Fx-AssertNoRawGitText $thrown
    }
    It 'must_fail_get_ignored_deleted_worktree_path_throws_own_message_not_git_text' {
        $gone = Join-Path $TestDrive 'fn-ignored-deleted-path'
        $thrown = $null
        try { $null = @(Get-WorktreeNonDisposableIgnored -Path $gone -DisposablePatterns @('target')) } catch { $thrown = $_.Exception.Message }
        $thrown | Should Match 'git status --ignored failed for'
        Fx-AssertNoRawGitText $thrown
    }

    # ---- Default-Branch ---------------------------------------------------------------------
    It 'must_pass_default_branch_prefers_origin_head' {
        $fx = Fx-NewRepo 'fn-def-originhead'
        Fx-Git -C $fx.Repo symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/trunk
        Default-Branch $fx.Repo | Should Be 'trunk'
    }
    It 'must_pass_default_branch_falls_back_to_master_when_main_is_absent' {
        $fx = Fx-NewRepo 'fn-def-master'
        Fx-Git -C $fx.Repo branch -m main master
        Default-Branch $fx.Repo | Should Be 'master'
    }
    It 'must_pass_default_branch_returns_main_when_no_known_branch_exists' {
        $fx = Fx-NewRepo 'fn-def-trunk'
        Fx-Git -C $fx.Repo branch -m main trunk
        Default-Branch $fx.Repo | Should Be 'main'
    }
    It 'must_fail_default_branch_non_repo_dir_returns_main_without_raw_throw' {
        $dir = Fx-NonRepoDir 'fn-def-not-a-repo'
        Default-Branch $dir | Should Be 'main'
    }
    It 'must_fail_default_branch_deleted_path_returns_main_without_raw_throw' {
        Default-Branch (Join-Path $TestDrive 'fn-def-deleted-path') | Should Be 'main'
    }

    # ---- Is-Merged --------------------------------------------------------------------------
    It 'must_pass_is_merged_true_when_sha_is_ancestor_of_default' {
        $fx = Fx-NewRepo 'fn-merged-anc'
        $sha = Fx-GitOut -C $fx.Repo rev-parse HEAD
        Fx-Commit $fx.Repo 'later.txt'
        Is-Merged $fx.Repo $sha 'br' 'main' | Should Be $true
    }
    It 'must_pass_is_merged_true_when_sha_is_ancestor_of_origin_default_only' {
        $fx = Fx-NewRepo 'fn-merged-origin'
        Fx-Git -C $fx.Repo checkout -q -b br
        Fx-Commit $fx.Repo 'feature.txt'
        $sha = Fx-GitOut -C $fx.Repo rev-parse HEAD
        Fx-Git -C $fx.Repo checkout -q main
        Fx-Git -C $fx.Repo update-ref refs/remotes/origin/main $sha   # origin/main contains it, local main does not
        Is-Merged $fx.Repo $sha 'br' 'main' | Should Be $true
    }
    It 'must_fail_is_merged_false_for_unmerged_branch_when_origin_ref_is_missing' {
        $fx = Fx-NewRepo 'fn-merged-noorigin'
        Fx-Git -C $fx.Repo checkout -q -b br
        Fx-Commit $fx.Repo 'feature.txt'
        $sha = Fx-GitOut -C $fx.Repo rev-parse HEAD
        Fx-Git -C $fx.Repo checkout -q main
        Is-Merged $fx.Repo $sha 'br' 'main' | Should Be $false
    }
    It 'must_fail_is_merged_false_for_bogus_sha' {
        $fx = Fx-NewRepo 'fn-merged-bogus'
        Is-Merged $fx.Repo 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeef' 'br' 'main' | Should Be $false
    }
    It 'must_fail_is_merged_false_for_missing_default_branch_ref' {
        $fx = Fx-NewRepo 'fn-merged-nodef'
        $sha = Fx-GitOut -C $fx.Repo rev-parse HEAD
        Is-Merged $fx.Repo $sha 'br' 'no-such-default' | Should Be $false
    }

    # ---- script top level (run the real script as a child process) --------------------------
    It 'must_fail_script_worktree_with_broken_gitfile_is_kept_as_status_failed' {
        Fx-ScenarioStatusFailed $script:ps51 'gitfile'
    }
    It 'must_fail_script_worktree_dir_emptied_is_kept_as_status_failed' {
        Fx-ScenarioStatusFailed $script:ps51 'emptied'
    }
    It 'must_pass_script_worktree_dir_deleted_is_skipped_silently' {
        Fx-ScenarioMissingWorktreeDir $script:ps51
    }
    It 'must_fail_script_unborn_head_worktree_is_kept_as_unmerged_not_crashed' {
        Fx-ScenarioUnbornHead $script:ps51
    }
    It 'must_fail_script_locked_worktree_removal_refusal_is_warned_not_thrown' {
        Fx-ScenarioLockedRemove $script:ps51
    }
    It 'must_fail_script_gone_branch_checked_out_is_left_alone_not_thrown' {
        Fx-ScenarioGoneBranchCheckedOut $script:ps51
    }
    It 'must_fail_script_force_without_origin_refuses_with_own_backup_message' {
        Fx-ScenarioForceBundleNoOrigin $script:ps51
    }
    It 'must_pass_script_gone_branch_not_checked_out_is_deleted' {
        $fx = Fx-NewGoneBranchFixture 'gone-del' 'old' $false
        $r = Fx-RunMaint $fx $script:ps51 @('-Apply')
        Fx-AssertNoRawGitText $r.Text
        $r.Code | Should Be 0
        $r.Text | Should Match 'delete gone branch: old'
        (Fx-GitOut -C $fx.Repo branch --list old) | Should BeNullOrEmpty
    }
    It 'must_pass_script_fetch_failure_against_unreachable_origin_warns_and_continues' {
        Fx-ScenarioFetchFails $script:ps51
    }
    It 'must_pass_script_scan_root_with_non_repo_and_broken_gitfile_children_still_finds_repo' {
        $fx = Fx-NewRepo 'scan-root'
        New-Item -ItemType Directory -Force -Path (Join-Path $fx.Root 'plain') | Out-Null
        New-Item -ItemType Directory -Force -Path (Join-Path $fx.Root 'junk') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $fx.Root 'junk\.git'), 'not a gitdir pointer')
        $r = Fx-RunMaint $fx $script:ps51 @()
        Fx-AssertNoRawGitText $r.Text
        $r.Code | Should Be 0
        $r.Text | Should Match 'Found 1 primary repo'
    }

    # ---- pwsh 7 parity: same observable outcome as before (skipped when pwsh is not installed) ----
    $noPwsh = [bool](-not (Get-Command pwsh -ErrorAction SilentlyContinue))
    It 'must_pass_pwsh7_broken_gitfile_worktree_is_kept_as_status_failed' -Skip:$noPwsh {
        Fx-ScenarioStatusFailed $script:pwsh 'gitfile'
    }
    It 'must_pass_pwsh7_unborn_head_worktree_is_kept_as_unmerged' -Skip:$noPwsh {
        Fx-ScenarioUnbornHead $script:pwsh
    }
    It 'must_pass_pwsh7_locked_worktree_removal_refusal_is_warned' -Skip:$noPwsh {
        Fx-ScenarioLockedRemove $script:pwsh
    }
    It 'must_pass_pwsh7_fetch_failure_against_unreachable_origin_warns_and_continues' -Skip:$noPwsh {
        Fx-ScenarioFetchFails $script:pwsh
    }
    It 'must_pass_pwsh7_gone_branch_checked_out_is_left_alone' -Skip:$noPwsh {
        Fx-ScenarioGoneBranchCheckedOut $script:pwsh
    }
    It 'must_pass_pwsh7_force_without_origin_refuses_with_own_backup_message' -Skip:$noPwsh {
        Fx-ScenarioForceBundleNoOrigin $script:pwsh
    }

    # ---- source-scan guard ------------------------------------------------------------------
    # Rule (generic, helper-name agnostic): while the script runs under $ErrorActionPreference='Stop',
    # a direct `git` invocation that redirects stderr (2>... / 2>&1 / *>...) must be protected by ONE of:
    #   a) an enclosing try that has a catch clause around the call, or
    #   b) a non-Stop $ErrorActionPreference assigned earlier in an enclosing function/script block.
    # A fix that routes every git call through one helper satisfies (b) inside that helper, or avoids a
    # direct redirected `git` command altogether; the behaviour tests above prove the helper is correct.
    It 'must_pass_scanner_flags_unprotected_and_accepts_protected_redirected_git_calls' {
        function Fx-Scan([string]$Body) { return @(Fx-FindUnprotectedGitCalls ("`$ErrorActionPreference = 'Stop'`n" + $Body)) }
        (Fx-Scan 'function F { git status 2>$null }').Count | Should Be 1
        (Fx-Scan 'function F { $x = (git -C $p log 2>&1 | Out-Null) }').Count | Should Be 1
        (Fx-Scan 'try { git fetch 2>$null } finally { }').Count | Should Be 1
        (Fx-Scan 'function F { $ErrorActionPreference = "Continue"; & git status 2>&1 }').Count | Should Be 0
        (Fx-Scan 'try { git fetch 2>$null } catch { }').Count | Should Be 0
        (Fx-Scan 'function F { git status }').Count | Should Be 0
    }
    It 'must_fail_source_scan_every_stderr_redirected_git_call_is_protected_under_stop' {
        $offenders = @(Fx-FindUnprotectedGitCalls ([System.IO.File]::ReadAllText($script:maint)))
        ($offenders -join ' | ') | Should BeNullOrEmpty
    }
}
