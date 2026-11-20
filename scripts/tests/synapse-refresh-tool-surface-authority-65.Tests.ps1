$ErrorActionPreference = 'Stop'

# #65: -RefreshToolSurfaceOnly must validate the running daemon's surface against the
# daemon's own registry authority (PUBLIC_TOOL_LIMIT / public registry), not a literal count.
Describe 'Synapse setup tool-surface refresh authority (#65)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $fn = $script:ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Assert-SynapseRefreshToolSurface' }, $true)
        if ($null -eq $fn) { throw 'Assert-SynapseRefreshToolSurface is missing' }
        Invoke-Expression $fn.Extent.Text
        function Die([string]$Message) { throw $Message }

        function New-Health([int]$ProcessId, [string]$Detail) {
            [pscustomobject]@{
                pid = $ProcessId
                subsystems = [pscustomobject]@{
                    public_tool_registry = [pscustomobject]@{ status = 'ok'; detail = $Detail }
                }
            }
        }
        function New-Surface([int]$ProcessId, [int]$Count) {
            [pscustomobject]@{ daemon_pid = $ProcessId; tool_count = $Count }
        }
        function New-Detail([int]$Public, [int]$Max) {
            "source_of_truth=x public_tool_count=$Public max_public_tool_count=$Max implementation_tool_count=300 registered_tools_present=$Public registered_tools_missing=0"
        }
    }

    It 'accepts the current registry count' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 43) -Health (New-Health 10 (New-Detail 43 43)) -Bind '127.0.0.1:7700' } | Should Not Throw
    }

    It 'follows the registry authority instead of a literal count' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 44) -Health (New-Health 10 (New-Detail 44 44)) -Bind '127.0.0.1:7700' } | Should Not Throw
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 41) -Health (New-Health 10 (New-Detail 41 43)) -Bind '127.0.0.1:7700' } | Should Not Throw
    }

    It 'rejects a tools/list count that disagrees with the registry' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 42) -Health (New-Health 10 (New-Detail 43 43)) -Bind '127.0.0.1:7700' } | Should Throw 'SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH'
    }

    It 'rejects a registry above its own limit' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 44) -Health (New-Health 10 (New-Detail 44 43)) -Bind '127.0.0.1:7700' } | Should Throw 'SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH'
    }

    It 'rejects an empty surface' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 0) -Health (New-Health 10 (New-Detail 0 43)) -Bind '127.0.0.1:7700' } | Should Throw 'SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH'
    }

    It 'rejects a snapshot taken from a different daemon pid' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 11 43) -Health (New-Health 10 (New-Detail 43 43)) -Bind '127.0.0.1:7700' } | Should Throw 'SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH'
    }

    It 'fails closed when the registry detail is unreadable' {
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 43) -Health (New-Health 10 'garbage') -Bind '127.0.0.1:7700' } | Should Throw 'SYNAPSE_TOOL_SURFACE_REFRESH_AUTHORITY_MISSING'
        $noSubsystem = [pscustomobject]@{ pid = 10; subsystems = [pscustomobject]@{} }
        { Assert-SynapseRefreshToolSurface -Surface (New-Surface 10 43) -Health $noSubsystem -Bind '127.0.0.1:7700' } | Should Throw 'SYNAPSE_TOOL_SURFACE_REFRESH_AUTHORITY_MISSING'
    }

    It 'has no hardcoded tool-count literal in the refresh path' {
        $setupText = Get-Content -Raw -LiteralPath (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1')
        $setupText | Should Not Match 'tool_count\s+-ne\s+\d+'
        $setupText | Should Not Match '\d+-tool surface before refreshing'
        $setupText | Should Match 'Assert-SynapseRefreshToolSurface\s+-Surface'
    }
}
