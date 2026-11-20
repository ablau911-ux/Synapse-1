$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the multi-browser bridge installer dispatcher and the
# shared browser-row readers (scripts\synapse-browser-rows.ps1). Read-only:
# -ResolveOnly never runs an installer, writes the registry or touches a browser.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-dispatch.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse multi-browser bridge dispatcher' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:dispatcher = Join-Path $scriptsDir 'install-synapse-browser-bridge.ps1'
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'
        $fixtureRow = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\tests\fixtures\testium.json'

        $script:rowsDir = Join-Path $TestDrive 'browsers'
        New-Item -ItemType Directory -Force -Path $script:rowsDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $repoRows 'chrome.json') -Destination $script:rowsDir
        Copy-Item -LiteralPath $fixtureRow -Destination $script:rowsDir

        $script:testiumRoot = Join-Path $TestDrive 'testium-root'
        $env:SYNAPSE_TESTIUM_ROOT = $script:testiumRoot
    }

    AfterAll {
        Remove-Item Env:\SYNAPSE_TESTIUM_ROOT -ErrorAction SilentlyContinue
    }

    It 'must_pass_dispatch_reads_chrome_row' {
        $row = Get-SynapseBrowserRow -Browser 'chrome' -BrowsersDir $script:rowsDir
        $row.family | Should Be 'chromium'
        $row.tab_target_prefix | Should Be 'chrome-tab:'
        $row.health_key | Should Be 'chrome_bridge'
        $row.registration_token_file | Should Be 'chrome-bridge-token.txt'
        $row.policy.root | Should Be 'Software\Policies\Google\Chrome'
        (Get-SynapseBrowserFamilyInstaller -Row $row) | Should Be (Join-Path (Split-Path -Parent $PSScriptRoot) 'install-synapse-chrome-debugger.ps1')
    }

    It 'must_pass_resolve_only_plans_an_installed_browser_without_installing' {
        $exe = Join-Path $script:testiumRoot 'Application\testium.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $exe) | Out-Null
        Set-Content -LiteralPath $exe -Value 'fake' -Encoding ASCII
        try {
            $result = & $script:dispatcher -Browser 'testium' -BrowsersDir $script:rowsDir -ResolveOnly
            $result.status | Should Be 'resolved'
            $result.plan.family | Should Be 'chromium'
            $result.plan.executable | Should Be $exe
            $result.plan.health_key | Should Be 'testium_bridge'
            $result.plan.installer | Should Match 'install-synapse-chrome-debugger\.ps1$'
        } finally {
            Remove-Item -LiteralPath $exe -Force
        }
    }

    It 'must_fail_not_installed_reports_skip_not_error' {
        $result = & $script:dispatcher -Browser 'testium' -BrowsersDir $script:rowsDir
        $result.ok | Should Be $true
        $result.status | Should Be 'not_installed'
        $result.reason | Should Be 'browser_executable_not_found'
    }

    It 'must_fail_unknown_or_invalid_browser_id' {
        { & $script:dispatcher -Browser 'nope' -BrowsersDir $script:rowsDir -ResolveOnly } | Should Throw 'SYNAPSE_BROWSER_ROW_UNKNOWN'
        { & $script:dispatcher -Browser 'Edge Browser' -BrowsersDir $script:rowsDir -ResolveOnly } | Should Throw 'SYNAPSE_BROWSER_ID_INVALID'
    }

    It 'must_pass_bridge_config_is_generated_from_the_row' {
        $chrome = Get-SynapseBrowserRow -Browser 'chrome' -BrowsersDir $script:rowsDir
        $config = New-SynapseBridgeConfig -Row $chrome
        $config.schemaVersion | Should Be 1
        $config.browserKind | Should Be 'chrome'
        $config.family | Should Be 'chromium'
        $config.expectedExtensionId | Should Be 'leoocgnkjnplbfdbklajepahofecgfbk'
        $config.tabTargetPrefix | Should Be 'chrome-tab:'
        @($config.windowTitleSuffixes)[0] | Should Be ' - Google Chrome'
        $config.policyRoot | Should Be 'Software\Policies\Google\Chrome'
        $testium = New-SynapseBridgeConfig -Row (Get-SynapseBrowserRow -Browser 'testium' -BrowsersDir $script:rowsDir)
        $testium.policyRoot | Should Be $null
        $json = ConvertTo-Json -Depth 4 -InputObject $testium | ConvertFrom-Json
        @($json.protectedExtensionIds).Count | Should Be 0
        $json.tabTargetPrefix | Should Be 'testium-tab:'
    }

    It 'must_fail_expand_path_with_unset_variable_returns_null' {
        Remove-Item Env:\SYNAPSE_BROWSER_ROWS_TEST_UNSET -ErrorAction SilentlyContinue
        (Expand-SynapseBrowserPath -Path '%SYNAPSE_BROWSER_ROWS_TEST_UNSET%\x.exe') | Should Be $null
        (Expand-SynapseBrowserPath -Path '%unterminated') | Should Be $null
        (Expand-SynapseBrowserPath -Path '%SYNAPSE_TESTIUM_ROOT%\x.exe') | Should Be (Join-Path $script:testiumRoot 'x.exe')
    }

    It 'must_fail_row_whose_id_differs_from_its_file_stem' {
        $badDir = Join-Path $TestDrive 'bad-rows'
        New-Item -ItemType Directory -Force -Path $badDir | Out-Null
        $row = Get-Content -Raw -LiteralPath (Join-Path $script:rowsDir 'testium.json') | ConvertFrom-Json
        $row.id = 'other'
        $row | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $badDir 'testium.json') -Encoding UTF8
        { Get-SynapseBrowserRows -BrowsersDir $badDir } | Should Throw 'SYNAPSE_BROWSER_ROW_ID_MISMATCH'
    }
}
