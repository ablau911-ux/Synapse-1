$ErrorActionPreference = 'Stop'

# #15 raid tests (Pester 3.4.0). The REAL setup entry point is run as a child process with an
# invalid -ActiveIssue and every path argument pointed into $TestDrive. It must exit with
# SYNAPSE_ACTIVE_ISSUE_INVALID before the maintenance lock, build, staging or candidate steps.
# SAFETY: the test pre-holds the (TestDrive) maintenance lock exclusively, so even a regressed
# setup that skipped validation would stop at SYNAPSE_SETUP_MAINTENANCE_LOCK_HELD and could
# never reach build/install. The live daemon bind is never used (closed loopback port).
Describe 'Synapse setup rejects an invalid ActiveIssue before any preparation (#15)' {
    BeforeAll {
        $script:setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $script:hostExe = (Get-Process -Id $PID).Path
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $fn = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Get-SynapseNormalizedIssueRef'
            }, $true)
        if ($null -eq $fn) { throw 'setup function is missing: Get-SynapseNormalizedIssueRef' }
        Invoke-Expression $fn.Extent.Text
        function Die([string]$Message) { throw $Message }

        function Get-ClosedLoopbackBind {
            $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
            $listener.Start(); $port = $listener.LocalEndpoint.Port; $listener.Stop()
            return "127.0.0.1:$port"
        }
        function Invoke-SetupChild([string]$Case, [string]$Issue) {
            $base = Join-Path $TestDrive $Case
            New-Item -ItemType Directory -Force -Path $base | Out-Null
            $lock = Join-Path $base 'setup-maintenance.lock.json'
            [System.IO.File]::WriteAllText($lock, 'pre-held-by-test')
            $held = [System.IO.File]::Open($lock, 'Open', 'ReadWrite', 'None')
            $savedManifest = $env:SYNAPSE_SETUP_REPAIR_MANIFEST
            $savedIssue = $env:SYNAPSE_ACTIVE_ISSUE
            $env:SYNAPSE_SETUP_REPAIR_MANIFEST = $null
            $env:SYNAPSE_ACTIVE_ISSUE = $null
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try {
                $out = & $script:hostExe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $script:setupPath `
                    -ActiveIssue $Issue -MaintenanceLockPath $lock -LogDir (Join-Path $base 'logs') `
                    -ExePath (Join-Path $base 'bin\synapse-mcp.exe') -ChromeNativeHostExePath (Join-Path $base 'bin\host.exe') `
                    -CargoTarget (Join-Path $base 'target') -DbPath (Join-Path $base 'db') -ProfilesDir (Join-Path $base 'profiles') `
                    -TokenPath (Join-Path $base 'appdata\token.txt') -CodexToolSurfaceSnapshotPath (Join-Path $base 'appdata\surface.json') `
                    -SourceDir (Join-Path $base 'src') -TaskName 'SynapseRaidTestNoSuchTask15' -Bind (Get-ClosedLoopbackBind) 2>&1 | Out-String
                $code = $LASTEXITCODE
            } finally {
                $ErrorActionPreference = $old
                $held.Dispose()
                $env:SYNAPSE_SETUP_REPAIR_MANIFEST = $savedManifest
                $env:SYNAPSE_ACTIVE_ISSUE = $savedIssue
            }
            return [pscustomobject]@{ Out = $out; Code = $code; Base = $base; Lock = $lock }
        }
    }

    foreach ($bad in @('13,14', '#abc', 'https://example.invalid/issues/15')) {
        It "must_fail setup with ActiveIssue '$bad' exits before the lock, build and staging" {
            $case = 'bad-' + ([guid]::NewGuid().ToString('N').Substring(0, 8))
            $r = Invoke-SetupChild -Case $case -Issue $bad

            $r.Code | Should Not Be 0
            $r.Out | Should Match 'SYNAPSE_ACTIVE_ISSUE_INVALID'
            $r.Out | Should Not Match 'SYNAPSE_SETUP_MAINTENANCE_LOCK_HELD'
            [System.IO.File]::ReadAllText($r.Lock) | Should Be 'pre-held-by-test'
            Test-Path -LiteralPath (Join-Path $r.Base 'logs') | Should Be $false
            Test-Path -LiteralPath (Join-Path $r.Base 'target') | Should Be $false
            Test-Path -LiteralPath (Join-Path $r.Base 'bin') | Should Be $false
            Test-Path -LiteralPath (Join-Path $r.Base 'appdata') | Should Be $false
        }
    }

    It 'must_pass accepted ActiveIssue forms normalize to #<number>' {
        Get-SynapseNormalizedIssueRef -Issue '15' | Should Be '#15'
        Get-SynapseNormalizedIssueRef -Issue ' #15 ' | Should Be '#15'
        Get-SynapseNormalizedIssueRef -Issue 'https://github.com/ChrisRoyse/Synapse/issues/15' | Should Be '#15'
        Get-SynapseNormalizedIssueRef -Issue 'https://github.com/ChrisRoyse/Synapse/issues/15#issuecomment-1' | Should Be '#15'
        $null -eq (Get-SynapseNormalizedIssueRef -Issue '') | Should Be $true
    }
}
