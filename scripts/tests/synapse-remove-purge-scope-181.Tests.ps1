$ErrorActionPreference = 'Stop'

Describe 'Synapse setup -Remove -Purge deletion scope (#181)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @('Test-SynapsePathStrictlyInside', 'Get-SynapsePurgePlan', 'Invoke-SynapsePurge')) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "setup function is missing: $name" }
            Invoke-Expression $functionAst.Extent.Text
        }
        function Info([string]$Message) { }

        # Fake machine layout under a temp dir; nothing real is ever touched.
        $script:base = Join-Path ([System.IO.Path]::GetTempPath()) ("synapse-181-" + [guid]::NewGuid().ToString('N'))
        $script:fakeHome = Join-Path $script:base 'home'
        $script:appData = Join-Path $script:fakeHome 'AppData\Roaming\synapse'
        $script:localData = Join-Path $script:fakeHome 'AppData\Local\synapse'
        $script:profiles = Join-Path $script:fakeHome '.cargo\bin\profiles'
        $script:roots = @($script:appData, $script:localData)
        $script:exact = @($script:profiles)

        function New-FakeMachine {
            if (Test-Path -LiteralPath $script:base) { Remove-Item -LiteralPath $script:base -Recurse -Force }
            foreach ($d in @($script:appData, (Join-Path $script:appData 'chrome-debugger'), (Join-Path $script:localData 'db-daemon'), $script:profiles, (Join-Path $script:fakeHome 'Documents'))) {
                New-Item -ItemType Directory -Force -Path $d | Out-Null
            }
            foreach ($f in @('token.txt', 'chrome-bridge-token.txt', 'chrome-bridge-deferred-repair.txt', 'codex-tool-surface.json', 'gitea-claude-token.txt', 'synapse-mcp.stderr.log')) {
                Set-Content -LiteralPath (Join-Path $script:appData $f) -Value 'x'
            }
            Set-Content -LiteralPath (Join-Path $script:appData 'chrome-debugger\host.json') -Value 'x'
            Set-Content -LiteralPath (Join-Path $script:localData 'db-daemon\CURRENT') -Value 'x'
            Set-Content -LiteralPath (Join-Path $script:profiles 'a.toml') -Value 'x'
            Set-Content -LiteralPath (Join-Path $script:fakeHome 'Documents\keep.docx') -Value 'x'
            Set-Content -LiteralPath (Join-Path $script:fakeHome 'token.txt') -Value 'x'
            Set-Content -LiteralPath (Join-Path $script:fakeHome 'other.txt') -Value 'x'
        }
        function Invoke-TestPurge([string]$TokenPath, [string]$DbPath, [string]$ProfilesDir) {
            Invoke-SynapsePurge -DbPath $DbPath -ProfilesDir $ProfilesDir -TokenPath $TokenPath `
                -AppDataSynapseDir $script:appData -AllowedRoots $script:roots -AllowedExactDirs $script:exact -Force
        }
    }

    AfterAll {
        if (Test-Path -LiteralPath $script:base) { Remove-Item -LiteralPath $script:base -Recurse -Force }
    }

    It 'must_pass custom TokenPath under a fake HOME deletes only the token file, never HOME' {
        New-FakeMachine
        $custom = Join-Path $script:fakeHome 'token.txt'
        Invoke-TestPurge -TokenPath $custom -DbPath (Join-Path $script:localData 'db-daemon') -ProfilesDir $script:profiles
        Test-Path -LiteralPath $custom | Should Be $false
        Test-Path -LiteralPath (Join-Path $script:fakeHome 'other.txt') | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:fakeHome 'Documents\keep.docx') | Should Be $true
        Test-Path -LiteralPath $script:fakeHome | Should Be $true
    }

    It 'must_pass default layout removes only the listed Synapse files and leaves foreign files' {
        New-FakeMachine
        Invoke-TestPurge -TokenPath (Join-Path $script:appData 'token.txt') -DbPath (Join-Path $script:localData 'db-daemon') -ProfilesDir $script:profiles
        foreach ($f in @('token.txt', 'chrome-bridge-token.txt', 'chrome-bridge-deferred-repair.txt', 'codex-tool-surface.json')) {
            Test-Path -LiteralPath (Join-Path $script:appData $f) | Should Be $false
        }
        Test-Path -LiteralPath (Join-Path $script:appData 'chrome-debugger') | Should Be $false
        Test-Path -LiteralPath (Join-Path $script:localData 'db-daemon') | Should Be $false
        Test-Path -LiteralPath $script:profiles | Should Be $false
        Test-Path -LiteralPath (Join-Path $script:appData 'gitea-claude-token.txt') | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:appData 'synapse-mcp.stderr.log') | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:fakeHome 'Documents\keep.docx') | Should Be $true
    }

    It 'must_fail refuses a DbPath outside the Synapse roots and deletes nothing' {
        New-FakeMachine
        $threw = $false
        try {
            Invoke-TestPurge -TokenPath (Join-Path $script:appData 'token.txt') -DbPath (Join-Path $script:fakeHome 'Documents') -ProfilesDir $script:profiles
        } catch { $threw = $true; $msg = $_.Exception.Message }
        $threw | Should Be $true
        $msg | Should Match 'SYNAPSE_PURGE_PATH_REFUSED'
        Test-Path -LiteralPath (Join-Path $script:fakeHome 'Documents\keep.docx') | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:appData 'token.txt') | Should Be $true
        Test-Path -LiteralPath $script:profiles | Should Be $true
    }

    It 'must_fail refuses a DbPath equal to a Synapse root, a drive root, or HOME' {
        New-FakeMachine
        foreach ($bad in @($script:appData, $script:fakeHome, [System.IO.Path]::GetPathRoot($script:fakeHome))) {
            $threw = $false
            try {
                Get-SynapsePurgePlan -DbPath $bad -ProfilesDir $script:profiles -TokenPath (Join-Path $script:appData 'token.txt') `
                    -AppDataSynapseDir $script:appData -AllowedRoots $script:roots -AllowedExactDirs $script:exact | Out-Null
            } catch { $threw = $true }
            $threw | Should Be $true
        }
        Test-Path -LiteralPath (Join-Path $script:appData 'token.txt') | Should Be $true
    }

    It 'must_fail refuses a ProfilesDir outside the allowed locations' {
        New-FakeMachine
        $threw = $false
        try {
            Invoke-TestPurge -TokenPath (Join-Path $script:appData 'token.txt') -DbPath (Join-Path $script:localData 'db-daemon') -ProfilesDir $script:fakeHome
        } catch { $threw = $true }
        $threw | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:fakeHome 'other.txt') | Should Be $true
    }

    It 'must_fail refuses a TokenPath that is a directory' {
        New-FakeMachine
        $threw = $false
        try {
            Invoke-TestPurge -TokenPath $script:fakeHome -DbPath (Join-Path $script:localData 'db-daemon') -ProfilesDir $script:profiles
        } catch { $threw = $true }
        $threw | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:fakeHome 'other.txt') | Should Be $true
    }

    It 'must_fail without -Force and with -WhatIf deletes nothing' {
        New-FakeMachine
        Invoke-SynapsePurge -DbPath (Join-Path $script:localData 'db-daemon') -ProfilesDir $script:profiles `
            -TokenPath (Join-Path $script:appData 'token.txt') -AppDataSynapseDir $script:appData `
            -AllowedRoots $script:roots -AllowedExactDirs $script:exact -WhatIf
        Test-Path -LiteralPath (Join-Path $script:appData 'token.txt') | Should Be $true
        Test-Path -LiteralPath (Join-Path $script:localData 'db-daemon\CURRENT') | Should Be $true
    }
}
