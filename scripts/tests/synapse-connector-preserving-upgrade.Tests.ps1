$ErrorActionPreference = 'Stop'

# Issue #11: a daemon upgrade must not kill live `--mode connect` stdio bridges
# (their parent MCP client cannot respawn them) and must be able to replace the
# installed image even while those bridges still execute it.
Describe 'Synapse setup preserves live connect bridges across daemon upgrade (#11)' {
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
                'Get-SynapseCommandLineArgumentValue',
                'Normalize-SynapseSetupPathForCompare',
                'Get-SynapseMcpDeployTargetMatch',
                'Test-SynapseSharingViolation',
                'Get-SynapseRetryDelayMs',
                'Copy-SynapseFileWithSharingRetry',
                'Move-SynapseFileWithSharingRetry',
                'Get-SynapseFileSha256',
                'Install-SynapseBinaryOverHeldImage')) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "setup function is missing: $name" }
            Invoke-Expression $functionAst.Extent.Text
        }
        $script:messages = @()
        function Info([string]$Message) { $script:messages += $Message }
        function Die([string]$Message) { throw $Message }
    }

    It 'does not select a connect bridge on the deploy bind as a stop target' {
        $proc = [pscustomobject]@{
            ProcessId = 4242
            CommandLine = '"C:\Users\x\.cargo\bin\synapse-mcp.exe" --mode connect --bind 127.0.0.1:7700'
        }
        $match = Get-SynapseMcpDeployTargetMatch -Process $proc -Bind '127.0.0.1:7700' -DbPath 'C:\db'
        $match.IsMatch | Should Be $false
        ($match.Rules -join ',') | Should Match 'connect_bridge_preserved'
    }

    It 'still selects the http daemon on the deploy bind' {
        $proc = [pscustomobject]@{
            ProcessId = 4343
            CommandLine = '"C:\Users\x\.cargo\bin\synapse-mcp.exe" --mode http --bind 127.0.0.1:7700'
        }
        $match = Get-SynapseMcpDeployTargetMatch -Process $proc -Bind '127.0.0.1:7700' -DbPath 'C:\db'
        $match.IsMatch | Should Be $true
    }

    It 'replaces an installed image that a live bridge still holds open' {
        $source = Join-Path $TestDrive 'candidate.exe'
        $destination = Join-Path $TestDrive 'synapse-mcp.exe'
        Set-Content -LiteralPath $source -Value 'candidate'
        # Hold the installed path the same way a live bridge does: by executing it.
        Copy-Item -LiteralPath (Join-Path $env:SystemRoot 'System32\PING.EXE') -Destination $destination
        $oldHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
        $holder = Start-Process -FilePath $destination -ArgumentList '-n','30','127.0.0.1' -WindowStyle Hidden -PassThru
        try {
            Start-Sleep -Milliseconds 1000
            $holder.HasExited | Should Be $false
            $plainOverwriteRefused = $false
            try {
                [System.IO.File]::Copy($source, $destination, $true)
            } catch {
                $plainOverwriteRefused = $true
            }
            $plainOverwriteRefused | Should Be $true
            $result = Install-SynapseBinaryOverHeldImage -Source $source -Destination $destination
            $holder.HasExited | Should Be $false
            (Get-Content -LiteralPath $destination -Raw).Trim() | Should Be 'candidate'
            $result.RetiredPath | Should Not BeNullOrEmpty
            (Get-FileHash -LiteralPath $result.RetiredPath -Algorithm SHA256).Hash | Should Be $oldHash
        } finally {
            Stop-Process -Id $holder.Id -Force -ErrorAction SilentlyContinue
            $holder.WaitForExit(5000) | Out-Null
        }
        ($script:messages -join "`n") | Should Match 'SYNAPSE_BINARY_HELD_IMAGE_RETIRED'
    }

    It 'restores the held image when the candidate cannot be installed' {
        $destination = Join-Path $TestDrive 'synapse-mcp-rollback.exe'
        Set-Content -LiteralPath $destination -Value 'old'
        $missing = Join-Path $TestDrive 'does-not-exist.exe'
        $threw = $false
        try {
            Install-SynapseBinaryOverHeldImage -Source $missing -Destination $destination | Out-Null
        } catch {
            $threw = $true
        }
        $threw | Should Be $true
        (Get-Content -LiteralPath $destination -Raw).Trim() | Should Be 'old'
    }
}
