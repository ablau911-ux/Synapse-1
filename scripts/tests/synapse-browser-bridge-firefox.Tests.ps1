$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the Firefox (gecko) bridge installer
# (scripts\install-synapse-gecko-bridge.ps1). Every case runs against
# TestDrive-redirected %LOCALAPPDATA% / %APPDATA%; nothing here writes the real
# registry, starts, stops or reads the owner's Firefox profile.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-firefox.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse Firefox (gecko) bridge installer' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:repoRoot = Split-Path -Parent $scriptsDir
        $script:installer = Join-Path $scriptsDir 'install-synapse-gecko-bridge.ps1'
        $script:dispatcher = Join-Path $scriptsDir 'install-synapse-browser-bridge.ps1'
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $script:rowsDir = Join-Path $TestDrive 'browsers'
        New-Item -ItemType Directory -Force -Path $script:rowsDir | Out-Null
        Copy-Item -LiteralPath (Join-Path $script:repoRoot 'crates\synapse-browsers\browsers\firefox.json') -Destination $script:rowsDir
        $script:savedLocalAppData = $env:LOCALAPPDATA
        $script:savedAppData = $env:APPDATA
        $env:LOCALAPPDATA = Join-Path $TestDrive 'Local'
        $env:APPDATA = Join-Path $TestDrive 'Roaming'
        New-Item -ItemType Directory -Force -Path $env:LOCALAPPDATA, $env:APPDATA | Out-Null
        . $script:installer -BrowsersDir $script:rowsDir -LibraryOnly
        $script:token = '0123456789abcdef' * 4
    }

    AfterAll {
        $env:LOCALAPPDATA = $script:savedLocalAppData
        $env:APPDATA = $script:savedAppData
    }

    It 'must_pass_dispatcher_routes_firefox_row_to_gecko_installer' {
        $row = Get-SynapseBrowserRow -Browser 'firefox' -BrowsersDir $script:rowsDir
        $row.family | Should Be 'gecko'
        (Get-SynapseBrowserFamilyInstaller -Row $row) | Should Be $script:installer
        $result = & $script:dispatcher -Browser 'firefox' -BrowsersDir $script:rowsDir -ResolveOnly
        $result.ok | Should Be $true
        @('resolved', 'not_installed') -contains $result.status | Should Be $true
        $result.plan.installer | Should Match 'install-synapse-gecko-bridge\.ps1$'
    }

    # Mocks live in their own Context: Pester 3 keeps an It-level mock until
    # the enclosing Context/Describe ends.
    Context 'plan only' {
    It 'must_pass_gecko_installer_plan_only_writes_nothing_and_never_touches_firefox' {
        Mock Stop-Process { throw 'Stop-Process must never run' }
        Mock Start-Process { throw 'Start-Process must never run' }
        Mock Set-ItemProperty { throw 'registry write in plan mode' }
        Mock New-ItemProperty { throw 'registry write in plan mode' }
        Mock New-Item { throw 'filesystem write in plan mode' }
        Mock Copy-Item { throw 'filesystem write in plan mode' }
        $before = @(Get-ChildItem -LiteralPath $env:LOCALAPPDATA, $env:APPDATA -Recurse -Force).Count
        $result = & $script:installer -BrowsersDir $script:rowsDir -PlanOnly
        $result.status | Should Be 'planned'
        $result.plan.browser | Should Be 'firefox'
        $result.plan.gecko_id | Should Be 'synapse-bridge@synapse.local'
        $result.plan.managed_storage_key | Should Be 'HKCU:\Software\Mozilla\ManagedStorage\synapse-bridge@synapse.local'
        $result.plan.token_path | Should Be (Join-Path $env:APPDATA 'synapse\browser-bridge-token-firefox.txt')
        $result.plan.stage_dir.StartsWith((Join-Path $env:LOCALAPPDATA 'synapse\browser-extension\firefox\')) | Should Be $true
        $result.plan.firefox_process_control | Should Be 'never'
        $result.plan.restart_required | Should Be $true
        @(Get-ChildItem -LiteralPath $env:LOCALAPPDATA, $env:APPDATA -Recurse -Force).Count | Should Be $before
        Assert-MockCalled Stop-Process -Times 0 -Exactly
        Assert-MockCalled Start-Process -Times 0 -Exactly
        Assert-MockCalled Set-ItemProperty -Times 0 -Exactly
        Assert-MockCalled New-Item -Times 0 -Exactly
    }
    }

    It 'must_fail_gecko_installer_refuses_token_in_xpi_payload' {
        $clean = Join-Path $TestDrive 'pkg-clean'
        New-Item -ItemType Directory -Force -Path $clean | Out-Null
        Set-Content -LiteralPath (Join-Path $clean 'manifest.json') -Value '{}' -Encoding UTF8
        { Assert-SynapseGeckoPackageHasNoSecret -PackageDir $clean -Token $script:token } | Should Not Throw

        $leaky = Join-Path $TestDrive 'pkg-leaky'
        New-Item -ItemType Directory -Force -Path (Join-Path $leaky 'adapters') | Out-Null
        Set-Content -LiteralPath (Join-Path $leaky 'adapters\x.js') -Value "const t = '$($script:token)';" -Encoding UTF8
        { Assert-SynapseGeckoPackageHasNoSecret -PackageDir $leaky -Token $script:token } | Should Throw 'SYNAPSE_GECKO_BRIDGE_SECRET_IN_PACKAGE'

        $authFile = Join-Path $TestDrive 'pkg-auth'
        New-Item -ItemType Directory -Force -Path $authFile | Out-Null
        Set-Content -LiteralPath (Join-Path $authFile 'bridge-auth.json') -Value '{}' -Encoding UTF8
        { Assert-SynapseGeckoPackageHasNoSecret -PackageDir $authFile -Token $script:token } | Should Throw 'SYNAPSE_GECKO_BRIDGE_SECRET_IN_PACKAGE'
    }

    It 'must_pass_gecko_installer_stage_assembles_unsigned_package' {
        $plan = Get-SynapseGeckoInstallPlan
        $staged = Invoke-SynapseGeckoStage -Plan $plan
        $dir = $staged.stage_dir
        foreach ($file in @('manifest.json', 'background.html', 'gecko_background.js', 'service_worker.js', 'bridge_config.js', 'bridge-config.json', 'adapters\gecko.js', 'adapters\index.js', 'adapters\chromium.js')) {
            Test-Path -LiteralPath (Join-Path $dir $file) -PathType Leaf | Should Be $true
        }
        foreach ($absent in @('bridge-auth.json', 'SIGNING.md', 'tests', 'README.md')) {
            Test-Path -LiteralPath (Join-Path $dir $absent) | Should Be $false
        }
        $manifest = Get-Content -Raw -LiteralPath (Join-Path $dir 'manifest.json') | ConvertFrom-Json
        $manifest.browser_specific_settings.gecko.id | Should Be 'synapse-bridge@synapse.local'
        $config = Get-Content -Raw -LiteralPath (Join-Path $dir 'bridge-config.json') | ConvertFrom-Json
        $config.family | Should Be 'gecko'
        $config.browserKind | Should Be 'firefox'
        $config.expectedExtensionId | Should Be 'synapse-bridge@synapse.local'
        $config.tabTargetPrefix | Should Be 'firefox-tab:'
        $worker = [System.IO.File]::ReadAllText((Join-Path $dir 'service_worker.js'))
        $worker.Contains('const BRIDGE_CODE_STAMP = "unstamped";') | Should Be $false
        $worker.Contains("const BRIDGE_CODE_STAMP = `"$($staged.code_stamp)`";") | Should Be $true
        $staged.code_stamp | Should Match '^[0-9a-f]{64}$'
        $staged.web_ext_sign_command | Should Match 'web-ext sign --channel=unlisted'
    }

    It 'must_pass_gecko_extension_settings_merge_preserves_other_entries' {
        $existing = '{"uBlock0@raymondhill.net":{"installation_mode":"normal_installed","install_url":"https://addons.mozilla.org/x.xpi"},"*":{"blocked_install_message":"x"}}'
        $merged = Merge-SynapseGeckoExtensionSettings -ExistingJson $existing -ExtensionId 'synapse-bridge@synapse.local' -InstallUrl 'file:///C:/X/bridge.xpi' | ConvertFrom-Json
        $merged.'uBlock0@raymondhill.net'.installation_mode | Should Be 'normal_installed'
        $merged.'*'.blocked_install_message | Should Be 'x'
        $merged.'synapse-bridge@synapse.local'.installation_mode | Should Be 'force_installed'
        $merged.'synapse-bridge@synapse.local'.install_url | Should Be 'file:///C:/X/bridge.xpi'
        $fresh = Merge-SynapseGeckoExtensionSettings -ExistingJson $null -ExtensionId 'synapse-bridge@synapse.local' -InstallUrl 'file:///C:/X/bridge.xpi' | ConvertFrom-Json
        @($fresh.PSObject.Properties).Count | Should Be 1
    }

    It 'must_fail_gecko_extension_settings_merge_rejects_invalid_existing_json' {
        { Merge-SynapseGeckoExtensionSettings -ExistingJson '{ broken' -ExtensionId 'synapse-bridge@synapse.local' -InstallUrl 'file:///C:/X/b.xpi' } | Should Throw 'SYNAPSE_GECKO_BRIDGE_POLICY_EXISTING_INVALID'
        { Merge-SynapseGeckoExtensionSettings -ExistingJson '[1,2]' -ExtensionId 'synapse-bridge@synapse.local' -InstallUrl 'file:///C:/X/b.xpi' } | Should Throw 'SYNAPSE_GECKO_BRIDGE_POLICY_EXISTING_INVALID'
    }

    It 'must_pass_gecko_file_url_and_managed_storage_manifest_shape' {
        ConvertTo-SynapseGeckoFileUrl -Path 'C:\Users\A B\x#1.xpi' | Should Be 'file:///C:/Users/A%20B/x%231.xpi'
        $manifest = New-SynapseGeckoManagedStorageManifest -ExtensionId 'synapse-bridge@synapse.local' -Token $script:token
        $manifest.name | Should Be 'synapse-bridge@synapse.local'
        $manifest.type | Should Be 'storage'
        $manifest.data.registrationToken | Should Be $script:token
        @($manifest.data.Keys).Count | Should Be 1
    }

    Context 'signed xpi refusal' {
    It 'must_fail_gecko_installer_signed_xpi_missing_or_not_a_zip_writes_nothing' {
        Mock Set-ItemProperty { throw 'registry write after a refused XPI' }
        Mock New-ItemProperty { throw 'registry write after a refused XPI' }
        $plan = Get-SynapseGeckoInstallPlan
        { Test-SynapseGeckoSignedXpi -Path (Join-Path $TestDrive 'missing.xpi') } | Should Throw 'SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_MISSING'
        $fake = Join-Path $TestDrive 'fake.xpi'
        Set-Content -LiteralPath $fake -Value 'not a zip' -Encoding ASCII
        { Test-SynapseGeckoSignedXpi -Path $fake } | Should Throw 'SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_INVALID'
        { Install-SynapseGeckoSignedXpi -Plan $plan -SignedXpiPath $fake -InstallMode 'manual' } | Should Throw 'SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_INVALID'
        Assert-MockCalled Set-ItemProperty -Times 0 -Exactly
        Assert-MockCalled New-ItemProperty -Times 0 -Exactly
    }
    }

    Context 'policy elevation' {
    It 'must_fail_gecko_policy_mode_requires_elevation' {
        Mock Test-SynapseGeckoElevated { $false }
        { Assert-SynapseGeckoPolicyWritable -InstallMode 'policy' } | Should Throw 'SYNAPSE_GECKO_BRIDGE_POLICY_REQUIRES_ELEVATION'
        { Assert-SynapseGeckoPolicyWritable -InstallMode 'manual' } | Should Not Throw
    }
    }

    It 'must_pass_installer_source_never_controls_firefox_process' {
        $source = Get-Content -Raw -LiteralPath $script:installer
        $source | Should Not Match '(?i)\b(Stop-Process|Start-Process|taskkill|CloseMainWindow)\b'
        $source | Should Not Match '(?i)&\s*[^\r\n]*firefox\.exe'
        $source | Should Not Match '(?i)\\Profiles\\'
    }
}
