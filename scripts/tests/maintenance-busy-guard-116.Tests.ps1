$ErrorActionPreference = 'Stop'

# #116 raid tests (Pester 3.4.0). Whole-directory removal of generated build trees must be
# opt-in, bounded to one repository, and refused while a Rust build process is running.
#  - repo-maintenance.ps1 is driven through its real entry point on a $TestDrive git fixture.
#  - cleanup-synapse-local-generated.ps1 (the sibling cleaner, Codex variant) must have the
#    same busy guard. NEEDS-SEAM: its targets are hardcoded; tests use the specified
#    -LocalDataRoot parameter so they never touch the real %LOCALAPPDATA%\synapse.
# Busy state is injected by mocking the process inventory (Get-CimInstance / Get-Process),
# so no real cargo/rustc process is needed and none on the host can flake the result.
Describe 'generated-artifact purge busy-process guard (#116)' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:maint = Join-Path $scriptsDir 'repo-maintenance.ps1'
        $script:localCleaner = Join-Path $scriptsDir 'cleanup-synapse-local-generated.ps1'
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive

        # Keep the real maintenance entry point, but never run a Git mutation.
        # Inventory exposes only the synthetic primary checkout, with no branches
        # or worktrees to prune. Any unrecognized invocation fails closed.
        function git {
            $a = @($args)
            $repo = (Get-Location).Path
            if ($a[0] -eq '-C') { $repo = $a[1]; $a = @($a | Select-Object -Skip 2) }
            $global:LASTEXITCODE = 0
            switch ($a -join ' ') {
                'rev-parse --is-inside-work-tree' { if (Test-Path (Join-Path $repo '.git')) { 'true' }; return }
                'rev-parse --show-toplevel' { $repo; return }
                'rev-parse --path-format=absolute --git-common-dir' { Join-Path $repo '.git'; return }
                'symbolic-ref --quiet refs/remotes/origin/HEAD' { 'refs/remotes/origin/main'; return }
                'fetch --prune --quiet' { return }
                'worktree list --porcelain' { return }
                'worktree prune' { return }
                'branch -vv' { return }
                # #264: gone-branch detection reads upstream track state, not branch -vv text.
                'for-each-ref --format=%(refname:lstrip=2)%09%(upstream:track)%09%(worktreepath) refs/heads' { return }
                default { throw "Unexpected Git invocation in #116 fixture: $a" }
            }
        }
        function New-Tree([string]$Path) {
            New-Item -ItemType Directory -Force -Path $Path | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $Path 'sentinel.bin'), 'sentinel')
            return $Path
        }
        function New-RepoWithTargets([string]$Case) {
            $root = Join-Path $TestDrive "$Case\root"
            $repo = Join-Path $root 'repo'
            New-Item -ItemType Directory -Force -Path $repo | Out-Null
            New-Item -ItemType Directory -Force -Path (Join-Path $repo '.git') | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $repo '.gitignore'), "/target/`n")
            [System.IO.File]::WriteAllText((Join-Path $repo 'a.txt'), 'a')
            New-Tree (Join-Path $repo 'target\debug') | Out-Null
            New-Tree (Join-Path $repo 'target\release') | Out-Null
            return @{ Root = $root; Repo = $repo; Backup = (Join-Path $TestDrive "$Case\backups") }
        }
        function Invoke-MaintPurge($Fx, [switch]$OptIn) {
            $p = @{ Root = $Fx.Root; Apply = $true; BackupDir = $Fx.Backup; MinFreeGB = 0; MaxTargetDebugGB = 0 }
            if ($OptIn) { $p.PurgeGeneratedArtifacts = $true; $p.ArtifactRepoPath = $Fx.Repo }
            return (& $script:maint @p *>&1 | Out-String)
        }
        function New-LocalDataRoot([string]$Case) {
            $root = Join-Path $TestDrive "$Case\localdata\synapse"
            New-Tree (Join-Path $root 'build-target\debug') | Out-Null
            New-Tree (Join-Path $root 'build-target\release') | Out-Null
            New-Tree (Join-Path $root 'codex-target-syn31-20260913\debug') | Out-Null
            return $root
        }
        # SAFETY INTERLOCK: a non-advanced script silently drops an unknown -LocalDataRoot into
        # $args and would then clean the REAL hardcoded targets. Never invoke the cleaner unless
        # it declares the seam and contains no absolute drive-path literal.
        function Assert-LocalCleanerSeam {
            $tokens = $null; $errors = $null
            $ast = [System.Management.Automation.Language.Parser]::ParseFile($script:localCleaner, [ref]$tokens, [ref]$errors)
            $errors.Count | Should Be 0
            $declared = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
            ($declared -contains 'LocalDataRoot') | Should Be $true
            $absolute = @($ast.FindAll({
                        param($n)
                        $n -is [System.Management.Automation.Language.StringConstantExpressionAst] -and $n.Value -match '^[A-Za-z]:\\'
                    }, $true))
            $absolute.Count | Should Be 0
        }
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_pass exact-repo opt-in with no build running removes target\debug and keeps target\release' {
        Mock Get-CimInstance { @() }
        $fx = New-RepoWithTargets 'optin'
        Test-Path -LiteralPath (Join-Path $fx.Repo 'target\debug\sentinel.bin') | Should Be $true

        $out = Invoke-MaintPurge $fx -OptIn

        $out | Should Match 'remove generated directory'
        Test-Path -LiteralPath (Join-Path $fx.Repo 'target\debug') | Should Be $false
        [System.IO.File]::ReadAllText((Join-Path $fx.Repo 'target\release\sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_fail exact-repo opt-in while cargo runs skips the busy tree and deletes nothing' {
        # Construct inside the mock: the production script has its own script scope.
        Mock Get-CimInstance { [pscustomobject]@{ Name = 'cargo.exe'; ProcessId = 4242; ExecutablePath = 'C:\fake\cargo.exe' } }
        $fx = New-RepoWithTargets 'busy'

        $out = Invoke-MaintPurge $fx -OptIn

        $out | Should Match 'skip busy path'
        [System.IO.File]::ReadAllText((Join-Path $fx.Repo 'target\debug\sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_fail scheduled-style -Apply without opt-in preserves the oversized tree' {
        Mock Get-CimInstance { @() }
        $fx = New-RepoWithTargets 'noopt'

        $out = Invoke-MaintPurge $fx

        $out | Should Match 'requires\s+-PurgeGeneratedArtifacts'
        [System.IO.File]::ReadAllText((Join-Path $fx.Repo 'target\debug\sentinel.bin')) | Should Be 'sentinel'
        # Cross the opt-in boundary on the same scheduled fixture while cargo is
        # busy: opting in must not bypass the second, independent deletion guard.
        Mock Get-CimInstance { [pscustomobject]@{ Name = 'cargo.exe'; ProcessId = 4242; ExecutablePath = 'C:\fake\cargo.exe' } }
        $out = Invoke-MaintPurge $fx -OptIn
        ('#116 scheduled fixture busy opt-in: ' + $out) | Should Match '#116 scheduled fixture busy opt-in: (?s).*skip busy path'
        Test-Path -LiteralPath (Join-Path $fx.Repo 'target\debug\sentinel.bin') | Should Be $true
        [System.IO.File]::ReadAllText((Join-Path $fx.Repo 'target\release\sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_pass local-generated cleaner removes both debug trees when no build runs' {
        Mock Get-CimInstance { @() }
        Mock Get-Process { @() }
        Assert-LocalCleanerSeam
        $root = New-LocalDataRoot 'localok'

        $json = & $script:localCleaner -Apply -LocalDataRoot $root | Out-String
        $result = $json | ConvertFrom-Json

        $result.Apply | Should Be $true
        @($result.Deleted).Count | Should Be 2
        Test-Path -LiteralPath (Join-Path $root 'build-target\debug') | Should Be $false
        Test-Path -LiteralPath (Join-Path $root 'codex-target-syn31-20260913\debug') | Should Be $false
        [System.IO.File]::ReadAllText((Join-Path $root 'build-target\release\sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_fail local-generated cleaner refuses while cargo runs and deletes nothing' {
        Mock Get-CimInstance { [pscustomobject]@{ Name = 'cargo.exe'; ProcessId = 4242; ExecutablePath = 'C:\fake\cargo.exe' } }
        Mock Get-Process { [pscustomobject]@{ ProcessName = 'cargo'; Name = 'cargo'; Id = 4242 } }
        Assert-LocalCleanerSeam
        $root = New-LocalDataRoot 'localbusy'

        $message = $null
        try { $null = & $script:localCleaner -Apply -LocalDataRoot $root } catch { $message = $_.Exception.Message }

        $message | Should Match 'Refusing cleanup while process is running: cargo'
        [System.IO.File]::ReadAllText((Join-Path $root 'build-target\debug\sentinel.bin')) | Should Be 'sentinel'
        [System.IO.File]::ReadAllText((Join-Path $root 'codex-target-syn31-20260913\debug\sentinel.bin')) | Should Be 'sentinel'
    }
}
