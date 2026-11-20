$ErrorActionPreference = 'Stop'

Describe 'Synapse setup binary copy retry' {
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
        foreach ($name in @('Test-SynapseSharingViolation', 'Get-SynapseRetryDelayMs', 'Copy-SynapseFileWithSharingRetry')) {
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
    }

    It 'retries a sharing violation until the destination lock is released' {
        $source = Join-Path $TestDrive 'candidate.exe'
        $destination = Join-Path $TestDrive 'synapse-mcp.exe'
        $ready = Join-Path $TestDrive 'lock-ready'
        Set-Content -LiteralPath $source -Value 'candidate'
        Set-Content -LiteralPath $destination -Value 'old'

        $job = Start-Job -ArgumentList $destination, $ready -ScriptBlock {
            param($Path, $ReadyPath)
            $stream = [System.IO.File]::Open(
                $Path,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
            Set-Content -LiteralPath $ReadyPath -Value 'ready'
            Start-Sleep -Milliseconds 600
            $stream.Dispose()
        }
        try {
            $deadline = (Get-Date).AddSeconds(5)
            while (-not (Test-Path -LiteralPath $ready)) {
                if ((Get-Date) -ge $deadline) { throw 'lock helper did not become ready' }
                Start-Sleep -Milliseconds 25
            }
            Copy-SynapseFileWithSharingRetry `
                -Source $source `
                -Destination $destination `
                -TimeoutMs 3000 `
                -DelayMs 50
            Wait-Job -Job $job | Out-Null
        } finally {
            Stop-Job -Job $job -ErrorAction SilentlyContinue
            Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
        }

        (Get-Content -LiteralPath $destination -Raw).Trim() | Should Be 'candidate'
        ($script:messages -join "`n") | Should Match 'SYNAPSE_BINARY_COPY_SHARING_RETRY'
        ($script:messages -join "`n") | Should Match 'SYNAPSE_BINARY_COPY_RETRY_SUCCEEDED'
    }
}
