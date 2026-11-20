$ErrorActionPreference = 'Stop'
# Pester 3.4 config guard; only fixture copy in TestDrive is parsed. No docker
# daemon, installation, environment mutation, or real credential required.
Describe 'Windows compose local-only authentication contract #254' {
    BeforeAll {
        $repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        $fixture = Join-Path $TestDrive 'docker-compose.windows.yml'
        Copy-Item -LiteralPath (Join-Path $repo 'docker-compose.windows.yml') -Destination $fixture
        $script:Compose254 = [IO.File]::ReadAllText($fixture)
        $script:Original254 = $script:Compose254
    }

    It 'must_pass_host_publication_is_localhost_only' {
        $script:Compose254 | Should Match '(?m)^\s*-\s*"127\.0\.0\.1:7700:7700"\s*$'
        $script:Compose254 | Should Match '(?m)^\s*SYNAPSE_MODE:\s*http\s*$'
        $script:Compose254 | Should Match '(?m)^\s*SYNAPSE_BIND:\s*0\.0\.0\.0:7700\s*$'
    }

    It 'must_fail_missing_or_empty_token_has_no_default_fallback' {
        $script:Compose254 | Should Match '\$\{SYNAPSE_BEARER_TOKEN:\?[^}]+\}'
        $script:Compose254 | Should Not Match 'synapse-local-dev-token'
        $script:Compose254 | Should Not Match '\$\{SYNAPSE_BEARER_TOKEN[:-]-'
        $script:Compose254 | Should Be $script:Original254
    }

    It 'must_fail_unrestricted_host_publication_is_absent' {
        $script:Compose254 | Should Not Match '(?m)^\s*-\s*"(?:7700:7700|0\.0\.0\.0:7700:7700|\[::\]:7700:7700)"'
        $script:Compose254 | Should Be $script:Original254
    }

    It 'must_pass_healthcheck_preserves_auth_timeout_and_retries' {
        $script:Compose254 | Should Match 'Authorization = .*Bearer \$\$env:SYNAPSE_BEARER_TOKEN'
        $script:Compose254 | Should Match 'Origin = .*http://localhost:7700'
        $script:Compose254 | Should Match 'Invoke-WebRequest -UseBasicParsing -Uri http://localhost:7700/health -Headers \$\$headers -TimeoutSec 5'
        foreach ($pair in @('interval: 30s','timeout: 10s','retries: 5','start_period: 20s')) {
            $script:Compose254.Contains($pair) | Should Be $true
        }
    }
}
