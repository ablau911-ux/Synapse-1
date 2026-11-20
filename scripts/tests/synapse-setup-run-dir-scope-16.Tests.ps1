$ErrorActionPreference = 'Stop'

# #16 raid tests (Pester 3.4.0). Recursive cleanup of setup run directories may only
# remove an EXACT DIRECT CHILD of its owning root whose name has the owned run-dir shape
# (`daemon-binary-` / `candidate-` + stamp + pid, see New-SynapseSetupRunDirectory), and
# maintenance start must reclaim legacy run directories left by earlier setups.
# Everything lives under $TestDrive; nothing outside it is created or deleted.
Describe 'Synapse setup run-directory cleanup scope and legacy prune (#16)' {
    BeforeAll {
        $script:setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $script:loaded = @{}
        foreach ($name in @('Test-SynapsePathStrictlyInside', 'Test-SynapseTreeHasReparsePoint',
                'Remove-SynapseSetupOwnedDirectory', 'New-SynapseSetupRunDirectory',
                'Remove-SynapseLegacySetupRunDirectories')) {
            $fn = $script:ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
                }, $true)
            $script:loaded[$name] = ($null -ne $fn)
            if ($null -ne $fn) { Invoke-Expression $fn.Extent.Text }
        }
        $script:messages = @()
        function Info([string]$Message) { $script:messages += $Message }
        function Die([string]$Message) { throw $Message }
        function Start-Sleep { param([int]$Milliseconds, [int]$Seconds) }

        function New-Tree([string]$Path) {
            New-Item -ItemType Directory -Force -Path $Path | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $Path 'payload.bin'), 'payload')
            return $Path
        }
        function New-OwnedName([string]$Prefix) { return "$Prefix-20260101T010203004Z-4242" }
    }

    BeforeEach { $script:messages = @() }

    It 'must_pass removes a run directory created by New-SynapseSetupRunDirectory and keeps its siblings' {
        $root = Join-Path $TestDrive 'p1\setup-staging'
        $sibling = New-Tree (Join-Path $root 'operator-notes')
        $run = New-SynapseSetupRunDirectory -Root $root -Purpose 'daemon-binary'
        New-Tree $run | Out-Null
        Test-Path -LiteralPath (Join-Path $run 'payload.bin') | Should Be $true

        Remove-SynapseSetupOwnedDirectory -Path $run -Root $root | Should Be $true

        Test-Path -LiteralPath $run | Should Be $false
        Test-Path -LiteralPath (Join-Path $sibling 'payload.bin') | Should Be $true
        Test-Path -LiteralPath $root | Should Be $true
    }

    It 'must_fail refuses a nested descendant that is not a direct child of the root' {
        $root = Join-Path $TestDrive 'f1\setup-candidates'
        $run = New-Tree (Join-Path $root (New-OwnedName 'candidate'))
        $nested = New-Tree (Join-Path $run 'db')
        Test-Path -LiteralPath (Join-Path $nested 'payload.bin') | Should Be $true

        Remove-SynapseSetupOwnedDirectory -Path $nested -Root $root | Should Be $false

        Test-Path -LiteralPath (Join-Path $nested 'payload.bin') | Should Be $true
        ($script:messages -join "`n") | Should Match 'SYNAPSE_SETUP_CLEANUP_REFUSED'
    }

    It 'must_fail refuses a direct child whose name is not an owned run directory' {
        $root = Join-Path $TestDrive 'f2\setup-staging'
        $foreign = New-Tree (Join-Path $root 'keep-me')
        $prefixOnly = New-Tree (Join-Path $root 'candidate-notes')

        Remove-SynapseSetupOwnedDirectory -Path $foreign -Root $root | Should Be $false
        Remove-SynapseSetupOwnedDirectory -Path $prefixOnly -Root $root | Should Be $false

        Test-Path -LiteralPath (Join-Path $foreign 'payload.bin') | Should Be $true
        Test-Path -LiteralPath (Join-Path $prefixOnly 'payload.bin') | Should Be $true
        ($script:messages -join "`n") | Should Match 'SYNAPSE_SETUP_CLEANUP_REFUSED'
    }

    It 'must_pass legacy prune reclaims owned run directories and keeps logs and foreign entries' {
        $script:loaded['Remove-SynapseLegacySetupRunDirectories'] | Should Be $true
        $logDir = Join-Path $TestDrive 'p2\logs'
        $staging = Join-Path $logDir 'setup-staging'
        $candidates = Join-Path $logDir 'setup-candidates'
        $oldStage = New-Tree (Join-Path $staging (New-OwnedName 'daemon-binary'))
        $oldCand = New-Tree (Join-Path $candidates (New-OwnedName 'candidate'))
        New-Tree (Join-Path $oldCand 'db') | Out-Null
        $foreign = New-Tree (Join-Path $staging 'operator-notes')
        [System.IO.File]::WriteAllText((Join-Path $logDir 'setup-build.log'), 'log')

        $removed = @(Remove-SynapseLegacySetupRunDirectories -LogDir $logDir)

        $removed.Count | Should Be 2
        Test-Path -LiteralPath $oldStage | Should Be $false
        Test-Path -LiteralPath $oldCand | Should Be $false
        Test-Path -LiteralPath (Join-Path $foreign 'payload.bin') | Should Be $true
        [System.IO.File]::ReadAllText((Join-Path $logDir 'setup-build.log')) | Should Be 'log'
        Test-Path -LiteralPath $staging | Should Be $true
        Test-Path -LiteralPath $candidates | Should Be $true
    }

    It 'must_fail legacy prune never follows a junction named like a run directory' {
        $script:loaded['Remove-SynapseLegacySetupRunDirectories'] | Should Be $true
        $logDir = Join-Path $TestDrive 'f3\logs'
        $staging = Join-Path $logDir 'setup-staging'
        New-Item -ItemType Directory -Force -Path $staging | Out-Null
        $outside = New-Tree (Join-Path $TestDrive 'f3\outside')
        $link = Join-Path $staging (New-OwnedName 'daemon-binary')
        New-Item -ItemType Junction -Path $link -Target $outside | Out-Null
        ((Get-Item -LiteralPath $link -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) | Should Not Be 0
        # A no-op pruner also preserves junctions. Require progress on an ordinary
        # owned sibling in the same invocation while refusing the junction.
        $owned = New-Tree (Join-Path $staging (New-OwnedName 'candidate'))

        $removed = @(Remove-SynapseLegacySetupRunDirectories -LogDir $logDir)

        ('#16 junction-safe prune removed owned sibling: ' + ($removed -contains $owned)) | Should Be '#16 junction-safe prune removed owned sibling: True'
        $removed.Count | Should Be 1
        Test-Path -LiteralPath $owned | Should Be $false
        Test-Path -LiteralPath $link | Should Be $true
        [System.IO.File]::ReadAllText((Join-Path $outside 'payload.bin')) | Should Be 'payload'
    }

    It 'must_pass maintenance start prunes legacy run directories after the lock and before preflight' {
        $top = @($script:ast.EndBlock.Statements)
        $lock = $top | Where-Object { $_.Extent.Text -match '^\s*Acquire-SynapseSetupMaintenanceLock\b' } | Select-Object -First 1
        # The call may sit inside a top-level `if` (e.g. not for -Remove), but never only inside a function.
        $prune = @($script:ast.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.CommandAst] -and
                    $node.GetCommandName() -eq 'Remove-SynapseLegacySetupRunDirectories'
                }, $true) | Where-Object {
                $p = $_.Parent
                while ($null -ne $p -and $p -isnot [System.Management.Automation.Language.FunctionDefinitionAst]) { $p = $p.Parent }
                $null -eq $p
            }) | Select-Object -First 1
        $preflight = $top | Where-Object { $_.Extent.Text -match '^\s*Step\s+"Preflight"' } | Select-Object -First 1
        $null -eq $lock | Should Be $false
        $null -eq $preflight | Should Be $false
        $null -eq $prune | Should Be $false
        ($prune.Extent.StartOffset -gt $lock.Extent.StartOffset) | Should Be $true
        ($prune.Extent.StartOffset -lt $preflight.Extent.StartOffset) | Should Be $true
    }
}
