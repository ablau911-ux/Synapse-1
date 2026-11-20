$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax). Run with:
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# Regression for #70 through the REAL runner process: codex_app_server_runner.ps1 runs
# end to end against fixtures\fake-codex-70\codex.ps1 (found first on PATH). The fake
# answers config/read with a synthetic effective config and records the exact
# thread/start params, so the test reads the transport Synapse really sent instead of
# calling Get-SynapseTransportOverride in isolation. No real Codex, model or network.

Describe 'codex_app_server_runner config/read merge through the real runner (#70)' {
    BeforeEach {
        $repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        $script:Runner = Join-Path $repo 'crates\synapse-mcp\src\server\codex_app_server_runner.ps1'
        $script:FakeDir = Join-Path $PSScriptRoot 'fixtures\fake-codex-70'
        $script:Url = 'http://127.0.0.1:7711/mcp'
        $script:SavedPath = $env:PATH
        $script:SavedBearer = $env:SYNAPSE_BEARER_TOKEN
        $env:PATH = $script:FakeDir + ';' + $env:PATH
        $env:SYNAPSE_BEARER_TOKEN = 'scoped-test-token-70'

        function Invoke-Runner([string]$ConfigJson, [string]$Mode = 'ok') {
            $dir = Join-Path $TestDrive ('runner70-' + [Guid]::NewGuid().ToString('N'))
            [void](New-Item -ItemType Directory -Path $dir)
            Set-Content -LiteralPath (Join-Path $dir 'prompt.txt') -Value 'never sent' -Encoding UTF8
            $env:FAKE_CODEX_CONFIG = $ConfigJson
            $env:FAKE_CODEX_MODE = $Mode
            $env:FAKE_CODEX_RECORD = Join-Path $dir 'thread-start.json'
            $runnerArgs = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $script:Runner,
                '-SpawnId', 'spawn-70-test', '-PromptPath', (Join-Path $dir 'prompt.txt'),
                '-StdoutPath', (Join-Path $dir 'stdout.log'), '-StderrPath', (Join-Path $dir 'stderr.log'),
                '-FinalMessagePath', (Join-Path $dir 'final.txt'), '-ControlPath', (Join-Path $dir 'control.json'),
                '-EventsPath', (Join-Path $dir 'events.jsonl'), '-AppServerStdoutPath', (Join-Path $dir 'app.out'),
                '-AppServerStderrPath', (Join-Path $dir 'app.err'), '-WorkingDir', $dir, '-McpUrl', $script:Url)
            & powershell.exe @runnerArgs | Out-Null
            $exit = $LASTEXITCODE
            $control = Get-Content -Raw -LiteralPath (Join-Path $dir 'control.json') | ConvertFrom-Json
            $record = $null
            if (Test-Path -LiteralPath $env:FAKE_CODEX_RECORD) {
                $record = Get-Content -Raw -LiteralPath $env:FAKE_CODEX_RECORD | ConvertFrom-Json
            }
            return [pscustomobject]@{ Exit = $exit; Control = $control; Record = $record; Dir = $dir }
        }

        function Assert-AppServerGone($Result) {
            $appPid = [int]$Result.Control.app_server_process_id
            ($appPid -gt 0) | Should Be $true
            (Get-Process -Id $appPid -ErrorAction SilentlyContinue) | Should Be $null
        }
    }

    AfterEach {
        $env:PATH = $script:SavedPath
        $env:SYNAPSE_BEARER_TOKEN = $script:SavedBearer
        Remove-Item Env:FAKE_CODEX_CONFIG, Env:FAKE_CODEX_MODE, Env:FAKE_CODEX_RECORD -ErrorAction SilentlyContinue
    }

    It 'precondition: the fake codex.ps1 is what the runner resolves' {
        (Get-Command codex.ps1).Source | Should Be (Join-Path $script:FakeDir 'codex.ps1')
    }

    It 'must_pass inherited stdio synapse-mcp connect gets args/env only, never url' {
        $cfg = '{"mcp_servers":{"synapse":{"command":"C:\\bin\\synapse-mcp.exe","args":["--mode","connect","--bind","127.0.0.1:7700"]}}}'
        $r = Invoke-Runner $cfg
        $synapse = $r.Record.config.mcp_servers.synapse
        $synapse | Should Not Be $null
        ($synapse.PSObject.Properties.Name -contains 'url') | Should Be $false
        ($synapse.PSObject.Properties.Name -contains 'bearer_token_env_var') | Should Be $false
        ($synapse.args -join ' ') | Should Be '--mode connect --bind 127.0.0.1:7711'
        $synapse.env.SYNAPSE_BEARER_TOKEN | Should Be 'scoped-test-token-70'
        $r.Control.last_error | Should Match 'fake codex stops after thread/start'
        Assert-AppServerGone $r
    }

    It 'must_pass inherited HTTP-only config gets the spawn url and bearer env var' {
        $r = Invoke-Runner '{"mcp_servers":{"synapse":{"url":"http://127.0.0.1:7700/mcp"}}}'
        $synapse = $r.Record.config.mcp_servers.synapse
        $synapse.url | Should Be $script:Url
        $synapse.bearer_token_env_var | Should Be 'SYNAPSE_BEARER_TOKEN'
        ($synapse.PSObject.Properties.Name -contains 'args') | Should Be $false
        Assert-AppServerGone $r
    }

    It 'must_fail a non-synapse stdio command stops before thread/start' {
        $r = Invoke-Runner '{"mcp_servers":{"synapse":{"command":"cmd.exe","args":["/c","exit"]}}}'
        $r.Exit | Should Be 1
        $r.Record | Should Be $null
        $r.Control.turn_status | Should Be 'runner_error'
        $r.Control.last_error | Should Be 'SYNAPSE_CODEX_TRANSPORT_INCOMPATIBLE: inherited synapse stdio command is not synapse-mcp.'
        Assert-AppServerGone $r
    }

    It 'must_fail synapse-mcp in --mode stdio stops before thread/start' {
        $r = Invoke-Runner '{"mcp_servers":{"synapse":{"command":"synapse-mcp","args":["--mode","stdio"]}}}'
        $r.Exit | Should Be 1
        $r.Record | Should Be $null
        $r.Control.last_error | Should Be 'SYNAPSE_CODEX_TRANSPORT_INCOMPATIBLE: inherited synapse command must use --mode connect.'
    }

    It 'must_fail a Codex without config/read stops with the actionable message' {
        $cfg = '{"mcp_servers":{"synapse":{"command":"synapse-mcp","args":["--mode","connect","--bind","127.0.0.1:7700"]}}}'
        $r = Invoke-Runner $cfg 'no_config_read'
        $r.Exit | Should Be 1
        $r.Record | Should Be $null
        $r.Control.last_error | Should Match '^SYNAPSE_CODEX_CONFIG_READ_UNSUPPORTED'
        Assert-AppServerGone $r
        # Follow the upgrade remediation against the same inherited config. The
        # retry must retain stdio transport rather than creating a URL/command mix.
        $retry = Invoke-Runner $cfg 'ok'
        $retry.Record | Should Not Be $null
        $synapse = $retry.Record.config.mcp_servers.synapse
        ($synapse.PSObject.Properties.Name -contains 'url') | Should Be $false
        ($synapse.args -join ' ') | Should Be '--mode connect --bind 127.0.0.1:7711'
        Assert-AppServerGone $retry
    }
}
