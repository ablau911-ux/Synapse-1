$ErrorActionPreference = 'Stop'

Describe 'Synapse setup tool-surface snapshot writer' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $writer = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Write-SynapseCodexToolSurfaceSnapshot' }, $true)
        if ($null -eq $writer) { throw 'snapshot writer function is missing' }
        Invoke-Expression $writer.Extent.Text
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }
        $snapshotPath = Join-Path $TestDrive 'surface.json'
    }

    It 'writes a snapshot with reproducible digests' {
        $surface = [pscustomobject]@{
            daemon_pid = 1234
            tool_count = 42
            tool_surface_sha256 = 'a' * 64
            tool_surface_setup_canonical_sha256 = 'b' * 64
        }
        Write-SynapseCodexToolSurfaceSnapshot -Path $snapshotPath -Surface $surface
        $saved = Get-Content -Raw -LiteralPath $snapshotPath | ConvertFrom-Json
        $saved.tool_surface_sha256 | Should Be ('a' * 64)
        $saved.tool_surface_setup_canonical_sha256 | Should Be ('b' * 64)
    }

    It 'rejects a placeholder without overwriting the prior snapshot' {
        $before = Get-Content -Raw -LiteralPath $snapshotPath
        $surface = [pscustomobject]@{
            daemon_pid = 1234
            tool_count = 42
            tool_surface_sha256 = 'a' * 64
            tool_surface_setup_canonical_sha256 = '<manual-refresh-from-tools-list>'
        }
        { Write-SynapseCodexToolSurfaceSnapshot -Path $snapshotPath -Surface $surface } | Should Throw 'SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_HASH_INVALID'
        (Get-Content -Raw -LiteralPath $snapshotPath) | Should Be $before
    }
}
