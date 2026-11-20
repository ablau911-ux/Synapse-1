$ErrorActionPreference = 'Stop'

# #65 raid tests (Pester 3.4.0). The supported refresh command `synapse-setup.ps1
# -RefreshToolSurfaceOnly` is run as a REAL child process with every path in $TestDrive and a
# closed loopback bind. Failure paths must fail closed and leave the existing snapshot
# byte-identical. The acceptance gate follows the daemon registry authority, read here from the
# Rust source of truth (PUBLIC_TOOL_LIMIT in crates/synapse-mcp/src/server/tool_profiles.rs).
# SAFETY INTERLOCK: the child runs only if the AST still shows the refresh branch returning
# before the token/install part of setup (which writes the user SYNAPSE_BEARER_TOKEN).
Describe 'Synapse -RefreshToolSurfaceOnly command fails closed (#65)' {
    BeforeAll {
        $script:setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $script:repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        $script:hostExe = (Get-Process -Id $PID).Path
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @('Assert-SynapseRefreshToolSurface', 'Write-SynapseCodexToolSurfaceSnapshot')) {
            $fn = $script:ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
                }, $true)
            if ($null -eq $fn) { throw "setup function is missing: $name" }
            Invoke-Expression $fn.Extent.Text
        }
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }

        function Assert-RefreshBranchInterlock {
            $text = $script:ast.Extent.Text
            $branch = $text.IndexOf('if ($RefreshToolSurfaceOnly) {')
            $tokenStep = $text.IndexOf('Step "Bearer token + data dirs"')
            ($branch -gt 0) | Should Be $true
            ($tokenStep -gt $branch) | Should Be $true
            $text.Substring($branch, $tokenStep - $branch) | Should Match 'Release-SynapseSetupMaintenanceLock -State released\s+return'
        }
        function Get-PublicToolLimit {
            $rs = [System.IO.File]::ReadAllText((Join-Path $script:repoRoot 'crates\synapse-mcp\src\server\tool_profiles.rs'))
            $m = [regex]::Match($rs, 'pub\(crate\) const PUBLIC_TOOL_LIMIT: usize = (\d+);')
            # Pester 3.4 emits a Boolean on success; this helper must return only the limit.
            $m.Success | Should Be $true | Out-Null
            return [int]$m.Groups[1].Value
        }
        function Get-ClosedLoopbackBind {
            $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
            $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()
            return "127.0.0.1:$port"
        }
        function Invoke-Refresh([string]$Case, [switch]$WithToken, [switch]$AddPurge) {
            $base = Join-Path $TestDrive $Case
            $snap = Join-Path $base 'appdata\codex-tool-surface.json'
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $snap) | Out-Null
            [System.IO.File]::WriteAllText($snap, '{"previous":"snapshot"}')
            $token = Join-Path $base 'appdata\token.txt'
            if ($WithToken) { [System.IO.File]::WriteAllText($token, 'raid-test-token-0123456789abcdef') }
            $argv = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $script:setupPath,
                '-RefreshToolSurfaceOnly', '-TokenPath', $token, '-CodexToolSurfaceSnapshotPath', $snap,
                '-MaintenanceLockPath', (Join-Path $base 'lock\setup.lock.json'), '-LogDir', (Join-Path $base 'logs'),
                '-ExePath', (Join-Path $base 'bin\synapse-mcp.exe'), '-DbPath', (Join-Path $base 'db'),
                '-TaskName', 'SynapseRaidTestNoSuchTask65', '-Bind', (Get-ClosedLoopbackBind))
            if ($AddPurge) { $argv += '-Purge' }
            $savedManifest = $env:SYNAPSE_SETUP_REPAIR_MANIFEST
            $env:SYNAPSE_SETUP_REPAIR_MANIFEST = $null
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $out = & $script:hostExe @argv 2>&1 | Out-String; $code = $LASTEXITCODE }
            finally { $ErrorActionPreference = $old; $env:SYNAPSE_SETUP_REPAIR_MANIFEST = $savedManifest }
            return [pscustomobject]@{ Out = $out; Code = $code; Snap = $snap; Base = $base }
        }
        function New-Surface([int]$Count, [int]$DaemonPid, [string]$Canonical) {
            return [pscustomobject]@{
                daemon_pid = $DaemonPid; tool_count = $Count
                tool_surface_sha256 = ('a' * 64); tool_surface_setup_canonical_sha256 = $Canonical
            }
        }
        function New-Health([int]$Public, [int]$Max, [int]$DaemonPid) {
            return [pscustomobject]@{
                pid = $DaemonPid
                subsystems = [pscustomobject]@{ public_tool_registry = [pscustomobject]@{ detail = "source_of_truth=x public_tool_count=$Public max_public_tool_count=$Max implementation_tool_count=1" } }
            }
        }
        function Assert-RegistryRefusal([string]$Case, $Health, [string]$Reason) {
            # Pair command-level refusal with an authority-boundary refusal. The
            # historical literal-42 gate accepts all these invalid authorities.
            $snap = Join-Path $TestDrive "$Case-registry\surface.json"
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $snap) | Out-Null
            [IO.File]::WriteAllText($snap, '{"previous":"snapshot"}')
            $surface = New-Surface 42 4242 ('b' * 64)
            $message = ''
            try {
                Assert-SynapseRefreshToolSurface -Surface $surface -Health $Health -Bind '127.0.0.1:1'
            } catch { $message = $_.Exception.Message }
            ('#65 registry refusal: ' + $message) | Should Match ('^#65 registry refusal: ' + $Reason)
            [IO.File]::ReadAllText($snap) | Should Be '{"previous":"snapshot"}'
        }
    }

    It 'must_fail refresh without a token file fails closed and keeps the old snapshot' {
        Assert-RefreshBranchInterlock
        $r = Invoke-Refresh -Case 'notoken'

        $r.Code | Should Not Be 0
        $r.Out | Should Match 'SYNAPSE_TOOL_SURFACE_REFRESH_TOKEN_MISSING'
        [System.IO.File]::ReadAllText($r.Snap) | Should Be '{"previous":"snapshot"}'
        Assert-RegistryRefusal 'missing-authority' ([pscustomobject]@{ pid = 4242 }) 'SYNAPSE_TOOL_SURFACE_REFRESH_AUTHORITY_MISSING'
    }

    It 'must_fail refresh against an unreachable daemon fails closed and keeps the old snapshot' {
        Assert-RefreshBranchInterlock
        $r = Invoke-Refresh -Case 'nodaemon' -WithToken

        $r.Code | Should Not Be 0
        $r.Out | Should Match 'SYNAPSE_TOOL_SURFACE_REFRESH_HEALTH_FAILED'
        [System.IO.File]::ReadAllText($r.Snap) | Should Be '{"previous":"snapshot"}'
        Test-Path -LiteralPath (Join-Path $r.Base 'db') | Should Be $false
        Assert-RegistryRefusal 'mismatched-count' (New-Health 41 42 4242) 'SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH'
    }

    It 'must_fail refresh combined with -Purge is refused before the maintenance lock' {
        Assert-RefreshBranchInterlock
        $r = Invoke-Refresh -Case 'flags' -WithToken -AddPurge

        $r.Code | Should Not Be 0
        $r.Out | Should Match 'SYNAPSE_TOOL_SURFACE_REFRESH_FLAGS_INVALID'
        Test-Path -LiteralPath (Join-Path $r.Base 'lock') | Should Be $false
        [System.IO.File]::ReadAllText($r.Snap) | Should Be '{"previous":"snapshot"}'
        Assert-RegistryRefusal 'over-budget' (New-Health 42 41 4242) 'SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH'
    }

    It 'must_pass a surface matching the registry authority (PUBLIC_TOOL_LIMIT) is accepted and published exactly' {
        $limit = Get-PublicToolLimit
        $canonical = 'b' * 64
        $surface = New-Surface -Count $limit -DaemonPid 4242 -Canonical $canonical
        $snap = Join-Path $TestDrive 'pass\surface.json'

        $message = ''
        try { Assert-SynapseRefreshToolSurface -Surface $surface -Health (New-Health $limit $limit 4242) -Bind '127.0.0.1:1' }
        catch { $message = $_.Exception.Message }
        ('#65 registry-authoritative count accepted: ' + $message) | Should Be '#65 registry-authoritative count accepted: '
        Write-SynapseCodexToolSurfaceSnapshot -Path $snap -Surface $surface

        $back = Get-Content -Raw -LiteralPath $snap | ConvertFrom-Json
        $back.tool_count | Should Be $limit
        $back.daemon_pid | Should Be 4242
        $back.tool_surface_setup_canonical_sha256 | Should BeExactly $canonical
    }

    It 'must_fail a placeholder canonical hash is refused and the old snapshot is kept' {
        $limit = Get-PublicToolLimit
        $snap = Join-Path $TestDrive 'placeholder\surface.json'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $snap) | Out-Null
        [System.IO.File]::WriteAllText($snap, '{"previous":"snapshot"}')
        $surface = New-Surface -Count $limit -DaemonPid 4242 -Canonical '<manual-refresh-from-tools-list>'

        { Write-SynapseCodexToolSurfaceSnapshot -Path $snap -Surface $surface } | Should Throw 'SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_HASH_INVALID'
        [System.IO.File]::ReadAllText($snap) | Should Be '{"previous":"snapshot"}'
        Assert-RegistryRefusal 'malformed-authority' ([pscustomobject]@{ pid = 4242; subsystems = [pscustomobject]@{ public_tool_registry = [pscustomobject]@{ detail = 'public_tool_count=invalid max_public_tool_count=42' } } }) 'SYNAPSE_TOOL_SURFACE_REFRESH_AUTHORITY_MISSING'
    }
}
