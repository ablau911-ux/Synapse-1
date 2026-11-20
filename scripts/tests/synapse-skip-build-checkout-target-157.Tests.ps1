$ErrorActionPreference = 'Stop'

Describe 'Synapse setup issue 157: SkipBuild per-checkout target and install backoff' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @(
            'Get-SynapseCheckoutCargoTarget',
            'Get-SynapseRetryDelayMs',
            'Get-SynapseCargoReleaseArtifactPath',
            'Resolve-SynapseSkipBuildCandidatePath',
            'Get-SynapseSkipBuildHeadIdentity',
            'Assert-SynapseSkipBuildRevision'
        )) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "setup function is missing: $name" }
            Invoke-Expression $functionAst.Extent.Text
        }
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }
    }

    It 'derives the per-checkout target from root and source path, case-insensitively' {
        $src = Join-Path $TestDrive 'MyCheckout'
        New-Item -ItemType Directory -Force -Path $src | Out-Null
        $a = Get-SynapseCheckoutCargoTarget -Root 'C:\bt' -SourceDir $src
        $b = Get-SynapseCheckoutCargoTarget -Root 'C:\bt' -SourceDir $src.ToUpperInvariant()
        (Split-Path -Leaf $a) | Should Match '^MyCheckout-[0-9a-f]{12}$'
        (Split-Path -Parent $a) | Should Be (Join-Path 'C:' 'bt')
        $b | Should Be $a
    }

    It 'refuses an artifact older than the minimum write time with a FATAL code' {
        $release = Join-Path $TestDrive 'stale\release'
        New-Item -ItemType Directory -Force -Path $release | Out-Null
        $exe = Join-Path $release 'synapse-mcp.exe'
        [System.IO.File]::WriteAllText($exe, 'old')
        (Get-Item $exe).LastWriteTimeUtc = [datetime]'2026-09-07'
        $message = $null
        try {
            Resolve-SynapseSkipBuildCandidatePath -CargoTarget (Join-Path $TestDrive 'stale') -MinimumWriteTimeUtc ([datetime]'2026-10-14')
        } catch { $message = $_.Exception.Message }
        $message | Should Match '^SYNAPSE_SKIP_BUILD_ARTIFACT_STALE '
    }

    It 'accepts an artifact newer than the minimum write time' {
        $release = Join-Path $TestDrive 'fresh\release'
        New-Item -ItemType Directory -Force -Path $release | Out-Null
        $exe = Join-Path $release 'synapse-mcp.exe'
        [System.IO.File]::WriteAllText($exe, 'new')
        (Resolve-SynapseSkipBuildCandidatePath -CargoTarget (Join-Path $TestDrive 'fresh') -MinimumWriteTimeUtc ([datetime]'2020-01-01')) |
            Should Be (Resolve-Path -LiteralPath $exe).Path
    }

    It 'grows the retry delay exponentially up to a cap' {
        (Get-SynapseRetryDelayMs -Attempt 1 -BaseMs 200 -MaxMs 2000) | Should Be 200
        (Get-SynapseRetryDelayMs -Attempt 2 -BaseMs 200 -MaxMs 2000) | Should Be 400
        (Get-SynapseRetryDelayMs -Attempt 3 -BaseMs 200 -MaxMs 2000) | Should Be 800
        (Get-SynapseRetryDelayMs -Attempt 9 -BaseMs 200 -MaxMs 2000) | Should Be 2000
    }

    It 'wires SkipBuild to the per-checkout target, HEAD identity, and revision assertion' {
        $text = Get-Content -Raw -LiteralPath $setupPath
        $text | Should Match 'Get-SynapseCheckoutCargoTarget -Root \$CargoTarget'
        $text | Should Match '-MinimumWriteTimeUtc'
        $text | Should Match 'Get-SynapseSkipBuildHeadIdentity -SourceDir \$skipSource'
        $text | Should Match 'Assert-SynapseSkipBuildRevision -CandidateBuild \$candidatePreflight\.Build'
        $text | Should Not Match 'stale-artifact gate skipped'
    }

    It 'reports the candidate daemon health build in the preflight result' {
        $text = Get-Content -Raw -LiteralPath $setupPath
        $text | Should Match 'Build = \[string\]\$health\.build'
    }

    It 'reads HEAD identity from a real repo and refuses an unreadable one with a FATAL code' {
        $repo = Join-Path (Resolve-Path $TestDrive).ProviderPath 'repo'
        New-Item -ItemType Directory -Force -Path $repo | Out-Null
        & git -C $repo init -q 2>&1 | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $repo 'a.txt'), 'a')
        & git -C $repo add a.txt 2>&1 | Out-Null
        & git -C $repo -c user.name=t -c user.email=t@t.invalid commit -q -m init 2>&1 | Out-Null
        $id = Get-SynapseSkipBuildHeadIdentity -SourceDir $repo
        $id.Revision | Should Match '^[0-9a-f]{12}$'
        $id.CommitTimeUtc | Should BeOfType [datetime]
        $id.Dirty | Should Be $false
        [System.IO.File]::WriteAllText((Join-Path $repo 'a.txt'), 'changed')
        (Get-SynapseSkipBuildHeadIdentity -SourceDir $repo).Dirty | Should Be $true

        $message = $null
        try { Get-SynapseSkipBuildHeadIdentity -SourceDir (Join-Path $TestDrive 'not-a-repo') } catch { $message = $_.Exception.Message }
        $message | Should Match '^SYNAPSE_SKIP_BUILD_HEAD_UNREADABLE FATAL'
    }

    It 'accepts a candidate whose embedded build equals HEAD (clean or dirty)' {
        $head = [pscustomobject]@{ Revision = '91c79baf5fe1'; CommitTimeUtc = [datetime]'2026-10-14'; Dirty = $false }
        { Assert-SynapseSkipBuildRevision -CandidateBuild '91c79baf5fe1' -HeadIdentity $head -CandidatePath 'x.exe' } | Should Not Throw
        { Assert-SynapseSkipBuildRevision -CandidateBuild '91c79baf5fe1-dirty' -HeadIdentity $head -CandidatePath 'x.exe' } | Should Not Throw
    }

    It 'refuses a newer artifact built from a different revision' {
        $head = [pscustomobject]@{ Revision = '91c79baf5fe1'; CommitTimeUtc = [datetime]'2026-10-14'; Dirty = $false }
        $message = $null
        try { Assert-SynapseSkipBuildRevision -CandidateBuild '0123456789ab' -HeadIdentity $head -CandidatePath 'x.exe' } catch { $message = $_.Exception.Message }
        $message | Should Match '^SYNAPSE_SKIP_BUILD_REVISION_MISMATCH FATAL'
        $message | Should Match 'candidate_build=0123456789ab'
        $message | Should Match 'head_revision=91c79baf5fe1'
    }

    It 'refuses a candidate with no embedded revision' {
        $head = [pscustomobject]@{ Revision = '91c79baf5fe1'; CommitTimeUtc = [datetime]'2026-10-14'; Dirty = $false }
        foreach ($build in @('dev', '', $null)) {
            $message = $null
            try { Assert-SynapseSkipBuildRevision -CandidateBuild $build -HeadIdentity $head -CandidatePath 'x.exe' } catch { $message = $_.Exception.Message }
            $message | Should Match '^SYNAPSE_SKIP_BUILD_REVISION_UNKNOWN FATAL'
        }
    }

    It 'keeps Install-SynapseBinaryOverHeldImage under 50 lines via the move-retry helper' {
        $fn = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Install-SynapseBinaryOverHeldImage'
            }, $true)
        ($fn.Extent.EndLineNumber - $fn.Extent.StartLineNumber + 1) | Should BeLessThan 50
        $fn.Extent.Text | Should Match 'Move-SynapseFileWithSharingRetry'
    }
}
