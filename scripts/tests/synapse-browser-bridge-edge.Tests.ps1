$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the Microsoft Edge row (crates\synapse-browsers\browsers\edge.json)
# as the installer dispatcher and the shared row readers see it. Read-only:
# every dispatcher call passes -ResolveOnly, so no installer runs, no registry
# value is written and no browser is touched. %ProgramFiles(x86)% is redirected
# to $TestDrive for the duration of a test and restored afterwards.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-edge.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

$edgeScriptsDir = Split-Path -Parent $PSScriptRoot
. (Join-Path $edgeScriptsDir 'synapse-browser-rows.ps1')
# Host fact, read-only: when App Paths already names an existing msedge.exe,
# the dispatcher resolves it before exe_candidates, so the redirected
# %ProgramFiles(x86)% cannot decide the outcome; those two cases are skipped
# (the live check covers an installed Edge).
$edgeAppPathsRegistered = @('HKLM', 'HKCU' | Where-Object {
        $key = "${_}:\$(Get-SynapseBrowserAppPathsKey)\msedge.exe"
        $value = ([string](Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).'(default)').Trim().Trim('"')
        $value -and (Test-Path -LiteralPath $value -PathType Leaf)
    }).Count -gt 0

Describe 'Synapse browser bridge: Microsoft Edge row' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:dispatcher = Join-Path $scriptsDir 'install-synapse-browser-bridge.ps1'
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'

        $script:rowsDir = Join-Path $TestDrive 'browsers'
        New-Item -ItemType Directory -Force -Path $script:rowsDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $repoRows 'chrome.json') -Destination $script:rowsDir
        Copy-Item -LiteralPath (Join-Path $repoRows 'edge.json') -Destination $script:rowsDir

        $script:programFilesX86Name = 'ProgramFiles(x86)'
        $script:originalProgramFilesX86 = [Environment]::GetEnvironmentVariable($script:programFilesX86Name, 'Process')
        $script:zeroWidthSpace = [string][char]0x200B
    }

    AfterAll {
        [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $script:originalProgramFilesX86, 'Process')
    }

    It 'must_pass_dispatch_reads_edge_row' {
        $row = Get-SynapseBrowserRow -Browser 'edge' -BrowsersDir $script:rowsDir
        $row.family | Should Be 'chromium'
        $row.label | Should Be 'Microsoft Edge'
        @($row.process_names) -join ',' | Should Be 'msedge.exe'
        $row.app_paths_name | Should Be 'msedge.exe'
        $row.tab_target_prefix | Should Be 'edge-tab:'
        $row.health_key | Should Be 'edge_bridge'
        $row.registration_token_file | Should Be 'browser-bridge-token-edge.txt'
        $row.policy.root | Should Be 'Software\Policies\Microsoft\Edge'
        $row.policy.extension_settings_value | Should Be 'ExtensionSettings'
        $row.extension.extensions_page_url | Should Be 'edge://extensions'
        $row.extension.deploy_subdir | Should Be 'browser-extension\edge'
        $row.winget_id | Should Be 'Microsoft.Edge'
        (Get-SynapseBrowserFamilyInstaller -Row $row) | Should Be (Join-Path (Split-Path -Parent $PSScriptRoot) 'install-synapse-chrome-debugger.ps1')
    }

    It 'must_pass_edge_and_chrome_rows_coexist_with_unique_wire_ids' {
        $rows = @(Get-SynapseBrowserRows -BrowsersDir $script:rowsDir)
        $rows.Count | Should Be 2
        foreach ($field in @('id', 'tab_target_prefix', 'health_key', 'endpoint_base', 'registration_token_file')) {
            @($rows | ForEach-Object { ([string]$_.$field).ToLowerInvariant() } | Sort-Object -Unique).Count | Should Be 2
        }
        $chrome = $rows | Where-Object { $_.id -eq 'chrome' }
        $chrome.health_key | Should Be 'chrome_bridge'
        $chrome.registration_token_file | Should Be 'chrome-bridge-token.txt'
    }

    It 'must_pass_edge_bridge_config_is_generated_from_the_row' {
        $config = New-SynapseBridgeConfig -Row (Get-SynapseBrowserRow -Browser 'edge' -BrowsersDir $script:rowsDir)
        $config.browserKind | Should Be 'edge'
        $config.family | Should Be 'chromium'
        $config.expectedExtensionId | Should Be 'leoocgnkjnplbfdbklajepahofecgfbk'
        $config.tabTargetPrefix | Should Be 'edge-tab:'
        $config.policyRoot | Should Be 'Software\Policies\Microsoft\Edge'
        $suffixes = @($config.windowTitleSuffixes)
        $suffixes.Count | Should Be 2
        $suffixes[0] | Should BeExactly (' - Microsoft' + $script:zeroWidthSpace + ' Edge')
        $suffixes[1] | Should BeExactly ' - Microsoft Edge'
        $json = ConvertTo-Json -Depth 4 -InputObject $config | ConvertFrom-Json
        @($json.protectedExtensionIds).Count | Should Be 0
        @($json.windowTitleSuffixes)[0].Length | Should Be 18
    }

    It 'must_fail_edge_row_never_claims_webview2' {
        $row = Get-SynapseBrowserRow -Browser 'edge' -BrowsersDir $script:rowsDir
        @($row.process_names) -contains 'msedgewebview2.exe' | Should Be $false
        $row.app_paths_name | Should Not Be 'msedgewebview2.exe'
        foreach ($candidate in @($row.exe_candidates)) {
            [string]$candidate | Should Not Match 'EdgeWebView'
            [string]$candidate | Should Match '\\Microsoft\\Edge\\Application\\msedge\.exe$'
        }
        @($row.exe_path_markers) -join ',' | Should Be '\Microsoft\Edge\Application\'
    }

    It 'must_pass_resolve_only_plans_edge_from_program_files_x86' -Skip:$edgeAppPathsRegistered {
        $fakeProgramFiles = Join-Path $TestDrive 'pf86-installed'
        $exe = Join-Path $fakeProgramFiles 'Microsoft\Edge\Application\msedge.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $exe) | Out-Null
        Set-Content -LiteralPath $exe -Value 'fake' -Encoding ASCII
        [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $fakeProgramFiles, 'Process')
        try {
            $result = & $script:dispatcher -Browser 'edge' -BrowsersDir $script:rowsDir -ResolveOnly
            $result.ok | Should Be $true
            $result.status | Should Be 'resolved'
            $result.plan.browser | Should Be 'edge'
            $result.plan.family | Should Be 'chromium'
            $result.plan.executable | Should Be $exe
            $result.plan.executable_source_of_truth | Should Match 'browsers/edge\.json exe_candidates'
            $result.plan.health_key | Should Be 'edge_bridge'
            $result.plan.installer | Should Match 'install-synapse-chrome-debugger\.ps1$'
        } finally {
            [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $script:originalProgramFilesX86, 'Process')
            Remove-Item -LiteralPath $exe -Force
        }
    }

    It 'must_fail_edge_not_installed_reports_skip_not_error' -Skip:$edgeAppPathsRegistered {
        # Configured-host shape: EdgeUpdate + WebView2 only, msedge.exe absent.
        $emptyProgramFiles = Join-Path $TestDrive 'pf86-empty'
        New-Item -ItemType Directory -Force -Path (Join-Path $emptyProgramFiles 'Microsoft\EdgeWebView\Application') | Out-Null
        [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $emptyProgramFiles, 'Process')
        try {
            $result = & $script:dispatcher -Browser 'edge' -BrowsersDir $script:rowsDir -ResolveOnly
            $result.ok | Should Be $true
            $result.browser | Should Be 'edge'
            $result.status | Should Be 'not_installed'
            $result.reason | Should Be 'browser_executable_not_found'
            $result.remediation | Should Be 'winget install Microsoft.Edge'
            $result.plan.executable | Should Be $null
            $result.plan.health_key | Should Be 'edge_bridge'
        } finally {
            [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $script:originalProgramFilesX86, 'Process')
        }
    }

    It 'must_fail_edge_with_unset_program_files_x86_is_not_installed' -Skip:$edgeAppPathsRegistered {
        [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $null, 'Process')
        try {
            (Resolve-SynapseBrowserExecutable -Row (Get-SynapseBrowserRow -Browser 'edge' -BrowsersDir $script:rowsDir)) | Should Be $null
            $result = & $script:dispatcher -Browser 'edge' -BrowsersDir $script:rowsDir -ResolveOnly
            $result.status | Should Be 'not_installed'
        } finally {
            [Environment]::SetEnvironmentVariable($script:programFilesX86Name, $script:originalProgramFilesX86, 'Process')
        }
    }
}
