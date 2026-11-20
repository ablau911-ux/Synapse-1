$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax). Run with:
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# Regression for #70: Get-SynapseTransportOverride must merge cleanly with the
# inherited effective Codex config (stdio connect adapter vs. HTTP table), and an old
# Codex without config/read must fail with an actionable message.

Describe 'codex_app_server_runner transport override (#70)' {
    BeforeAll {
        $runnerPath = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'crates\synapse-mcp\src\server\codex_app_server_runner.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($runnerPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in 'Get-JsonProperty', 'Get-PrivateConfigReadFailureMessage', 'Get-SynapseTransportOverride') {
            $fn = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }, $true)
            if ($null -eq $fn) { throw "$name is missing from the runner" }
            Invoke-Expression $fn.Extent.Text
        }
        $script:CodexConfigReadVerifiedVersion = '0.162.1'
        $savedBearer = $env:SYNAPSE_BEARER_TOKEN
        $env:SYNAPSE_BEARER_TOKEN = 'scoped-test-token'
        $url = 'http://127.0.0.1:7711/mcp'

        function New-Config($server) {
            return ('{"mcp_servers":{"synapse":' + $server + '}}') | ConvertFrom-Json
        }
    }

    AfterAll {
        $env:SYNAPSE_BEARER_TOKEN = $savedBearer
    }

    It 'injects HTTP url + bearer env var when no synapse server is configured' {
        $o = Get-SynapseTransportOverride -Config ('{"mcp_servers":{}}' | ConvertFrom-Json) -Url $url
        $o.url | Should Be $url
        $o.bearer_token_env_var | Should Be 'SYNAPSE_BEARER_TOKEN'
        $o.ContainsKey('args') | Should Be $false
    }

    It 'handles a config with no mcp_servers table at all' {
        $o = Get-SynapseTransportOverride -Config ('{}' | ConvertFrom-Json) -Url $url
        $o.url | Should Be $url
    }

    It 'overrides an inherited HTTP table with url + bearer env var only' {
        $cfg = New-Config '{"url":"http://127.0.0.1:7700/mcp","bearer_token_env_var":"OTHER"}'
        $o = Get-SynapseTransportOverride -Config $cfg -Url $url
        $o.url | Should Be $url
        $o.bearer_token_env_var | Should Be 'SYNAPSE_BEARER_TOKEN'
        $o.ContainsKey('args') | Should Be $false
        $o.ContainsKey('env') | Should Be $false
    }

    It 'never adds url to an inherited stdio server (merge would be invalid)' {
        $cfg = New-Config '{"command":"C:\\bin\\synapse-mcp.exe","args":["--mode","connect","--bind","127.0.0.1:7700"]}'
        $o = Get-SynapseTransportOverride -Config $cfg -Url $url
        $o.ContainsKey('url') | Should Be $false
        $o.ContainsKey('bearer_token_env_var') | Should Be $false
        ($o.args -join ' ') | Should Be '--mode connect --bind 127.0.0.1:7711'
        $o.env.SYNAPSE_BEARER_TOKEN | Should Be 'scoped-test-token'
    }

    It 'rewrites --bind=/--mode= forms and keeps other forwarded args in order' {
        $cfg = New-Config '{"command":"synapse-mcp","args":["--log","debug","--mode=connect","--bind=127.0.0.1:7700","--flag"]}'
        $o = Get-SynapseTransportOverride -Config $cfg -Url $url
        ($o.args -join ' ') | Should Be '--log debug --flag --mode connect --bind 127.0.0.1:7711'
    }

    It 'rejects a stdio command that is not synapse-mcp' {
        $cfg = New-Config '{"command":"node","args":["x.js"]}'
        { Get-SynapseTransportOverride -Config $cfg -Url $url } | Should Throw 'SYNAPSE_CODEX_TRANSPORT_INCOMPATIBLE'
    }

    It 'rejects a stdio synapse-mcp that is not --mode connect' {
        $cfg = New-Config '{"command":"synapse-mcp","args":["--mode","stdio"]}'
        { Get-SynapseTransportOverride -Config $cfg -Url $url } | Should Throw 'must use --mode connect'
    }

    It 'rejects incomplete connect arguments' {
        $cfg = New-Config '{"command":"synapse-mcp","args":["--mode","connect","--bind"]}'
        { Get-SynapseTransportOverride -Config $cfg -Url $url } | Should Throw 'incomplete connect arguments'
    }

    It 'rejects a non-/mcp or non-http endpoint for connect' {
        $cfg = New-Config '{"command":"synapse-mcp","args":["--mode","connect"]}'
        { Get-SynapseTransportOverride -Config $cfg -Url 'http://127.0.0.1:7711/other' } | Should Throw 'http://host:port/mcp'
        { Get-SynapseTransportOverride -Config $cfg -Url 'https://127.0.0.1:7711/mcp' } | Should Throw 'http://host:port/mcp'
    }

    It 'names the Codex version when config/read is not implemented' {
        $err = '{"code":-32601,"message":"method not found: config/read secret-config-text"}' | ConvertFrom-Json
        $msg = Get-PrivateConfigReadFailureMessage $err
        $msg | Should Match 'SYNAPSE_CODEX_CONFIG_READ_UNSUPPORTED'
        $msg | Should Match '0\.162\.1'
        $msg | Should Not Match 'secret-config-text'
    }

    It 'keeps other config/read failures generic and private' {
        $err = '{"code":-32603,"message":"leaks private config"}' | ConvertFrom-Json
        $msg = Get-PrivateConfigReadFailureMessage $err
        $msg | Should Be 'SYNAPSE_CODEX_CONFIG_READ_FAILED: unable to read effective configuration.'
    }
}
