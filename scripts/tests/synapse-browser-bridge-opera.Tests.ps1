$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the Opera and Opera GX browser rows as the installer
# dispatcher reads them. Read-only: -ResolveOnly never runs an installer,
# writes the registry or touches a browser. The install-location variables
# (LOCALAPPDATA, ProgramFiles, ProgramFiles(x86)) are pointed at $TestDrive for
# this process only, so a real Opera install on the host never leaks in, and
# are restored afterwards.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-opera.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse Opera / Opera GX browser rows' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:dispatcher = Join-Path $scriptsDir 'install-synapse-browser-bridge.ps1'
        $script:chromiumInstaller = Join-Path $scriptsDir 'install-synapse-chrome-debugger.ps1'
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'

        $script:rowsDir = Join-Path $TestDrive 'browsers'
        New-Item -ItemType Directory -Force -Path $script:rowsDir | Out-Null
        foreach ($id in @('chrome', 'opera', 'opera_gx')) {
            Copy-Item -LiteralPath (Join-Path $repoRows "$id.json") -Destination $script:rowsDir
        }

        $script:envNames = @('LOCALAPPDATA', 'ProgramFiles', 'ProgramFiles(x86)')
        $script:savedEnv = @{}
        foreach ($name in $script:envNames) {
            $script:savedEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        }
        $script:fakeLocal = Join-Path $TestDrive 'Local'
        $script:fakePf = Join-Path $TestDrive 'PF'
        $script:fakePf86 = Join-Path $TestDrive 'PF86'
        [Environment]::SetEnvironmentVariable('LOCALAPPDATA', $script:fakeLocal, 'Process')
        [Environment]::SetEnvironmentVariable('ProgramFiles', $script:fakePf, 'Process')
        [Environment]::SetEnvironmentVariable('ProgramFiles(x86)', $script:fakePf86, 'Process')

        function New-FakeOperaExe {
            param([Parameter(Mandatory = $true)][string]$Path)
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
            Set-Content -LiteralPath $Path -Value 'fake' -Encoding ASCII
            return $Path
        }
    }

    AfterAll {
        foreach ($name in $script:envNames) {
            [Environment]::SetEnvironmentVariable($name, $script:savedEnv[$name], 'Process')
        }
    }

    It 'must_pass_dispatch_reads_opera_and_opera_gx_rows' {
        $opera = Get-SynapseBrowserRow -Browser 'opera' -BrowsersDir $script:rowsDir
        $gx = Get-SynapseBrowserRow -Browser 'opera_gx' -BrowsersDir $script:rowsDir
        foreach ($row in @($opera, $gx)) {
            $row.family | Should Be 'chromium'
            @($row.process_names)[0] | Should Be 'opera.exe'
            $row.app_paths_name | Should Be $null
            $row.policy | Should Be $null
            $row.extension.extensions_page_url | Should Be 'opera://extensions'
            (Get-SynapseBrowserFamilyInstaller -Row $row) | Should Be $script:chromiumInstaller
        }
        $opera.tab_target_prefix | Should Be 'opera-tab:'
        $gx.tab_target_prefix | Should Be 'operagx-tab:'
        $opera.health_key | Should Be 'opera_bridge'
        $gx.health_key | Should Be 'opera_gx_bridge'
        $opera.registration_token_file | Should Be 'browser-bridge-token-opera.txt'
        $gx.registration_token_file | Should Be 'browser-bridge-token-opera_gx.txt'
        $opera.profile_layout | Should Be 'multi'
        $gx.profile_layout | Should Be 'single'
    }

    It 'must_pass_opera_row_ids_equal_their_file_stems' {
        # Throws SYNAPSE_BROWSER_ROW_ID_MISMATCH when an id differs from its file stem.
        $ids = @(Get-SynapseBrowserRows -BrowsersDir $script:rowsDir | ForEach-Object { [string]$_.id } | Sort-Object)
        ($ids -join ',') | Should Be 'chrome,opera,opera_gx'
    }

    It 'must_fail_opera_not_installed_reports_skip_not_error' {
        foreach ($case in @(@{ id = 'opera'; winget = 'Opera.Opera' }, @{ id = 'opera_gx'; winget = 'Opera.OperaGX' })) {
            $result = & $script:dispatcher -Browser $case.id -BrowsersDir $script:rowsDir -ResolveOnly
            $result.ok | Should Be $true
            $result.status | Should Be 'not_installed'
            $result.reason | Should Be 'browser_executable_not_found'
            $result.remediation | Should Be "winget install $($case.winget)"
            $result.plan.health_key | Should Be "$($case.id)_bridge"
        }
    }

    It 'must_pass_resolve_only_plans_opera_gx_by_its_own_install_path' {
        $exe = New-FakeOperaExe -Path (Join-Path $script:fakeLocal 'Programs\Opera GX\opera.exe')
        try {
            $gx = & $script:dispatcher -Browser 'opera_gx' -BrowsersDir $script:rowsDir -ResolveOnly
            $gx.status | Should Be 'resolved'
            $gx.plan.executable | Should Be $exe
            $gx.plan.executable_source_of_truth | Should Match 'browsers/opera_gx\.json exe_candidates'
            $gx.plan.installer | Should Match 'install-synapse-chrome-debugger\.ps1$'
            $opera = & $script:dispatcher -Browser 'opera' -BrowsersDir $script:rowsDir -ResolveOnly
            $opera.status | Should Be 'not_installed'
        } finally {
            Remove-Item -LiteralPath $exe -Force
        }
    }

    It 'must_pass_resolve_only_plans_machine_wide_opera_without_claiming_gx' {
        $exe = New-FakeOperaExe -Path (Join-Path $script:fakePf 'Opera\opera.exe')
        try {
            $opera = & $script:dispatcher -Browser 'opera' -BrowsersDir $script:rowsDir -ResolveOnly
            $opera.status | Should Be 'resolved'
            $opera.plan.executable | Should Be $exe
            $gx = & $script:dispatcher -Browser 'opera_gx' -BrowsersDir $script:rowsDir -ResolveOnly
            $gx.status | Should Be 'not_installed'
        } finally {
            Remove-Item -LiteralPath $exe -Force
        }
    }

    It 'must_pass_bridge_config_for_opera_rows_has_no_policy_root' {
        $config = New-SynapseBridgeConfig -Row (Get-SynapseBrowserRow -Browser 'opera_gx' -BrowsersDir $script:rowsDir)
        $config.browserKind | Should Be 'opera_gx'
        $config.family | Should Be 'chromium'
        $config.expectedExtensionId | Should Be 'leoocgnkjnplbfdbklajepahofecgfbk'
        $config.tabTargetPrefix | Should Be 'operagx-tab:'
        @($config.windowTitleSuffixes)[0] | Should Be ' - Opera'
        $config.policyRoot | Should Be $null
        $opera = New-SynapseBridgeConfig -Row (Get-SynapseBrowserRow -Browser 'opera' -BrowsersDir $script:rowsDir)
        $opera.browserKind | Should Be 'opera'
        $opera.tabTargetPrefix | Should Be 'opera-tab:'
    }
}
