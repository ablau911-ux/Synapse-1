$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the row-declared profile layout (Opera GX report):
#   single -> the user-data root itself is the one profile (Preferences at root);
#   multi  -> child profile dirs except Snapshots (Chrome parity, unchanged);
#   other  -> SYNAPSE_BROWSER_PROFILE_LAYOUT_UNSUPPORTED.
# Covers the shared readers (scripts\synapse-browser-rows.ps1) and the installer
# call sites that consume them. Read-only: fixtures live in TestDrive.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-shared-profile-layout.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse browser profile layout (single / multi)' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'
        $script:chromeRow = Get-SynapseBrowserRow -Browser 'chrome' -BrowsersDir $repoRows
        $script:gxRow = Get-SynapseBrowserRow -Browser 'opera_gx' -BrowsersDir $repoRows
        $script:firefoxRow = Get-SynapseBrowserRow -Browser 'firefox' -BrowsersDir $repoRows
        $script:extensionId = 'leoocgnkjnplbfdbklajepahofecgfbk'

        $script:installerPath = Join-Path $scriptsDir 'install-synapse-chrome-debugger.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($script:installerPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @(
                'Get-SynapseChromeBridgeManifestApiPermissions',
                'Test-SynapseChromeBridgeProfileRow',
                'Get-SynapseActiveChromeProfileName'
            )) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "installer function is missing: $name" }
            Invoke-Expression $functionAst.Extent.Text
        }

        # Unpacked extension dir the Preferences rows point at.
        $script:extensionDir = Join-Path $TestDrive 'ext'
        New-Item -ItemType Directory -Force -Path $script:extensionDir | Out-Null
        '{"manifest_version":3,"permissions":["tabs"]}' | Set-Content -LiteralPath (Join-Path $script:extensionDir 'manifest.json') -Encoding UTF8
        $installedPrefs = [ordered]@{
            extensions = [ordered]@{
                settings = [ordered]@{
                    $script:extensionId = [ordered]@{
                        path = $script:extensionDir
                        active_permissions = [ordered]@{ api = @('tabs') }
                    }
                }
            }
        } | ConvertTo-Json -Depth 8

        # Single layout (Opera GX): Preferences at the user-data root.
        $script:singleRoot = Join-Path $TestDrive 'Opera Software\Opera GX Stable'
        New-Item -ItemType Directory -Force -Path $script:singleRoot | Out-Null
        $installedPrefs | Set-Content -LiteralPath (Join-Path $script:singleRoot 'Preferences') -Encoding UTF8
        $script:singleSecureOnlyRoot = Join-Path $TestDrive 'Opera Software\Opera Stable'
        New-Item -ItemType Directory -Force -Path $script:singleSecureOnlyRoot | Out-Null
        '{}' | Set-Content -LiteralPath (Join-Path $script:singleSecureOnlyRoot 'Secure Preferences') -Encoding UTF8
        $script:singleEmptyRoot = Join-Path $TestDrive 'Opera Software\Opera Empty'
        New-Item -ItemType Directory -Force -Path $script:singleEmptyRoot | Out-Null

        # Multi layout (Chrome): profile dirs under the user-data root.
        $script:multiRoot = Join-Path $TestDrive 'Chrome\User Data'
        foreach ($dir in @('Default', 'Profile 1', 'Snapshots', 'Crashpad')) {
            New-Item -ItemType Directory -Force -Path (Join-Path $script:multiRoot $dir) | Out-Null
        }
        $installedPrefs | Set-Content -LiteralPath (Join-Path $script:multiRoot 'Default\Preferences') -Encoding UTF8
        '{}' | Set-Content -LiteralPath (Join-Path $script:multiRoot 'Profile 1\Secure Preferences') -Encoding UTF8
        '{}' | Set-Content -LiteralPath (Join-Path $script:multiRoot 'Snapshots\Preferences') -Encoding UTF8
        '{"profile":{"last_used":"Default"}}' | Set-Content -LiteralPath (Join-Path $script:multiRoot 'Local State') -Encoding UTF8

        function ConvertTo-FullPath { param([string]$Path) [System.IO.Path]::GetFullPath($Path).TrimEnd('\') }
    }

    It 'must_pass_single_layout_profile_dirs_returns_root' {
        $dirs = @(Get-SynapseBrowserProfileDirs -Row $script:gxRow -UserDataRoot $script:singleRoot)
        $dirs.Count | Should Be 1
        (ConvertTo-FullPath $dirs[0].FullName) | Should Be (ConvertTo-FullPath $script:singleRoot)
        $dirs[0].Name | Should Be 'Opera GX Stable'

        $secureOnly = @(Get-SynapseBrowserProfileDirs -Row $script:gxRow -UserDataRoot $script:singleSecureOnlyRoot)
        $secureOnly.Count | Should Be 1
        (ConvertTo-FullPath $secureOnly[0].FullName) | Should Be (ConvertTo-FullPath $script:singleSecureOnlyRoot)

        # Boundary: a root without any preference file / a missing root has no profile.
        @(Get-SynapseBrowserProfileDirs -Row $script:gxRow -UserDataRoot $script:singleEmptyRoot).Count | Should Be 0
        @(Get-SynapseBrowserProfileDirs -Row $script:gxRow -UserDataRoot (Join-Path $TestDrive 'missing-root')).Count | Should Be 0

        $resolved = Resolve-SynapseBrowserProfilePath -Row $script:gxRow -UserDataRoot $script:singleRoot -ProfileName 'Opera GX Stable'
        (ConvertTo-FullPath $resolved) | Should Be (ConvertTo-FullPath $script:singleRoot)
    }

    It 'must_pass_multi_layout_unchanged' {
        $dirs = @(Get-SynapseBrowserProfileDirs -Row $script:chromeRow -UserDataRoot $script:multiRoot)
        # Snapshots is excluded, Crashpad holds no preference file; sorted by name.
        @($dirs | ForEach-Object { $_.Name }) -join '|' | Should Be 'Default|Profile 1'
        (ConvertTo-FullPath $dirs[0].FullName) | Should Be (ConvertTo-FullPath (Join-Path $script:multiRoot 'Default'))
        @(Get-SynapseBrowserProfileDirs -Row $script:chromeRow -UserDataRoot (Join-Path $TestDrive 'missing-root')).Count | Should Be 0

        $resolved = Resolve-SynapseBrowserProfilePath -Row $script:chromeRow -UserDataRoot $script:multiRoot -ProfileName 'Profile 1'
        $resolved | Should Be (Join-Path $script:multiRoot 'Profile 1')
    }

    It 'must_fail_unknown_layout_throws' {
        $bogus = [pscustomobject]@{ id = 'fixture_bogus'; profile_layout = 'bogus' }
        $missing = [pscustomobject]@{ id = 'fixture_missing' }
        foreach ($row in @($script:firefoxRow, $bogus, $missing)) {
            { Get-SynapseBrowserProfileDirs -Row $row -UserDataRoot $script:multiRoot } | Should Throw 'SYNAPSE_BROWSER_PROFILE_LAYOUT_UNSUPPORTED'
            { Resolve-SynapseBrowserProfilePath -Row $row -UserDataRoot $script:multiRoot -ProfileName 'Default' } | Should Throw 'SYNAPSE_BROWSER_PROFILE_LAYOUT_UNSUPPORTED'
        }
    }

    It 'must_pass_single_layout_installer_reads_root_preferences' {
        $script:BrowserRow = $script:gxRow
        $row = Test-SynapseChromeBridgeProfileRow `
            -ChromeUserDataRoot $script:singleRoot `
            -ProfileName 'Opera GX Stable' `
            -ExtensionId $script:extensionId `
            -ExtensionDir $script:extensionDir
        $row.installed | Should Be $true
        $row.manifest_path_matches | Should Be $true
        (ConvertTo-FullPath $row.pref_path) | Should Be (ConvertTo-FullPath (Join-Path $script:singleRoot 'Preferences'))

        $active = Get-SynapseActiveChromeProfileName `
            -ChromeUserDataRoot $script:singleRoot `
            -ExtensionId $script:extensionId `
            -ExtensionDir $script:extensionDir
        $active | Should Be 'Opera GX Stable'
    }

    It 'must_pass_multi_layout_installer_unchanged' {
        $script:BrowserRow = $script:chromeRow
        $row = Test-SynapseChromeBridgeProfileRow `
            -ChromeUserDataRoot $script:multiRoot `
            -ProfileName 'Default' `
            -ExtensionId $script:extensionId `
            -ExtensionDir $script:extensionDir
        $row.installed | Should Be $true
        $row.manifest_path_matches | Should Be $true
        $row.pref_path | Should Be (Join-Path (Join-Path $script:multiRoot 'Default') 'Preferences')

        $absent = Test-SynapseChromeBridgeProfileRow `
            -ChromeUserDataRoot $script:multiRoot `
            -ProfileName 'Profile 1' `
            -ExtensionId $script:extensionId `
            -ExtensionDir $script:extensionDir
        $absent.installed | Should Be $false

        $active = Get-SynapseActiveChromeProfileName `
            -ChromeUserDataRoot $script:multiRoot `
            -ExtensionId $script:extensionId `
            -ExtensionDir $script:extensionDir
        $active | Should Be 'Default'
    }

    It 'must_fail_installer_never_enumerates_or_joins_the_user_data_root_directly' {
        # Guard: every profile enumeration / profile path in the installer goes
        # through the layout-aware readers, so a single-layout root can never be
        # scanned as if it held child profile dirs.
        $text = Get-Content -Raw -LiteralPath $script:installerPath
        $text | Should Not Match 'Get-ChildItem\s+-LiteralPath\s+\$[cC]hromeUserDataRoot'
        $text | Should Not Match 'Join-Path\s+\$ChromeUserDataRoot\s+\$ProfileName'
        ([regex]::Matches($text, 'Get-SynapseBrowserProfileDirs')).Count | Should Be 2
        ([regex]::Matches($text, 'Resolve-SynapseBrowserProfilePath')).Count | Should Be 1
    }
}
