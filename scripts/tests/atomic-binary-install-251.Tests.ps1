$ErrorActionPreference = 'Stop'
# Pester 3.4. Import ONLY function ASTs. Never dot-source setup's entry point.
Describe 'Atomic daemon binary publication #251' {
    BeforeAll {
        $setup = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null; $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setup, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw ($errors | Out-String) }
        foreach ($name in @('Install-SynapseBinaryOverHeldImage', 'Get-SynapseFileSha256',
                'Test-SynapsePathStrictlyInside', 'Test-SynapseTreeHasReparsePoint', 'Remove-SynapseSetupOwnedDirectory')) {
            $fn = $ast.Find({ param($n)
                $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
            }, $true)
            if ($null -eq $fn) { throw "Missing production function $name" }
            Invoke-Expression $fn.Extent.Text
        }
        function Info([string]$Message) { $script:InfoMessages += $Message }
        function Die([string]$Message) { throw $Message }
        function Copy-SynapseFileWithSharingRetry([string]$Source, [string]$Destination) {
            $script:Copies += [pscustomobject]@{
                Destination = $Destination
                Canonical = if (Test-Path -LiteralPath $script:Canonical) {
                    [IO.File]::ReadAllText($script:Canonical)
                } else { '<absent>' }
            }
            if ($script:RemoveCandidateDuringCopy) { Remove-Item -LiteralPath $Source -Force }
            if ($script:CorruptCopy) { [IO.File]::WriteAllText($Destination, 'corrupt'); return }
            Copy-Item -LiteralPath $Source -Destination $Destination -ErrorAction Stop
        }
        function Move-SynapseFileWithSharingRetry([string]$Source, [string]$Destination) {
            Move-Item -LiteralPath $Source -Destination $Destination -ErrorAction Stop
        }
        function New-BinaryCase([string]$Case) {
            $root = Join-Path $TestDrive $Case
            New-Item -ItemType Directory -Path $root | Out-Null
            $source = Join-Path $root 'candidate.exe'
            $script:Canonical = Join-Path $root 'synapse-mcp.exe'
            [IO.File]::WriteAllText($source, 'candidate-251')
            [IO.File]::WriteAllText($script:Canonical, 'old-251')
            $script:Copies = @(); $script:CorruptCopy = $false; $script:RemoveCandidateDuringCopy = $false
            return $source
        }
    }

    It 'must_pass_candidate_is_staged_hashed_and_renamed_atomically' {
        $source = New-BinaryCase 'normal'
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'old-251'
        $null = Install-SynapseBinaryOverHeldImage -Source $source -Destination $script:Canonical
        $script:Copies.Count | Should Be 1
        $script:Copies[0].Destination | Should Match ('^' + [regex]::Escape($script:Canonical) + '\.incoming-')
        $script:Copies[0].Canonical | Should Be 'old-251'
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'candidate-251'
        (Get-FileHash -LiteralPath $script:Canonical).Hash | Should Be (Get-FileHash -LiteralPath $source).Hash
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $source) -Filter '*.incoming-*').Count | Should Be 0
    }

    It 'must_fail_corrupt_staging_copy_preserves_old_binary' {
        $source = New-BinaryCase 'corrupt'
        $script:CorruptCopy = $true
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'old-251'
        $message = $null
        try { $null = Install-SynapseBinaryOverHeldImage -Source $source -Destination $script:Canonical }
        catch { $message = $_.Exception.Message }
        $message | Should Match 'SYNAPSE_INSTALL_PREPARED_HASH_MISMATCH'
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'old-251'
        $script:Copies[0].Canonical | Should Be 'old-251'
    }

    It 'must_fail_missing_candidate_preserves_binary_without_staging' {
        $source = New-BinaryCase 'missing'
        $missing = Join-Path (Split-Path -Parent $source) 'does-not-exist.exe'
        Test-Path -LiteralPath $missing | Should Be $false
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'old-251'
        { Install-SynapseBinaryOverHeldImage -Source $missing -Destination $script:Canonical } |
            Should Throw 'SYNAPSE_BINARY_INSTALL_SOURCE_MISSING'
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'old-251'
        $script:Copies.Count | Should Be 0
        # Also cover a candidate disappearing after the initial existence check.
        # The old canonical must remain readable while acquisition is attempted.
        $source = New-BinaryCase 'missing-during-staging'
        $script:RemoveCandidateDuringCopy = $true
        $message = $null
        try { $null = Install-SynapseBinaryOverHeldImage -Source $source -Destination $script:Canonical }
        catch { $message = $_.Exception.Message }
        $message | Should Match '^SYNAPSE_BINARY_INSTALL_PUBLISH_FAILED'
        $script:Copies.Count | Should Be 1
        $script:Copies[0].Canonical | Should Be 'old-251'
        [IO.File]::ReadAllText($script:Canonical) | Should Be 'old-251'
        @(Get-ChildItem -LiteralPath (Split-Path -Parent $source) -Filter '*.incoming-*').Count | Should Be 0
    }

    It 'must_fail_cleanup_preserves_siblings_and_nested_reparse_targets' {
        $root = Join-Path $TestDrive 'cleanup-root'
        $foreign = Join-Path $TestDrive 'cleanup-root-sibling'
        $owned = Join-Path $root 'candidate-20261015T000000000Z-4242'
        New-Item -ItemType Directory -Path $root,$foreign,$owned -Force | Out-Null
        $sentinel = Join-Path $foreign 'keep.txt'
        [IO.File]::WriteAllText($sentinel, 'KEEP_REPARSE_251')
        [IO.File]::ReadAllText($sentinel) | Should Be 'KEEP_REPARSE_251'
        Remove-SynapseSetupOwnedDirectory -Path $foreign -Root $root | Should Be $false
        New-Item -ItemType Junction -Path (Join-Path $owned 'link') -Target $foreign | Out-Null
        ((Get-Item -LiteralPath (Join-Path $owned 'link')).Attributes -band [IO.FileAttributes]::ReparsePoint) |
            Should Not Be 0
        $script:InfoMessages = @()
        $removed = Remove-SynapseSetupOwnedDirectory -Path $owned -Root $root
        $script:InfoMessages.Count | Should Be 1
        $script:InfoMessages[0] | Should Be "WARN: SYNAPSE_SETUP_CLEANUP_REFUSED reason=reparse_point_or_not_a_directory path=$owned"
        $removed | Should Be $false
        [IO.File]::ReadAllText($sentinel) | Should Be 'KEEP_REPARSE_251'
        Test-Path -LiteralPath $owned | Should Be $true
    }
}
