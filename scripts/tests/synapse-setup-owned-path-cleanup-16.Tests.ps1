$ErrorActionPreference = 'Stop'
# Fixture names use the setup run-directory shape (`<purpose>-<yyyyMMddTHHmmssfffZ>-<pid>`):
# the frozen #16 contract refuses any other direct child of the owning root.

# #16: setup must remove the exact run directories it created (staged binary copy and
# candidate DB) instead of leaking one per upgrade, without following redirections.
Describe 'Synapse setup-owned directory cleanup (#16)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        function Get-FunctionAst([string]$Name) {
            $fn = $script:ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $Name }, $true)
            if ($null -eq $fn) { throw "function $Name is missing" }
            return $fn
        }
        foreach ($name in 'Test-SynapsePathStrictlyInside', 'Test-SynapseTreeHasReparsePoint', 'Remove-SynapseSetupOwnedDirectory',
                'Register-SynapseSetupOwnedDirectory', 'Remove-SynapseSetupOwnedDirectories') {
            Invoke-Expression (Get-FunctionAst $name).Extent.Text
        }
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }
        $script:SynapseSetupOwnedDirectories = @()
    }

    BeforeEach {
        $script:root = Join-Path $TestDrive ('root-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $script:root | Out-Null
        $script:SynapseSetupOwnedDirectories = @()
    }

    It 'removes a run directory and its contents' {
        $run = Join-Path $script:root 'candidate-20261016T120000001Z-4242'
        New-Item -ItemType Directory -Path (Join-Path $run 'db') | Out-Null
        Set-Content -LiteralPath (Join-Path $run 'db\CURRENT') -Value 'x'
        Remove-SynapseSetupOwnedDirectory -Path $run -Root $script:root | Should Be $true
        Test-Path -LiteralPath $run | Should Be $false
        Test-Path -LiteralPath $script:root | Should Be $true
    }

    It 'refuses a path outside the owning root' {
        $outside = Join-Path $TestDrive ('outside-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $outside | Out-Null
        Remove-SynapseSetupOwnedDirectory -Path $outside -Root $script:root | Should Be $false
        Test-Path -LiteralPath $outside | Should Be $true
    }

    It 'refuses the root itself' {
        Remove-SynapseSetupOwnedDirectory -Path $script:root -Root $script:root | Should Be $false
        Test-Path -LiteralPath $script:root | Should Be $true
    }

    It 'does not follow a reparse point inside the run directory' {
        $run = Join-Path $script:root 'candidate-20261016T120000002Z-4242'
        $foreign = Join-Path $TestDrive ('foreign-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $run, $foreign | Out-Null
        Set-Content -LiteralPath (Join-Path $foreign 'keep.txt') -Value 'keep'
        New-Item -ItemType Junction -Path (Join-Path $run 'link') -Target $foreign | Out-Null
        Remove-SynapseSetupOwnedDirectory -Path $run -Root $script:root | Should Be $false
        Test-Path -LiteralPath (Join-Path $foreign 'keep.txt') | Should Be $true
        Test-Path -LiteralPath $run | Should Be $true
    }

    It 'refuses a run directory that is itself a reparse point' {
        $foreign = Join-Path $TestDrive ('foreign-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $foreign | Out-Null
        Set-Content -LiteralPath (Join-Path $foreign 'keep.txt') -Value 'keep'
        $run = Join-Path $script:root 'candidate-20261016T120000003Z-4242'
        New-Item -ItemType Junction -Path $run -Target $foreign | Out-Null
        Remove-SynapseSetupOwnedDirectory -Path $run -Root $script:root | Should Be $false
        Test-Path -LiteralPath (Join-Path $foreign 'keep.txt') | Should Be $true
    }

    It 'reports a locked path without throwing' {
        $run = Join-Path $script:root 'candidate-20261016T120000004Z-4242'
        New-Item -ItemType Directory -Path $run | Out-Null
        $lockedFile = Join-Path $run 'locked.db'
        $stream = [System.IO.File]::Open($lockedFile, 'Create', 'ReadWrite', 'None')
        try {
            Remove-SynapseSetupOwnedDirectory -Path $run -Root $script:root | Should Be $false
            Test-Path -LiteralPath $lockedFile | Should Be $true
        } finally {
            $stream.Dispose()
        }
    }

    It 'treats an already-missing directory as cleaned' {
        Remove-SynapseSetupOwnedDirectory -Path (Join-Path $script:root 'candidate-20261016T120000009Z-4242') -Root $script:root | Should Be $true
    }

    It 'removes every registered directory and clears the registry' {
        $a = Join-Path $script:root 'daemon-binary-20261016T120000001Z-4242'
        $b = Join-Path $script:root 'daemon-binary-20261016T120000002Z-4242'
        New-Item -ItemType Directory -Path $a, $b | Out-Null
        Register-SynapseSetupOwnedDirectory -Path $a -Root $script:root
        Register-SynapseSetupOwnedDirectory -Path $b -Root $script:root
        Remove-SynapseSetupOwnedDirectories
        Test-Path -LiteralPath $a | Should Be $false
        Test-Path -LiteralPath $b | Should Be $false
        @($script:SynapseSetupOwnedDirectories).Count | Should Be 0
    }

    It 'keeps a registered directory registered when cleanup is refused' {
        $outside = Join-Path $TestDrive ('outside-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $outside | Out-Null
        Register-SynapseSetupOwnedDirectory -Path $outside -Root $script:root
        Remove-SynapseSetupOwnedDirectories
        Test-Path -LiteralPath $outside | Should Be $true
    }

    Context 'setup wiring' {
        It 'candidate preflight removes its run directory in the finally block' {
            $fn = Get-FunctionAst 'Test-SynapseCandidateDaemon'
            $try = $fn.Body.Find({ param($n) $n -is [System.Management.Automation.Language.TryStatementAst] }, $false)
            $try.Finally | Should Not BeNullOrEmpty
            $try.Finally.Extent.Text | Should Match 'Remove-SynapseSetupOwnedDirectory'
        }

        It 'staging registers its run directory for cleanup' {
            (Get-FunctionAst 'New-SynapseStagedDaemonBinary').Extent.Text | Should Match 'Register-SynapseSetupOwnedDirectory'
        }

        It 'failure trap cleans registered directories' {
            $trap = $script:ast.Find({ param($n) $n -is [System.Management.Automation.Language.TrapStatementAst] }, $false)
            $trap.Extent.Text | Should Match 'Remove-SynapseSetupOwnedDirectories'
        }

        It 'success path cleans registered directories once the installed binary is verified' {
            $cleanup = $null
            $verified = $null
            foreach ($statement in @($script:ast.EndBlock.Statements)) {
                foreach ($command in @($statement.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] }, $false))) {
                    if ($command.GetCommandName() -eq 'Remove-SynapseSetupOwnedDirectories' -and $null -eq $cleanup) { $cleanup = $statement.Extent.StartOffset }
                    if ($command.Extent.Text -match 'Installed binary verified path' -and $null -eq $verified) { $verified = $statement.Extent.StartOffset }
                }
            }
            $cleanup | Should Not BeNullOrEmpty
            $verified | Should Not BeNullOrEmpty
            ($cleanup -ge $verified) | Should Be $true
        }
    }
}
