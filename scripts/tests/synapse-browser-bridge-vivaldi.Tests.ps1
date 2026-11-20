$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the Vivaldi browser row (crates\synapse-browsers\browsers\vivaldi.json)
# through the installer dispatcher and the shared row readers. Read-only:
# -ResolveOnly never runs an installer, writes the registry or touches a browser.
# Executable resolution runs against a TestDrive copy of the row with
# app_paths_name cleared, so a real Vivaldi install on the host (App Paths)
# can never change the outcome; the verbatim row is asserted separately.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-vivaldi.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse Vivaldi browser bridge row' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:dispatcher = Join-Path $scriptsDir 'install-synapse-browser-bridge.ps1'
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'
        $script:vivaldiUiExtensionId = 'mpognobbkildjkofajifpdfhcoklimli'

        $script:rowsDir = Join-Path $TestDrive 'browsers'
        New-Item -ItemType Directory -Force -Path $script:rowsDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $repoRows 'chrome.json') -Destination $script:rowsDir
        Copy-Item -LiteralPath (Join-Path $repoRows 'vivaldi.json') -Destination $script:rowsDir

        $script:isolatedRowsDir = Join-Path $TestDrive 'browsers-isolated'
        New-Item -ItemType Directory -Force -Path $script:isolatedRowsDir | Out-Null
        $isolated = Get-Content -Raw -LiteralPath (Join-Path $repoRows 'vivaldi.json') | ConvertFrom-Json
        $isolated.app_paths_name = $null
        $isolated | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $script:isolatedRowsDir 'vivaldi.json') -Encoding UTF8

        $script:savedLocalAppData = $env:LOCALAPPDATA
        $script:savedProgramFiles = $env:ProgramFiles
        $script:fakeLocalAppData = Join-Path $TestDrive 'LocalAppData'
        $script:fakeProgramFiles = Join-Path $TestDrive 'ProgramFiles'
        New-Item -ItemType Directory -Force -Path $script:fakeLocalAppData, $script:fakeProgramFiles | Out-Null
    }

    AfterAll {
        $env:LOCALAPPDATA = $script:savedLocalAppData
        $env:ProgramFiles = $script:savedProgramFiles
    }

    It 'must_pass_vivaldi_row_reads_with_unique_wire_ids' {
        $rows = @(Get-SynapseBrowserRows -BrowsersDir $script:rowsDir)
        $rows.Count | Should Be 2
        $row = Get-SynapseBrowserRow -Browser 'vivaldi' -BrowsersDir $script:rowsDir
        $chrome = Get-SynapseBrowserRow -Browser 'chrome' -BrowsersDir $script:rowsDir
        $row.family | Should Be 'chromium'
        $row.app_paths_name | Should Be 'vivaldi.exe'
        $row.tab_target_prefix | Should Be 'vivaldi-tab:'
        $row.health_key | Should Be 'vivaldi_bridge'
        $row.registration_token_file | Should Be 'browser-bridge-token-vivaldi.txt'
        $row.winget_id | Should Be 'Vivaldi.Vivaldi'
        $row.extension.extensions_page_url | Should Be 'vivaldi://extensions'
        $row.policy | Should Be $null
        ($row.tab_target_prefix -ceq $chrome.tab_target_prefix) | Should Be $false
        ($row.health_key -ceq $chrome.health_key) | Should Be $false
        ($row.registration_token_file -ceq $chrome.registration_token_file) | Should Be $false
        (Get-SynapseBrowserFamilyInstaller -Row $row) | Should Match 'install-synapse-chrome-debugger\.ps1$'
    }

    It 'must_fail_vivaldi_ui_extension_never_suppressed' {
        $row = Get-SynapseBrowserRow -Browser 'vivaldi' -BrowsersDir $script:rowsDir
        @($row.protected_extension_ids) -contains $script:vivaldiUiExtensionId | Should Be $true
        $config = New-SynapseBridgeConfig -Row $row
        $json = ConvertTo-Json -Depth 4 -InputObject $config | ConvertFrom-Json
        @($json.protectedExtensionIds) -contains $script:vivaldiUiExtensionId | Should Be $true
        $json.browserKind | Should Be 'vivaldi'
        $json.tabTargetPrefix | Should Be 'vivaldi-tab:'
        @($json.windowTitleSuffixes)[0] | Should Be ' - Vivaldi'
        # No ExtensionSettings policy root: the installer's policy shield is a
        # no-op for Vivaldi (row_declares_no_policy_root), so it cannot block the UI.
        $json.policyRoot | Should Be $null
    }

    It 'must_pass_resolve_only_plans_a_per_user_install_without_installing' {
        $exe = Join-Path $script:fakeLocalAppData 'Vivaldi\Application\vivaldi.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $exe) | Out-Null
        Set-Content -LiteralPath $exe -Value 'fake' -Encoding ASCII
        $env:LOCALAPPDATA = $script:fakeLocalAppData
        $env:ProgramFiles = $script:fakeProgramFiles
        try {
            $result = & $script:dispatcher -Browser 'vivaldi' -BrowsersDir $script:isolatedRowsDir -ResolveOnly
            $result.ok | Should Be $true
            $result.status | Should Be 'resolved'
            $result.plan.browser | Should Be 'vivaldi'
            $result.plan.family | Should Be 'chromium'
            $result.plan.executable | Should Be $exe
            $result.plan.executable_source_of_truth | Should Match 'exe_candidates %LOCALAPPDATA%\\Vivaldi\\Application\\vivaldi\.exe'
            $result.plan.health_key | Should Be 'vivaldi_bridge'
            $result.plan.installer | Should Match 'install-synapse-chrome-debugger\.ps1$'
        } finally {
            $env:LOCALAPPDATA = $script:savedLocalAppData
            $env:ProgramFiles = $script:savedProgramFiles
            Remove-Item -LiteralPath $exe -Force
        }
    }

    It 'must_pass_resolve_only_plans_a_system_install_without_installing' {
        $exe = Join-Path $script:fakeProgramFiles 'Vivaldi\Application\vivaldi.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $exe) | Out-Null
        Set-Content -LiteralPath $exe -Value 'fake' -Encoding ASCII
        $env:LOCALAPPDATA = $script:fakeLocalAppData
        $env:ProgramFiles = $script:fakeProgramFiles
        try {
            $result = & $script:dispatcher -Browser 'vivaldi' -BrowsersDir $script:isolatedRowsDir -ResolveOnly
            $result.status | Should Be 'resolved'
            $result.plan.executable | Should Be $exe
        } finally {
            $env:LOCALAPPDATA = $script:savedLocalAppData
            $env:ProgramFiles = $script:savedProgramFiles
            Remove-Item -LiteralPath $exe -Force
        }
    }

    It 'must_pass_not_installed_reports_skip_not_error' {
        $env:LOCALAPPDATA = $script:fakeLocalAppData
        $env:ProgramFiles = $script:fakeProgramFiles
        try {
            # -ResolveOnly: not_installed is decided before it, and the flag
            # guarantees no installer can ever run from this test.
            $result = & $script:dispatcher -Browser 'vivaldi' -BrowsersDir $script:isolatedRowsDir -ResolveOnly
            $result.ok | Should Be $true
            $result.status | Should Be 'not_installed'
            $result.reason | Should Be 'browser_executable_not_found'
            $result.remediation | Should Be 'winget install Vivaldi.Vivaldi'
            $result.plan.executable | Should Be $null
        } finally {
            $env:LOCALAPPDATA = $script:savedLocalAppData
            $env:ProgramFiles = $script:savedProgramFiles
        }
    }
}
