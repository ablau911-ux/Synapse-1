$ErrorActionPreference = 'Stop'

Describe 'Synapse setup SkipBuild candidate selection' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $setupPath,
            [ref]$tokens,
            [ref]$errors
        )
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @(
            'Get-SynapseFileSha256',
            'Get-SynapseCargoReleaseArtifactPath',
            'Resolve-SynapseSkipBuildCandidatePath',
            'Get-SynapseInstallDisposition',
            'Restore-SynapseBinaryBackupAfterInstallFailure',
            'Test-SynapseSharingViolation',
            'Get-SynapseRetryDelayMs',
            'Copy-SynapseFileWithSharingRetry',
            'Move-SynapseFileWithSharingRetry',
            'Install-SynapseBinaryOverHeldImage'
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
        function Get-ScheduledTask { param([string]$TaskName, $ErrorAction) [pscustomobject]@{ TaskName = $TaskName } }
        function Start-ScheduledTask { param([string]$TaskName) $script:restartedTask = $TaskName }
    }

    It 'selects and installs the CargoTarget release artifact instead of the destination' {
        $target = Join-Path $TestDrive 'target'
        $release = Join-Path $target 'release'
        $source = Join-Path $release 'synapse-mcp.exe'
        $destination = Join-Path $TestDrive 'installed\synapse-mcp.exe'
        New-Item -ItemType Directory -Force -Path $release, (Split-Path -Parent $destination) | Out-Null
        [System.IO.File]::WriteAllText($source, 'new candidate bytes')
        [System.IO.File]::WriteAllText($destination, 'old installed bytes')
        $oldHash = Get-SynapseFileSha256 -Path $destination

        $selected = Resolve-SynapseSkipBuildCandidatePath -CargoTarget $target
        $candidateHash = Get-SynapseFileSha256 -Path $selected
        $candidateHash | Should Not Be $oldHash
        $selected | Should Be (Resolve-Path -LiteralPath $source).Path

        Copy-SynapseFileWithSharingRetry -Source $selected -Destination $destination
        (Get-SynapseFileSha256 -Path $destination) | Should Be $candidateHash
    }

    It 'fails clearly when CargoTarget has no release artifact' {
        $target = Join-Path $TestDrive 'missing-target'
        New-Item -ItemType Directory -Force -Path $target | Out-Null

        $message = $null
        try {
            Resolve-SynapseSkipBuildCandidatePath -CargoTarget $target
        } catch {
            $message = $_.Exception.Message
        }
        $message | Should Match '^SYNAPSE_SKIP_BUILD_BINARY_MISSING '
        $message | Should Match 'release\\synapse-mcp\.exe'
    }

    It 'keeps same-path installs safe' {
        $target = Join-Path $TestDrive 'same-target'
        $release = Join-Path $target 'release'
        $source = Join-Path $release 'synapse-mcp.exe'
        New-Item -ItemType Directory -Force -Path $release | Out-Null
        [System.IO.File]::WriteAllText($source, 'candidate')

        $selected = Resolve-SynapseSkipBuildCandidatePath -CargoTarget $target
        [System.IO.Path]::GetFullPath($selected) |
            Should Be ([System.IO.Path]::GetFullPath($source))
    }

    It 'skips installation by hash even when source and destination paths differ' {
        $source = Join-Path $TestDrive 'alias-source.exe'
        $destination = Join-Path $TestDrive 'alias-destination.exe'
        [System.IO.File]::WriteAllText($source, 'identical bytes')
        [System.IO.File]::WriteAllText($destination, 'identical bytes')

        $candidateHash = Get-SynapseFileSha256 -Path $source
        $disposition = Get-SynapseInstallDisposition `
            -CandidateHash $candidateHash `
            -ExePath $destination
        $disposition.InstallRequired | Should Be $false
        $disposition.InstalledHash | Should Be $candidateHash
    }

    It 'requires installation when the destination is absent' {
        $destination = Join-Path $TestDrive 'fresh\synapse-mcp.exe'
        $disposition = Get-SynapseInstallDisposition `
            -CandidateHash ('A' * 64) `
            -ExePath $destination
        $disposition.InstallRequired | Should Be $true
        $disposition.InstalledHash | Should Be $null
    }

    It 'resolves a configured Cargo target triple artifact' {
        $previous = $env:CARGO_BUILD_TARGET
        try {
            $env:CARGO_BUILD_TARGET = 'x86_64-pc-windows-msvc'
            $target = Join-Path $TestDrive 'triple-target'
            $expected = Join-Path $target 'x86_64-pc-windows-msvc\release\synapse-mcp.exe'
            (Get-SynapseCargoReleaseArtifactPath -CargoTarget $target) |
                Should Be ([System.IO.Path]::GetFullPath($expected))
        } finally {
            $env:CARGO_BUILD_TARGET = $previous
        }
    }

    It 'resolves a relative CargoTarget against the process working directory' {
        $previousLocation = Get-Location
        $previousCurrentDirectory = [Environment]::CurrentDirectory
        try {
            $root = Join-Path $TestDrive 'relative-root'
            $release = Join-Path $root 'release'
            New-Item -ItemType Directory -Force -Path $release | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $release 'synapse-mcp.exe'), 'candidate')
            Set-Location $root
            [Environment]::CurrentDirectory = $root

            $selected = Resolve-SynapseSkipBuildCandidatePath -CargoTarget '.'
            [System.IO.Path]::GetFullPath($selected) |
                Should Be ([System.IO.Path]::GetFullPath((Join-Path $release 'synapse-mcp.exe')))
        } finally {
            Set-Location $previousLocation
            [Environment]::CurrentDirectory = $previousCurrentDirectory
        }
    }

    It 'keeps the post-exit continuation on the installed verified binary' {
        $setupText = Get-Content -Raw -LiteralPath $setupPath

        $setupText | Should Match '(?m)^\s{4}\[switch\]\$PostExitUseInstalledBinary,'
        # The continuation launcher must request the installed-binary branch.
        $setupText | Should Match "(?s)'-SkipBuild',\s*'-PostExitUseInstalledBinary'"
        # The installed-binary branch must win over plain artifact resolution.
        $setupText | Should Match '\$SkipBuild -and \$PostExitUseInstalledBinary'
        # The flag is continuation-only: a plain -SkipBuild run must be rejected.
        $setupText | Should Match 'SYNAPSE_POST_EXIT_USE_INSTALLED_BINARY_MISUSED'
        $setupText | Should Match 'SYNAPSE_POST_EXIT_INSTALLED_BINARY_MISSING'
        # The launcher must hand the parent-verified installed hash through so
        # the continuation can prove the bytes were not replaced.
        $setupText | Should Match "(?s)-PostExitUseInstalledBinary',\s*'-PostExitInstalledSha256'"
        $setupText | Should Match '(?m)^\s{4}\[string\]\$PostExitInstalledSha256 = '''',\s*$'
        $setupText | Should Match '-InstalledBinarySha256 \$installedHash'
        $setupText | Should Match '\$PostExitUseInstalledBinary -and -not \(\$script:SynapsePostExitStartOnly -and \$SkipBuild\)'
        $setupText | Should Match 'SYNAPSE_POST_EXIT_INSTALLED_SHA_MISSING'
        $setupText | Should Match 'SYNAPSE_POST_EXIT_INSTALLED_BINARY_REPLACED'
        # The installed-bytes equality gate must run before the candidate is
        # accepted as the install source.
        $setupText | Should Match '(?s)if \(\$installSourceHash -ne \$PostExitInstalledSha256\) \{\s*Die "SYNAPSE_POST_EXIT_INSTALLED_BINARY_REPLACED'
        # The launched daemon image is re-hashed after health, binding the
        # executed bytes to the parent-verified hash (the OS write-locks a
        # running image, so post-launch bytes are the bytes that launched).
        $setupText | Should Match 'SYNAPSE_POST_EXIT_LAUNCHED_BINARY_REPLACED'
        # The installed-binary contract is mandatory in continuation mode: a
        # hand-written post-exit run without the flag must fail closed.
        $setupText | Should Match '\$script:SynapsePostExitStartOnly -and -not \$PostExitUseInstalledBinary'
        $setupText | Should Match 'SYNAPSE_POST_EXIT_INSTALLED_BINARY_MODE_REQUIRED'
        # The executed-image re-hash must sit after the health gate and
        # before the continuation completion manifest.
        $launchedIdx = $setupText.IndexOf('SYNAPSE_POST_EXIT_LAUNCHED_BINARY_REPLACED')
        $healthIdx = $setupText.IndexOf('SYNAPSE_INSTALL_HEALTH_FAILED')
        # The unique post-health bridge-step marker bounds the re-hash from
        # above: it must run after health but before any bridge/wiring step
        # treats the daemon as accepted.
        $bridgeIdx = $setupText.IndexOf('Skipped Chrome bridge post-start assertion')
        $completedIdx = $setupText.IndexOf('SYNAPSE_POST_EXIT_CONTINUATION_COMPLETED')
        $launchedIdx | Should BeGreaterThan $healthIdx
        $bridgeIdx | Should BeGreaterThan $launchedIdx
        $completedIdx | Should BeGreaterThan $launchedIdx
    }

    It 'binds the parent-verified hash through the continuation launcher (AST)' {
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        $errors.Count | Should Be 0
        $launcher = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Start-SynapsePostExitSetupContinuation'
            }, $true)
        $launcher | Should Not Be $null

        # Document-order flattening of the launcher's argument array: assert
        # the '-PostExitInstalledSha256' literal is immediately followed by
        # the $InstalledBinarySha256 variable element, so the child really
        # receives the hash the parent passed in.
        $argArray = $launcher.Body.Find({
                param($node)
                $node -is [System.Management.Automation.Language.ArrayExpressionAst]
            }, $true)
        $argArray | Should Not Be $null
        $flat = @($argArray.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.StringConstantExpressionAst] -or
                    $node -is [System.Management.Automation.Language.VariableExpressionAst]
                }, $true) | ForEach-Object {
            if ($_ -is [System.Management.Automation.Language.VariableExpressionAst]) { $_.VariablePath.UserPath } else { $_.Value }
        })
        $idx = [array]::IndexOf($flat, '-PostExitInstalledSha256')
        $idx | Should BeGreaterThan -1
        $flat[$idx + 1] | Should Be 'InstalledBinarySha256'

        # The call site must bind the function parameter to the hash the
        # parent computed and verified after installing ($installedHash).
        $call = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.CommandAst] -and
                $node.GetCommandName() -eq 'Start-SynapsePostExitSetupContinuation'
            }, $true)
        $call | Should Not Be $null
        $callFlat = @($call.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.StringConstantExpressionAst] -or
                    $node -is [System.Management.Automation.Language.CommandParameterAst] -or
                    $node -is [System.Management.Automation.Language.VariableExpressionAst]
                }, $true) | ForEach-Object {
            if ($_ -is [System.Management.Automation.Language.VariableExpressionAst]) { $_.VariablePath.UserPath }
            elseif ($_ -is [System.Management.Automation.Language.CommandParameterAst]) { $_.ParameterName }
            else { $_.Value }
        })
        $callIdx = [array]::IndexOf($callFlat, 'InstalledBinarySha256')
        $callIdx | Should BeGreaterThan -1
        $callFlat[$callIdx + 1] | Should Be 'installedHash'
    }

    It 'restores the verified backup and restarts the scheduled daemon after copy failure' {
        $backup = Join-Path $TestDrive 'synapse-mcp.exe.bak'
        $destination = Join-Path $TestDrive 'rollback\synapse-mcp.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
        [System.IO.File]::WriteAllText($backup, 'old verified daemon')
        [System.IO.File]::WriteAllText($destination, 'partial candidate')
        $expected = Get-SynapseFileSha256 -Path $backup
        $script:restartedTask = $null

        $restored = Restore-SynapseBinaryBackupAfterInstallFailure `
            -BackupPath $backup `
            -ExePath $destination `
            -ExpectedHash $expected `
            -TaskName 'SynapseMcpDaemon' `
            -InstallError 'fixture copy failure'

        $restored | Should Be $expected
        (Get-SynapseFileSha256 -Path $destination) | Should Be $expected
        $script:restartedTask | Should Be 'SynapseMcpDaemon'
    }
}
