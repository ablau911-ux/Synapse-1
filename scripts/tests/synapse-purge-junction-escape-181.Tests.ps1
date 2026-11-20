$ErrorActionPreference = 'Stop'

# #181 raid tests (Pester 3.4.0). The -Remove -Purge plan must never approve a recursive delete
# whose REAL location leaves the Synapse roots. Lexical containment is not enough: a junction
# ANYWHERE on the path (an ancestor inside an allowed root, or the named appdata dir itself)
# redirects the delete outside. Expected: refuse with SYNAPSE_PURGE_PATH_REFUSED before any delete.
# Fake machine under $TestDrive only; "outside" dirs are also inside $TestDrive.
Describe 'Synapse -Purge refuses junction escapes (#181)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @('Test-SynapsePathStrictlyInside', 'Get-SynapsePurgePlan', 'Invoke-SynapsePurge')) {
            $fn = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
                }, $true)
            if ($null -eq $fn) { throw "setup function is missing: $name" }
            Invoke-Expression $fn.Extent.Text
        }
        function Info([string]$Message) { }

        function New-Tree([string]$Path) {
            New-Item -ItemType Directory -Force -Path $Path | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $Path 'sentinel.bin'), 'sentinel')
            return $Path
        }
        function New-Machine([string]$Case) {
            $fakeHome = Join-Path $TestDrive "$Case\home"
            $m = @{
                Home = $fakeHome
                AppData = (Join-Path $fakeHome 'AppData\Roaming\synapse')
                Local = (Join-Path $fakeHome 'AppData\Local\synapse')
                Profiles = (Join-Path $fakeHome '.cargo\bin\profiles')
                Outside = (Join-Path $TestDrive "$Case\outside")
            }
            New-Item -ItemType Directory -Force -Path $m.AppData, $m.Local | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $m.AppData 'token.txt'), 'tok')
            return $m
        }
        function Invoke-Purge($M, [string]$DbPath, [string]$ProfilesDir) {
            Invoke-SynapsePurge -DbPath $DbPath -ProfilesDir $ProfilesDir -TokenPath (Join-Path $M.AppData 'token.txt') `
                -AppDataSynapseDir $M.AppData -AllowedRoots @($M.AppData, $M.Local) -AllowedExactDirs @($M.Profiles) -Force
        }
        function Get-ThrownMessage([scriptblock]$Block) {
            try { & $Block; return $null } catch { return $_.Exception.Message }
        }
        function Test-IsJunction([string]$Path) {
            return (((Get-Item -LiteralPath $Path -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
        }
    }

    It 'must_fail refuses a DbPath whose ancestor inside the allowed root is a junction to outside' {
        $m = New-Machine 'dbanc'
        New-Tree (Join-Path $m.Outside 'db') | Out-Null
        $link = Join-Path $m.Local 'linked'
        New-Item -ItemType Junction -Path $link -Target $m.Outside | Out-Null
        Test-IsJunction $link | Should Be $true
        New-Tree $m.Profiles | Out-Null

        $msg = Get-ThrownMessage { Invoke-Purge $m -DbPath (Join-Path $link 'db') -ProfilesDir $m.Profiles }

        $msg | Should Match '^SYNAPSE_PURGE_PATH_REFUSED'
        [System.IO.File]::ReadAllText((Join-Path $m.Outside 'db\sentinel.bin')) | Should Be 'sentinel'
        [System.IO.File]::ReadAllText((Join-Path $m.AppData 'token.txt')) | Should Be 'tok'
        Test-Path -LiteralPath (Join-Path $m.Profiles 'sentinel.bin') | Should Be $true
    }

    It 'must_fail refuses an exact ProfilesDir reached through a junctioned parent' {
        $m = New-Machine 'profanc'
        New-Tree (Join-Path $m.Outside 'profiles') | Out-Null
        $cargo = Join-Path $m.Home '.cargo'
        New-Item -ItemType Directory -Force -Path $cargo | Out-Null
        New-Item -ItemType Junction -Path (Join-Path $cargo 'bin') -Target $m.Outside | Out-Null
        Test-IsJunction (Join-Path $cargo 'bin') | Should Be $true
        $db = New-Tree (Join-Path $m.Local 'db-daemon')

        $msg = Get-ThrownMessage { Invoke-Purge $m -DbPath $db -ProfilesDir $m.Profiles }

        $msg | Should Match '^SYNAPSE_PURGE_PATH_REFUSED'
        [System.IO.File]::ReadAllText((Join-Path $m.Outside 'profiles\sentinel.bin')) | Should Be 'sentinel'
        [System.IO.File]::ReadAllText((Join-Path $db 'sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_fail refuses when the named appdata directory chrome-debugger is a junction' {
        $m = New-Machine 'named'
        New-Tree $m.Outside | Out-Null
        New-Item -ItemType Junction -Path (Join-Path $m.AppData 'chrome-debugger') -Target $m.Outside | Out-Null
        $db = New-Tree (Join-Path $m.Local 'db-daemon')
        New-Tree $m.Profiles | Out-Null

        $msg = Get-ThrownMessage { Invoke-Purge $m -DbPath $db -ProfilesDir $m.Profiles }

        $msg | Should Match '^SYNAPSE_PURGE_PATH_REFUSED'
        [System.IO.File]::ReadAllText((Join-Path $m.Outside 'sentinel.bin')) | Should Be 'sentinel'
        [System.IO.File]::ReadAllText((Join-Path $db 'sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_pass a junction-free layout still purges the Synapse-owned paths only' {
        $m = New-Machine 'clean'
        $db = New-Tree (Join-Path $m.Local 'db-daemon')
        New-Tree $m.Profiles | Out-Null
        New-Tree (Join-Path $m.AppData 'chrome-debugger') | Out-Null
        $foreign = New-Tree (Join-Path $m.Home 'Documents')

        Invoke-Purge $m -DbPath $db -ProfilesDir $m.Profiles

        Test-Path -LiteralPath $db | Should Be $false
        Test-Path -LiteralPath $m.Profiles | Should Be $false
        Test-Path -LiteralPath (Join-Path $m.AppData 'chrome-debugger') | Should Be $false
        Test-Path -LiteralPath (Join-Path $m.AppData 'token.txt') | Should Be $false
        [System.IO.File]::ReadAllText((Join-Path $foreign 'sentinel.bin')) | Should Be 'sentinel'
    }
}
