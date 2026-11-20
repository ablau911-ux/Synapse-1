$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the Brave row through the shared row readers and the
# install-synapse-browser-bridge.ps1 dispatcher. Read-only: -ResolveOnly and
# the not-installed path never run an installer, write the registry or touch a
# browser. The install-location env vars are redirected into $TestDrive for
# this process only and restored afterwards.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-brave.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

# A real Brave registered in App Paths wins over the redirected candidates, so
# the candidate-driven cases only run on hosts without one.
$appPathsKey = 'Software\Microsoft\Windows\CurrentVersion\App Paths\brave.exe'
$braveAppPathsRegistered = (Test-Path -LiteralPath "HKLM:\$appPathsKey") -or (Test-Path -LiteralPath "HKCU:\$appPathsKey")

Describe 'Synapse Brave browser bridge row' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:dispatcher = Join-Path $scriptsDir 'install-synapse-browser-bridge.ps1'
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'

        $script:rowsDir = Join-Path $TestDrive 'browsers'
        New-Item -ItemType Directory -Force -Path $script:rowsDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $repoRows 'chrome.json') -Destination $script:rowsDir
        Copy-Item -LiteralPath (Join-Path $repoRows 'brave.json') -Destination $script:rowsDir

        $script:savedEnv = @{}
        foreach ($name in @('ProgramFiles', 'ProgramFiles(x86)', 'LOCALAPPDATA')) {
            $script:savedEnv[$name] = [Environment]::GetEnvironmentVariable($name)
            $redirect = Join-Path $TestDrive ('env-' + ($name -replace '[()]', ''))
            New-Item -ItemType Directory -Force -Path $redirect | Out-Null
            [Environment]::SetEnvironmentVariable($name, $redirect)
        }
        $script:perUserExe = Join-Path $env:LOCALAPPDATA 'BraveSoftware\Brave-Browser\Application\brave.exe'
    }

    AfterAll {
        foreach ($name in $script:savedEnv.Keys) {
            [Environment]::SetEnvironmentVariable($name, $script:savedEnv[$name])
        }
    }

    It 'must_pass_dispatch_reads_brave_row' {
        $row = Get-SynapseBrowserRow -Browser 'brave' -BrowsersDir $script:rowsDir
        $row.family | Should Be 'chromium'
        $row.tab_target_prefix | Should Be 'brave-tab:'
        $row.health_key | Should Be 'brave_bridge'
        $row.registration_token_file | Should Be 'browser-bridge-token-brave.txt'
        $row.policy.root | Should Be 'Software\Policies\BraveSoftware\Brave'
        $row.user_data_root | Should Be '%LOCALAPPDATA%\BraveSoftware\Brave-Browser\User Data'
        $row.extension.extensions_page_url | Should Be 'brave://extensions'
        $row.extension.deploy_subdir | Should Be 'browser-extension\brave'
        $row.winget_id | Should Be 'Brave.Brave'
        (Get-SynapseBrowserFamilyInstaller -Row $row) | Should Be (Join-Path (Split-Path -Parent $PSScriptRoot) 'install-synapse-chrome-debugger.ps1')
    }

    It 'must_pass_brave_and_chrome_rows_coexist_with_distinct_wire_ids' {
        $rows = @(Get-SynapseBrowserRows -BrowsersDir $script:rowsDir)
        @($rows | ForEach-Object { [string]$_.id }) -join ',' | Should Be 'brave,chrome'
        foreach ($field in @('tab_target_prefix', 'health_key', 'registration_token_file', 'endpoint_base')) {
            @($rows | ForEach-Object { [string]$_.$field } | Sort-Object -Unique).Count | Should Be 2
        }
        @($rows | Where-Object { @($_.process_names) -contains 'brave.exe' }).Count | Should Be 1
        @($rows | Where-Object { @($_.process_names) -contains 'brave.exe' })[0].id | Should Be 'brave'
    }

    It 'must_pass_resolve_only_plans_per_user_brave_without_installing' -Skip:$braveAppPathsRegistered {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $script:perUserExe) | Out-Null
        Set-Content -LiteralPath $script:perUserExe -Value 'fake' -Encoding ASCII
        try {
            $result = & $script:dispatcher -Browser 'brave' -BrowsersDir $script:rowsDir -ResolveOnly
            $result.ok | Should Be $true
            $result.status | Should Be 'resolved'
            $result.plan.browser | Should Be 'brave'
            $result.plan.family | Should Be 'chromium'
            $result.plan.executable | Should Be $script:perUserExe
            $result.plan.executable_source_of_truth | Should Match 'browsers/brave\.json exe_candidates'
            $result.plan.health_key | Should Be 'brave_bridge'
            $result.plan.installer | Should Match 'install-synapse-chrome-debugger\.ps1$'
        } finally {
            Remove-Item -LiteralPath $script:perUserExe -Force
        }
    }

    It 'must_pass_resolve_only_plans_system_brave_without_installing' -Skip:$braveAppPathsRegistered {
        $systemExe = Join-Path $env:ProgramFiles 'BraveSoftware\Brave-Browser\Application\brave.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $systemExe) | Out-Null
        Set-Content -LiteralPath $systemExe -Value 'fake' -Encoding ASCII
        try {
            $result = & $script:dispatcher -Browser 'brave' -BrowsersDir $script:rowsDir -ResolveOnly
            $result.status | Should Be 'resolved'
            $result.plan.executable | Should Be $systemExe
        } finally {
            Remove-Item -LiteralPath $systemExe -Force
        }
    }

    It 'must_fail_brave_not_installed_reports_skip_not_error' -Skip:$braveAppPathsRegistered {
        $result = & $script:dispatcher -Browser 'brave' -BrowsersDir $script:rowsDir
        $result.ok | Should Be $true
        $result.status | Should Be 'not_installed'
        $result.reason | Should Be 'browser_executable_not_found'
        $result.remediation | Should Be 'winget install Brave.Brave'
        $result.plan.executable | Should Be $null
    }

    It 'must_pass_brave_bridge_config_protects_brave_component_extension' {
        $row = Get-SynapseBrowserRow -Browser 'brave' -BrowsersDir $script:rowsDir
        $config = New-SynapseBridgeConfig -Row $row
        $config.browserKind | Should Be 'brave'
        $config.family | Should Be 'chromium'
        $config.expectedExtensionId | Should Be 'leoocgnkjnplbfdbklajepahofecgfbk'
        $config.tabTargetPrefix | Should Be 'brave-tab:'
        @($config.windowTitleSuffixes)[0] | Should Be ' - Brave'
        $config.policyRoot | Should Be 'Software\Policies\BraveSoftware\Brave'
        $json = ConvertTo-Json -Depth 4 -InputObject $config | ConvertFrom-Json
        @($json.protectedExtensionIds) -contains 'mnojpmjdmbbfmejpflffifhffcmidifd' | Should Be $true
        @($json.protectedExtensionIds) -contains 'leoocgnkjnplbfdbklajepahofecgfbk' | Should Be $false
    }

    It 'must_fail_brave_user_data_root_never_expands_to_google_chrome' {
        $row = Get-SynapseBrowserRow -Browser 'brave' -BrowsersDir $script:rowsDir
        $expanded = Expand-SynapseBrowserPath -Path ([string]$row.user_data_root)
        $expanded | Should Be (Join-Path $env:LOCALAPPDATA 'BraveSoftware\Brave-Browser\User Data')
        $expanded | Should Not Match 'Google\\Chrome'
    }
}
