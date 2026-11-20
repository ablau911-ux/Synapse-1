$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax). Run with:
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# Regression for #230: Wait-AppServerReady must stop at once when the app-server
# process has already exited instead of polling /healthz for the full timeout.

Describe 'codex_app_server_runner Wait-AppServerReady exit detection (#230)' {
    BeforeAll {
        $runnerPath = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'crates\synapse-mcp\src\server\codex_app_server_runner.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($runnerPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $fn = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Wait-AppServerReady' }, $true)
        if ($null -eq $fn) { throw 'Wait-AppServerReady is missing from the runner' }
        Invoke-Expression $fn.Extent.Text
    }

    It 'must_fail fails immediately when the app-server process already exited' {
        $proc = Start-Process -FilePath $env:ComSpec -ArgumentList '/c', 'exit 3' -WindowStyle Hidden -PassThru
        $proc.WaitForExit()
        $watch = [System.Diagnostics.Stopwatch]::StartNew()
        $threw = $null
        try {
            Wait-AppServerReady -Url 'http://127.0.0.1:1/healthz' -TimeoutMs 15000 -Process $proc
        } catch {
            $threw = $_.Exception.Message
        }
        $watch.Stop()
        $threw | Should Match 'exited'
        $threw | Should Match '3'
        ($watch.Elapsed.TotalSeconds -lt 5) | Should Be $true
    }

    It 'must_pass still times out on the deadline while the process is alive' {
        $proc = Start-Process -FilePath $env:ComSpec -ArgumentList '/c', 'ping -n 30 127.0.0.1 >nul' -WindowStyle Hidden -PassThru
        try {
            $threw = $null
            try {
                Wait-AppServerReady -Url 'http://127.0.0.1:1/healthz' -TimeoutMs 800 -Process $proc
            } catch {
                $threw = $_.Exception.Message
            }
            $threw | Should Match 'did not become ready'
        } finally {
            if (-not $proc.HasExited) { $proc.Kill() }
        }
    }
}
