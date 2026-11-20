$ErrorActionPreference = 'Stop'

Describe 'Synapse setup native build preflight' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $preflight = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Initialize-SynapseNativeBuildEnvironment' }, $true)
        if ($null -eq $preflight) { throw 'native build preflight function is missing' }
        Invoke-Expression $preflight.Extent.Text
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }
        $savedPath = $env:Path
        $savedLibclangPath = $env:LIBCLANG_PATH
    }

    AfterAll {
        $env:Path = $savedPath
        $env:LIBCLANG_PATH = $savedLibclangPath
    }

    It 'fails before Cargo when the configured DLL is missing' {
        { Initialize-SynapseNativeBuildEnvironment -LibclangPath (Join-Path $TestDrive 'missing') } |
            Should Throw 'SYNAPSE_NATIVE_LIBCLANG_MISSING'
        $env:Path | Should Be $savedPath
        $env:LIBCLANG_PATH | Should Be $savedLibclangPath
    }

    It 'loads the configured DLL and prepares child process lookup' {
        $configured = $savedLibclangPath
        if (-not $configured -or -not (Test-Path -LiteralPath (Join-Path $configured 'libclang.dll'))) {
            Set-TestInconclusive 'Configured libclang.dll is unavailable on this host'
        }
        $dll = Initialize-SynapseNativeBuildEnvironment -LibclangPath $configured
        $dll | Should Be (Join-Path $configured 'libclang.dll')
        $env:LIBCLANG_PATH | Should Be $configured
        ($env:Path -split ';')[0] | Should Be $configured
    }
}
