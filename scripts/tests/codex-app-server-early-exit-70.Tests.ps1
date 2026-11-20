# Pester 3.4.0. Full runner entry point; no daemon, model, credentials or Codex installation used.
$ErrorActionPreference = 'Stop'

Describe 'real runner child exit-code preservation #70' {
    BeforeEach {
        $script:SavedPath70 = $env:PATH
        $script:Runner70 = Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'crates\synapse-mcp\src\server\codex_app_server_runner.ps1'
        $script:Case70 = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        [void](New-Item -ItemType Directory -Path $script:Case70)
        $script:Fake70 = Join-Path $script:Case70 'codex.ps1'
        Set-Content -LiteralPath (Join-Path $script:Case70 'prompt.txt') -Value 'must never reach a model'
        $env:PATH = $script:Case70 + ';' + $script:SavedPath70
        function Invoke-EarlyExitRunner70([int]$ChildExit) {
            Set-Content -LiteralPath $script:Fake70 -Value ('exit ' + $ChildExit) -Encoding ASCII
            (Get-Command codex.ps1).Source | Should Be $script:Fake70
            (Get-Content -Raw -LiteralPath $script:Fake70).Trim() | Should Be ('exit ' + $ChildExit)
            $args70 = @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File',$script:Runner70,
                '-SpawnId','fu5b-70','-PromptPath',(Join-Path $script:Case70 'prompt.txt'),
                '-StdoutPath',(Join-Path $script:Case70 'stdout.log'),
                '-StderrPath',(Join-Path $script:Case70 'stderr.log'),
                '-FinalMessagePath',(Join-Path $script:Case70 'final.txt'),
                '-ControlPath',(Join-Path $script:Case70 'control.json'),
                '-EventsPath',(Join-Path $script:Case70 'events.jsonl'),
                '-AppServerStdoutPath',(Join-Path $script:Case70 'app.out'),
                '-AppServerStderrPath',(Join-Path $script:Case70 'app.err'),
                '-WorkingDir',$script:Case70,'-McpUrl','http://127.0.0.1:1/mcp')
            $process70 = Start-Process powershell.exe -ArgumentList $args70 -WindowStyle Hidden -PassThru
            $null = $process70.Handle
            try {
                if (-not $process70.WaitForExit(20000)) { throw 'Runner failed to stop after early child exit' }
                $process70.WaitForExit()
                $script:RunnerExit70 = $process70.ExitCode
            } finally {
                if (-not $process70.HasExited) { $process70.Kill(); $process70.WaitForExit() }
                $process70.Dispose()
            }
            $script:Control70 = Get-Content -Raw -LiteralPath (Join-Path $script:Case70 'control.json') | ConvertFrom-Json
            $script:Error70 = Get-Content -Raw -LiteralPath (Join-Path $script:Case70 'stderr.log')
        }
    }
    AfterEach { $env:PATH = $script:SavedPath70 }

    It 'must_pass_ready_response_preserves_readiness_success' {
        $tokens70 = $null; $errors70 = $null
        $ast70 = [System.Management.Automation.Language.Parser]::ParseFile($script:Runner70, [ref]$tokens70, [ref]$errors70)
        $errors70.Count | Should Be 0
        $fn70 = $ast70.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Wait-AppServerReady' }, $true)
        Invoke-Expression $fn70.Extent.Text
        Mock Invoke-WebRequest { [pscustomobject]@{ StatusCode = 200 } }
        $result70 = Wait-AppServerReady -Url 'http://127.0.0.1:1/healthz' -TimeoutMs 1000
        $result70 | Should Be $null
        Assert-MockCalled Invoke-WebRequest -Times 1 -Exactly
    }

    It 'must_fail_nonzero_child_exit_reports_exact_real_code_in_control_and_stderr' {
        Invoke-EarlyExitRunner70 37
        $script:RunnerExit70 | Should Be 1
        $script:Control70.turn_status | Should Be 'runner_error'
        $script:Control70.last_error | Should Match '^codex app-server exited \(code 37\) before becoming ready at http://127\.0\.0\.1:[0-9]+/healthz$'
        $script:Error70 | Should Match 'SYNAPSE_CODEX_APP_SERVER_RUNNER_ERROR: codex app-server exited \(code 37\)'
        (Test-Path -LiteralPath (Join-Path $script:Case70 'final.txt')) | Should Be $false
        (Get-Process -Id ([int]$script:Control70.app_server_process_id) -ErrorAction SilentlyContinue) | Should Be $null
    }

    It 'must_fail_clean_child_exit_before_ready_reports_zero_not_blank_or_timeout' {
        Invoke-EarlyExitRunner70 0
        $script:RunnerExit70 | Should Be 1
        $script:Control70.turn_status | Should Be 'runner_error'
        $script:Control70.last_error | Should Match '^codex app-server exited \(code 0\) before becoming ready at http://127\.0\.0\.1:[0-9]+/healthz$'
        $script:Error70 | Should Match 'exited \(code 0\)'
    }
}
