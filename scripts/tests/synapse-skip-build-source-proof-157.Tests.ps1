$ErrorActionPreference = 'Stop'

# #157 manager contract; Pester 3.4; all fixture writes stay in TestDrive.
# Required seams: Write-SynapseBuildSourceFingerprint / Assert-SynapseSkipBuildSourceFingerprint
# Both take -SourceDir <string> -CandidatePath <string>. Writer persists SHA-256 hex
# in <CandidatePath>.source-fingerprint; validator refuses with SOURCE_UNVERIFIED FATAL.
# NEEDS-SEAM: implementations/wiring belong to fixer; no substitute logic here.
Describe 'Synapse SkipBuild source fingerprint (#157)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast157 = [System.Management.Automation.Language.Parser]::ParseFile(
            $setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        # Load definitions only. Never run full setup's host mutations.
        foreach ($statement in $script:ast157.EndBlock.Statements) {
            if ($statement -is [System.Management.Automation.Language.FunctionDefinitionAst]) {
                Invoke-Expression $statement.Extent.Text
            }
        }
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }
        function Invoke-FixtureGit157([string]$Repo, [string[]]$GitArguments) {
            $output = @(& git -C $Repo @GitArguments 2>&1)
            if ($LASTEXITCODE -ne 0) { throw ($output -join "`n") }
            return $output
        }
        function New-Fixture157([string]$Name, [bool]$Dirty = $false) {
            $root = Join-Path (Resolve-Path $TestDrive).ProviderPath $Name
            $repo = Join-Path $root 'source'
            $bin = Join-Path $root 'release'
            $installed = Join-Path $root 'installed'
            New-Item -ItemType Directory -Path $repo, $bin, $installed -Force | Out-Null
            $tracked = Join-Path $repo 'tracked.txt'
            [IO.File]::WriteAllText($tracked, 'committed source')
            $null = Invoke-FixtureGit157 $repo @('init', '-q')
            $null = Invoke-FixtureGit157 $repo @('add', '--', 'tracked.txt')
            $null = Invoke-FixtureGit157 $repo @('-c', 'user.name=fixture', '-c',
                'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false',
                'commit', '-q', '-m', 'synthetic source')
            if ($Dirty) { [IO.File]::WriteAllText($tracked, 'dirty source at build') }
            $candidate = Join-Path $bin 'synapse-mcp.exe'
            $dest = Join-Path $installed 'synapse-mcp.exe'
            [IO.File]::WriteAllText($candidate, 'candidate bytes')
            [IO.File]::WriteAllText($dest, 'installed sentinel')
            $identity = Get-SynapseSkipBuildHeadIdentity -SourceDir $repo
            $identity.Dirty | Should Be $Dirty
            $expected = if ($Dirty) { 'dirty source at build' } else { 'committed source' }
            [IO.File]::ReadAllText($tracked) | Should Be $expected
            return [pscustomobject]@{ Repo = $repo; Tracked = $tracked; Candidate = $candidate
                Dest = $dest; Installed = $installed; Identity = $identity
                Stamp = "$candidate.source-fingerprint" }
        }
        function Write-Proof157($Fixture) {
            $null = Write-SynapseBuildSourceFingerprint -SourceDir $Fixture.Repo -CandidatePath $Fixture.Candidate
            Test-Path -LiteralPath $Fixture.Stamp -PathType Leaf | Should Be $true
            (Get-Content -Raw -LiteralPath $Fixture.Stamp).Trim() | Should Match '^[0-9a-fA-F]{64}$'
        }
        function Get-ProductionGuard157 {
            return $script:ast157.EndBlock.Statements | Where-Object {
                $_ -is [System.Management.Automation.Language.IfStatementAst] -and
                $_.Extent.Text -match 'Assert-SynapseSkipBuildRevision -CandidateBuild'
            } | Select-Object -First 1
        }
        function Invoke-GuardAndInstall157($Fixture) {
            # Execute actual production SkipBuild guard, followed by real install helper.
            $guard = Get-ProductionGuard157
            if ($null -eq $guard) { throw 'production SkipBuild revision guard not found' }
            $SkipBuild = $true
            $PostExitUseInstalledBinary = $false
            $skipSource = $Fixture.Repo
            $SourceDir = $Fixture.Repo
            $installSourcePath = $Fixture.Candidate
            $headIdentity = Get-SynapseSkipBuildHeadIdentity -SourceDir $Fixture.Repo
            $build = $Fixture.Identity.Revision
            if ($Fixture.Identity.Dirty) { $build += '-dirty' }
            $candidatePreflight = [pscustomobject]@{ Build = $build }
            Invoke-Expression $guard.Extent.Text
            $null = Install-SynapseBinaryOverHeldImage -Source $Fixture.Candidate -Destination $Fixture.Dest
        }
        function Assert-Refused157($Fixture) {
            [IO.File]::ReadAllText($Fixture.Dest) | Should Be 'installed sentinel'
            [IO.File]::ReadAllText($Fixture.Candidate) | Should Be 'candidate bytes'
            $message = $null
            try { Invoke-GuardAndInstall157 $Fixture } catch { $message = $_.Exception.Message }
            $message | Should Match '^SYNAPSE_SKIP_BUILD_SOURCE_UNVERIFIED FATAL(?:\s|$)'
            [IO.File]::ReadAllText($Fixture.Dest) | Should Be 'installed sentinel'
            [IO.File]::ReadAllText($Fixture.Candidate) | Should Be 'candidate bytes'
            @(Get-ChildItem -LiteralPath $Fixture.Installed -Force).Count | Should Be 1
        }
    }

    It 'must_pass_matching_clean_fingerprint_installs_candidate' {
        $f = New-Fixture157 'clean'
        Write-Proof157 $f
        [IO.File]::ReadAllText($f.Dest) | Should Be 'installed sentinel'
        { Invoke-GuardAndInstall157 $f } | Should Not Throw
        [IO.File]::ReadAllText($f.Dest) | Should Be 'candidate bytes'
    }

    It 'must_pass_matching_dirty_fingerprint_installs_candidate' {
        $f = New-Fixture157 'dirty' $true
        Write-Proof157 $f
        [IO.File]::ReadAllText($f.Dest) | Should Be 'installed sentinel'
        { Invoke-GuardAndInstall157 $f } | Should Not Throw
        [IO.File]::ReadAllText($f.Dest) | Should Be 'candidate bytes'
        (Get-SynapseSkipBuildHeadIdentity -SourceDir $f.Repo).Dirty | Should Be $true
    }

    It 'must_fail_source_changed_after_clean_build_refuses_without_installing' {
        $f = New-Fixture157 'clean-then-changed'
        Write-Proof157 $f
        $proof = Get-Content -Raw -LiteralPath $f.Stamp
        [IO.File]::WriteAllText($f.Tracked, 'changed after build')
        [IO.File]::ReadAllText($f.Tracked) | Should Be 'changed after build'
        (Get-SynapseSkipBuildHeadIdentity -SourceDir $f.Repo).Revision | Should Be $f.Identity.Revision
        Assert-Refused157 $f
        (Get-Content -Raw -LiteralPath $f.Stamp) | Should Be $proof
    }

    It 'must_fail_different_dirty_bytes_at_same_HEAD_refuses_without_installing' {
        $f = New-Fixture157 'dirty-then-changed' $true
        Write-Proof157 $f
        [IO.File]::WriteAllText($f.Tracked, 'different dirty source')
        [IO.File]::ReadAllText($f.Tracked) | Should Be 'different dirty source'
        $identity = Get-SynapseSkipBuildHeadIdentity -SourceDir $f.Repo
        $identity.Dirty | Should Be $true
        $identity.Revision | Should Be $f.Identity.Revision
        Assert-Refused157 $f
    }

    It 'must_fail_missing_fingerprint_refuses_without_installing' {
        $f = New-Fixture157 'missing'
        Test-Path -LiteralPath $f.Stamp | Should Be $false
        Assert-Refused157 $f
        Test-Path -LiteralPath $f.Stamp | Should Be $false
    }

    It 'must_fail_malformed_fingerprint_refuses_without_installing' {
        $index = 0
        foreach ($bad in @('', 'not-a-fingerprint', ('a' * 63), ('g' * 64), (('a' * 64) + ' trailing'))) {
            $f = New-Fixture157 "malformed-$index"
            [IO.File]::WriteAllText($f.Stamp, $bad)
            [IO.File]::ReadAllText($f.Stamp) | Should Be $bad
            Assert-Refused157 $f
            [IO.File]::ReadAllText($f.Stamp) | Should Be $bad
            $index++
        }
    }

    It 'must_pass_setup_wires_build_proof_and_pre_handoff_source_guard' {
        $text = $script:ast157.Extent.Text
        $text | Should Match 'Write-SynapseBuildSourceFingerprint\s+-'
        $guard = Get-ProductionGuard157
        $guard.Extent.Text | Should Match 'Assert-SynapseSkipBuildSourceFingerprint\s+-'
        $install = $text.IndexOf('Install-SynapseBinaryOverHeldImage -Source $installSourcePath')
        ($install -gt $guard.Extent.EndOffset) | Should Be $true
    }
}
