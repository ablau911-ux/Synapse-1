$ErrorActionPreference = 'Stop'

# #125 raid tests (Pester 3.4.0): drive the REAL repo-maintenance.ps1 entry point (-Apply) on
# a disposable git fixture under $TestDrive. A merged worktree that holds ignored data (.env)
# or uncommitted work must survive byte-for-byte; only ignored build output may go with it.
# GIT_CEILING_DIRECTORIES pins git discovery inside $TestDrive so no real repository is seen.
Describe 'repo-maintenance worktree removal preserves ignored and dirty data (#125)' {
    BeforeAll {
        $script:maint = Join-Path (Split-Path -Parent $PSScriptRoot) 'repo-maintenance.ps1'
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive

        function Invoke-Git {
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $null = & git @args 2>&1; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $old }
            if ($code -ne 0) { throw "git $args failed ($code)" }
        }
        function Test-GitInside([string]$Path) {
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $null = & git -C $Path rev-parse --is-inside-work-tree 2>&1; return ($LASTEXITCODE -eq 0) } finally { $ErrorActionPreference = $old }
        }
        function New-MaintFixture([string]$Case) {
            $root = Join-Path $TestDrive "$Case\root"
            $repo = Join-Path $root 'repo'
            New-Item -ItemType Directory -Force -Path $repo | Out-Null
            # Precondition: the scan root itself is not inside any repository.
            Test-GitInside $root | Should Be $false
            Invoke-Git -C $repo init -q -b main
            Invoke-Git -C $repo config user.email t@example.invalid
            Invoke-Git -C $repo config user.name t
            [System.IO.File]::WriteAllText((Join-Path $repo '.gitignore'), ".env`n/target/`n")
            [System.IO.File]::WriteAllText((Join-Path $repo 'a.txt'), 'a')
            Invoke-Git -C $repo add -A
            Invoke-Git -C $repo commit -q -m init
            return @{ Root = $root; Repo = $repo; Backup = (Join-Path $TestDrive "$Case\backups"); Wts = (Join-Path $TestDrive "$Case\wts") }
        }
        function New-MergedWorktree($Fixture, [string]$Name) {
            $wt = Join-Path $Fixture.Wts $Name
            Invoke-Git -C $Fixture.Repo worktree add -q -b "br-$Name" $wt
            return $wt
        }
        function Get-WorktreeList($Fixture) {
            return ((& git -C $Fixture.Repo worktree list --porcelain) -join "`n") -replace '\\', '/'
        }
        function Invoke-Maint($Fixture, [switch]$Force) {
            $args2 = @{ Root = $Fixture.Root; Apply = $true; BackupDir = $Fixture.Backup; MinFreeGB = 0 }
            if ($Force) { $args2.Force = $true }
            return (& $script:maint @args2 *>&1 | Out-String)
        }
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_fail keeps a merged worktree holding an ignored .env and its exact bytes' {
        $fx = New-MaintFixture 'env'
        $wt = New-MergedWorktree $fx 'with-env'
        [System.IO.File]::WriteAllText((Join-Path $wt '.env'), 'SECRET=keep-me')
        (Get-WorktreeList $fx) | Should Match ([regex]::Escape(($wt -replace '\\', '/')))

        $out = Invoke-Maint $fx

        $out | Should Match 'keep \(IGNORED files'
        [System.IO.File]::ReadAllText((Join-Path $wt '.env')) | Should Be 'SECRET=keep-me'
        (Get-WorktreeList $fx) | Should Match ([regex]::Escape(($wt -replace '\\', '/')))
    }

    It 'must_fail -Force does not remove a merged worktree holding ignored data' {
        $fx = New-MaintFixture 'force'
        $wt = New-MergedWorktree $fx 'forced-env'
        [System.IO.File]::WriteAllText((Join-Path $wt '.env'), 'SECRET=forced')

        $out = Invoke-Maint $fx -Force

        $out | Should Match 'keep \(IGNORED files'
        [System.IO.File]::ReadAllText((Join-Path $wt '.env')) | Should Be 'SECRET=forced'
    }

    It 'must_fail -Force does not remove a worktree with an uncommitted change' {
        $fx = New-MaintFixture 'dirty'
        $wt = New-MergedWorktree $fx 'dirty'
        [System.IO.File]::WriteAllText((Join-Path $wt 'a.txt'), 'edited-not-committed')

        $out = Invoke-Maint $fx -Force

        $out | Should Match 'keep \(DIRTY'
        [System.IO.File]::ReadAllText((Join-Path $wt 'a.txt')) | Should Be 'edited-not-committed'
    }

    It 'must_pass removes a merged worktree whose only ignored content is build output' {
        $fx = New-MaintFixture 'build'
        $wt = New-MergedWorktree $fx 'build-only'
        New-Item -ItemType Directory -Force -Path (Join-Path $wt 'target\debug') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $wt 'target\debug\x.bin'), 'x')

        $out = Invoke-Maint $fx

        $out | Should Match 'worktrees removed = 1'
        Test-Path -LiteralPath $wt | Should Be $false
        (Get-WorktreeList $fx) | Should Not Match ([regex]::Escape(($wt -replace '\\', '/')))
        [System.IO.File]::ReadAllText((Join-Path $fx.Repo 'a.txt')) | Should Be 'a'
    }
}
