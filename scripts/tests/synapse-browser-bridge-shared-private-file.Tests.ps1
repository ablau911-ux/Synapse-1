$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the shared private-file helpers and the token-only
# provisioning mode the gecko installer calls:
#   - scripts\synapse-private-file.ps1 owns New-PrivateFileSecurity,
#     Assert-PrivateFilePermissions, Set-PrivateFilePermissions and
#     Write-PrivateAtomicFile; provision-synapse-chrome-auth.ps1 dot-sources it
#     (behaviour unchanged);
#   - provision-synapse-chrome-auth.ps1 -TokenOnly creates or validates the
#     registration token file and returns token_path, never touching an
#     extension dir, manifest or bridge-auth.json.
# Every file lives in TestDrive (-TokenPath is always explicit).
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-shared-private-file.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse shared private-file helpers and token-only provisioning' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:provision = Join-Path $scriptsDir 'provision-synapse-chrome-auth.ps1'
        $script:privateFile = Join-Path $scriptsDir 'synapse-private-file.ps1'
        $script:helperNames = @(
            'New-PrivateFileSecurity',
            'Assert-PrivateFilePermissions',
            'Set-PrivateFilePermissions',
            'Write-PrivateAtomicFile'
        )
        function Get-ScriptAst {
            param([string]$Path)
            $tokens = $null
            $errors = $null
            $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
            if ($errors.Count -gt 0) { throw ($errors | Out-String) }
            return $ast
        }
        function Get-DefinedFunctionNames {
            param([System.Management.Automation.Language.Ast]$Ast)
            @($Ast.FindAll({
                        param($node)
                        $node -is [System.Management.Automation.Language.FunctionDefinitionAst]
                    }, $true) | ForEach-Object { $_.Name })
        }
    }

    It 'must_pass_private_file_helpers_live_in_the_shared_script' {
        $helperAst = Get-ScriptAst -Path $script:privateFile
        $defined = Get-DefinedFunctionNames -Ast $helperAst
        foreach ($name in $script:helperNames) {
            @($defined) -contains $name | Should Be $true
        }

        $provisionAst = Get-ScriptAst -Path $script:provision
        $provisionDefined = Get-DefinedFunctionNames -Ast $provisionAst
        foreach ($name in $script:helperNames) {
            @($provisionDefined) -contains $name | Should Be $false
        }
        $dotSources = @($provisionAst.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.CommandAst] -and
                    $node.InvocationOperator -eq [System.Management.Automation.Language.TokenKind]::Dot -and
                    $node.Extent.Text -match "synapse-private-file\.ps1"
                }, $true))
        $dotSources.Count | Should Be 1
    }

    It 'must_pass_dot_sourced_helpers_write_a_private_file' {
        . $script:privateFile
        foreach ($name in $script:helperNames) {
            (Get-Command -Name $name -CommandType Function -ErrorAction SilentlyContinue) | Should Not BeNullOrEmpty
        }
        $path = Join-Path $TestDrive 'helpers\private.txt'
        Write-PrivateAtomicFile -Path $path -Content 'secret-value' -Replace $false
        [System.IO.File]::ReadAllText($path) | Should BeExactly 'secret-value'
        { Assert-PrivateFilePermissions -Path $path } | Should Not Throw
        Write-PrivateAtomicFile -Path $path -Content 'second-value' -Replace $true
        [System.IO.File]::ReadAllText($path) | Should BeExactly 'second-value'
    }

    It 'must_fail_assert_private_permissions_rejects_an_inherited_dacl' {
        . $script:privateFile
        $path = Join-Path $TestDrive 'helpers\inherited.txt'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
        [System.IO.File]::WriteAllText($path, 'not private')
        { Assert-PrivateFilePermissions -Path $path } | Should Throw 'SYNAPSE_CHROME_BRIDGE_AUTH_PERMISSIONS_INVALID'
    }

    It 'must_pass_token_only_creates_the_token_and_returns_its_path' {
        $tokenPath = Join-Path $TestDrive 'appdata\synapse\browser-bridge-token-fixture.txt'
        $extensionDir = Join-Path $TestDrive 'token-only-ext'
        New-Item -ItemType Directory -Force -Path $extensionDir | Out-Null

        $result = & $script:provision -TokenOnly -TokenPath $tokenPath -ExtensionDir $extensionDir
        $result.token_path | Should Be ([System.IO.Path]::GetFullPath($tokenPath))
        $result.token_only | Should Be $true
        $token = [System.IO.File]::ReadAllText($tokenPath).Trim()
        $token | Should Match '\A[0-9a-f]{64}\z'
        # Token only: no manifest is read and no bridge-auth.json is written.
        (Test-Path -LiteralPath (Join-Path $extensionDir 'bridge-auth.json')) | Should Be $false
        @($result.PSObject.Properties.Name) -contains 'extension_config_path' | Should Be $false

        # Second run validates the existing token and never rotates it.
        $again = & $script:provision -TokenOnly -TokenPath $tokenPath
        $again.token_path | Should Be ([System.IO.Path]::GetFullPath($tokenPath))
        [System.IO.File]::ReadAllText($tokenPath).Trim() | Should BeExactly $token
    }

    It 'must_fail_token_only_refuses_an_invalid_existing_token_without_replacing_it' {
        $tokenPath = Join-Path $TestDrive 'appdata\synapse\browser-bridge-token-invalid.txt'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $tokenPath) | Out-Null
        foreach ($bad in @('', 'not-a-token', ('AB' * 32), ('ab' * 31))) {
            [System.IO.File]::WriteAllText($tokenPath, $bad)
            { & $script:provision -TokenOnly -TokenPath $tokenPath } | Should Throw 'SYNAPSE_CHROME_BRIDGE_AUTH_INVALID'
            [System.IO.File]::ReadAllText($tokenPath) | Should BeExactly $bad
        }
    }

    It 'must_fail_full_mode_still_requires_an_extension_dir' {
        $tokenPath = Join-Path $TestDrive 'appdata\synapse\browser-bridge-token-full.txt'
        { & $script:provision -TokenPath $tokenPath } | Should Throw 'SYNAPSE_CHROME_BRIDGE_AUTH_EXTENSION_DIR_MISSING'
        # Refused before any credential is created.
        (Test-Path -LiteralPath $tokenPath) | Should Be $false
    }
}
