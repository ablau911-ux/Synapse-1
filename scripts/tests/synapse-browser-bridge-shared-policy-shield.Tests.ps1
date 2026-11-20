$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the shared Chromium installer popup shield
# (scripts\install-synapse-chrome-debugger.ps1):
#   - protected_extension_ids (Vivaldi UI, Brave Wallet) are never hazards and
#     never policy-blocked (Vivaldi + Brave reports);
#   - the shield never writes runtime_blocked_hosts, nor a debugger block, for
#     the bridge's own id or for "*" (Edge 154 report: either one disables
#     chrome.debugger.attach for the bridge).
# Read-only: the registry cmdlets the shield calls are replaced by in-memory
# stubs in this Describe scope, so no policy key is ever created or written.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-shared-policy-shield.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse shared Chromium installer popup shield' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        . (Join-Path $scriptsDir 'synapse-browser-rows.ps1')
        $repoRows = Join-Path (Split-Path -Parent $scriptsDir) 'crates\synapse-browsers\browsers'
        $script:braveRow = Get-SynapseBrowserRow -Browser 'brave' -BrowsersDir $repoRows
        $script:chromeRow = Get-SynapseBrowserRow -Browser 'chrome' -BrowsersDir $repoRows
        $script:BrowserRow = $script:braveRow
        $script:braveProtectedId = 'mnojpmjdmbbfmejpflffifhffcmidifd'
        $script:bridgeId = 'leoocgnkjnplbfdbklajepahofecgfbk'
        $script:thirdPartyId = 'abcdefghijklmnopabcdefghijklmnop'

        $script:installerPath = Join-Path $scriptsDir 'install-synapse-chrome-debugger.ps1'
        $tokens = $null
        $errors = $null
        $script:installerAst = [System.Management.Automation.Language.Parser]::ParseFile(
            $script:installerPath,
            [ref]$tokens,
            [ref]$errors
        )
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @(
                'ConvertTo-CompressedJson',
                'Set-SynapseChromeExternalDebuggerPolicy',
                'Get-SynapsePolicyShieldGuardedState'
            )) {
            $functionAst = $script:installerAst.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "installer function is missing: $name" }
            # Invoke-Expression directly in BeforeAll scope so the extracted
            # function persists for the It blocks.
            Invoke-Expression $functionAst.Extent.Text
        }

        $script:SynapseChromeBlockedInstallMessage = 'Synapse blocked this extension on this host because debugger/nativeMessaging permissions can surface Chrome debugger or native-host popups during background automation.'
        $script:policySeed = [ordered]@{}
        $script:policyWrites = @()
        $script:createdKeys = @()

        # In-memory registry stubs (functions shadow the cmdlets for the
        # extracted installer functions). Defined after every real file read.
        function Get-ChromePolicyRoot { param([string]$Hive) 'HKCU:\Software\SynapsePolicyShieldTest\NeverWritten' }
        function Get-RegistryAclDiagnostic { param([string]$Path) 'acl=stubbed' }
        function Test-Path { param([string]$LiteralPath, [string]$Path, [string]$PathType) $true }
        function New-Item { param([string]$Path, [switch]$Force) $script:createdKeys += $Path }
        function New-ItemProperty {
            param([string]$LiteralPath, [string]$Name, [string]$PropertyType, [object]$Value, [switch]$Force)
            $script:policyWrites += [pscustomobject]@{ path = $LiteralPath; name = $Name; value = [string]$Value }
        }
        function Read-ChromeExtensionSettingsPolicy {
            param([string]$Hive)
            # Deep copy through JSON so a test's seed is never mutated in place.
            $copy = [ordered]@{}
            if ($script:policySeed.Count -eq 0) { return $copy }
            $parsed = ($script:policySeed | ConvertTo-Json -Depth 8) | ConvertFrom-Json
            foreach ($property in $parsed.PSObject.Properties) {
                $entry = [ordered]@{}
                foreach ($entryProperty in $property.Value.PSObject.Properties) {
                    if ($entryProperty.Value -is [System.Array]) {
                        $entry[$entryProperty.Name] = @($entryProperty.Value)
                    } else {
                        $entry[$entryProperty.Name] = $entryProperty.Value
                    }
                }
                $copy[$property.Name] = $entry
            }
            return $copy
        }
        function New-HazardRow {
            param([string]$Id, [string[]]$HazardApi)
            [pscustomobject]@{
                extension_id = $Id
                name = "fixture $Id"
                active_api = @($HazardApi)
                granted_api = @()
                manifest_api = @()
                hazard_api = @($HazardApi)
                runtime_enabled = $true
                source = 'chrome_profile_active'
            }
        }
        function Invoke-Shield {
            param([object[]]$Extensions, [string[]]$ProtectedExtensionIds = @())
            $script:policyWrites = @()
            $script:createdKeys = @()
            Set-SynapseChromeExternalDebuggerPolicy `
                -Extensions $Extensions `
                -BridgeExtensionId $script:bridgeId `
                -ProtectedExtensionIds $ProtectedExtensionIds
        }
        function Get-WrittenSettings {
            @($script:policyWrites).Count | Should Be 1
            $script:policyWrites[0].name | Should Be 'ExtensionSettings'
            $script:policyWrites[0].value | ConvertFrom-Json
        }
    }

    It 'must_pass_protected_id_helper_reads_the_row' {
        (Test-SynapseBrowserProtectedExtensionId -Row $script:braveRow -ExtensionId $script:braveProtectedId) | Should Be $true
        (Test-SynapseBrowserProtectedExtensionId -Row $script:braveRow -ExtensionId $script:thirdPartyId) | Should Be $false
        # Exact, case-sensitive id match only.
        (Test-SynapseBrowserProtectedExtensionId -Row $script:braveRow -ExtensionId $script:braveProtectedId.ToUpperInvariant()) | Should Be $false
        (Test-SynapseBrowserProtectedExtensionId -Row $script:chromeRow -ExtensionId $script:braveProtectedId) | Should Be $false
        (Test-SynapseBrowserProtectedExtensionId -Row $script:braveRow -ExtensionId '') | Should Be $false
        $rowWithoutField = [pscustomobject]@{ id = 'fixture' }
        (Test-SynapseBrowserProtectedExtensionId -Row $rowWithoutField -ExtensionId $script:braveProtectedId) | Should Be $false
    }

    It 'must_fail_protected_debugger_extension_is_never_policy_blocked' {
        $script:policySeed = [ordered]@{}
        $result = Invoke-Shield `
            -Extensions @(New-HazardRow -Id $script:braveProtectedId -HazardApi @('debugger')) `
            -ProtectedExtensionIds @($script:braveRow.protected_extension_ids)
        $result.changed | Should Be $false
        $result.reason | Should Be 'no_debugger_or_native_hazards'
        @($result.shielded_entries).Count | Should Be 0
        @($script:policyWrites).Count | Should Be 0
        @($script:createdKeys).Count | Should Be 0
    }

    It 'must_pass_unprotected_hazard_is_still_blocked' {
        $script:policySeed = [ordered]@{}
        $result = Invoke-Shield `
            -Extensions @(
                (New-HazardRow -Id $script:braveProtectedId -HazardApi @('debugger')),
                (New-HazardRow -Id $script:thirdPartyId -HazardApi @('debugger'))
            ) `
            -ProtectedExtensionIds @($script:braveRow.protected_extension_ids)
        $result.changed | Should Be $true
        @($result.shielded_entries | ForEach-Object { $_.extension_id }) -join ',' | Should Be $script:thirdPartyId
        $written = Get-WrittenSettings
        @($written.$($script:thirdPartyId).blocked_permissions) -contains 'debugger' | Should Be $true
        @($written.PSObject.Properties.Name) -contains $script:braveProtectedId | Should Be $false
    }

    It 'must_fail_hazard_scan_skips_protected_ids_before_anything_else' {
        # The Preferences scan is top-level installer code (not a function), so
        # this guard pins its first statement: a protected id is skipped before
        # it can be counted as a hazard, shielded or reported.
        $loops = @($script:installerAst.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.ForEachStatementAst] -and
                    $node.Condition.Extent.Text -eq '$pref.extensions.settings.PSObject.Properties'
                }, $true))
        $loops.Count | Should Be 1
        $first = @($loops[0].Body.Statements)[0].Extent.Text
        $first | Should Match 'Test-SynapseBrowserProtectedExtensionId'
        $first | Should Match '\$script:BrowserRow'
        $first | Should Match '\$extensionProperty\.Name'
        $first | Should Match '\bcontinue\b'
    }

    It 'must_fail_shield_never_blocks_debugger_or_runtime_hosts_for_own_bridge_id' {
        # Self-shield row with no declared hazard api: the generic default
        # (debugger + nativeMessaging) must not reach the bridge's own entry.
        $script:policySeed = [ordered]@{}
        $result = Invoke-Shield -Extensions @(New-HazardRow -Id $script:bridgeId -HazardApi @())
        $result.changed | Should Be $true
        $written = Get-WrittenSettings
        $own = $written.$($script:bridgeId)
        @($own.blocked_permissions) -contains 'debugger' | Should Be $false
        @($own.blocked_permissions) -contains 'nativeMessaging' | Should Be $true
        @($own.PSObject.Properties.Name) -contains 'runtime_blocked_hosts' | Should Be $false
    }

    It 'must_fail_shield_never_writes_the_wildcard_entry' {
        $script:policySeed = [ordered]@{ '*' = [ordered]@{ installation_mode = 'allowed' } }
        $result = Invoke-Shield -Extensions @(
            (New-HazardRow -Id '*' -HazardApi @('debugger')),
            (New-HazardRow -Id $script:bridgeId -HazardApi @('nativeMessaging')),
            (New-HazardRow -Id $script:thirdPartyId -HazardApi @('debugger'))
        )
        $result.changed | Should Be $true
        @($result.shielded_entries | ForEach-Object { $_.extension_id }) -contains '*' | Should Be $false
        $written = Get-WrittenSettings
        $wildcard = $written.'*'
        @($wildcard.PSObject.Properties.Name) -join ',' | Should Be 'installation_mode'
        $wildcard.installation_mode | Should Be 'allowed'
        @($written.$($script:bridgeId).PSObject.Properties.Name) -contains 'runtime_blocked_hosts' | Should Be $false
        @($written.$($script:bridgeId).blocked_permissions) -contains 'debugger' | Should Be $false
        @($written.$($script:thirdPartyId).blocked_permissions) -contains 'debugger' | Should Be $true
    }

    It 'must_pass_admin_preset_guarded_entries_are_preserved_unchanged' {
        # Policy an admin already set on the guarded entries is neither removed
        # nor extended by the shield (it only merges its own two keys).
        $script:policySeed = [ordered]@{
            '*' = [ordered]@{ runtime_blocked_hosts = @('*://wildcard.example/*') }
        }
        $script:policySeed[$script:bridgeId] = [ordered]@{ runtime_blocked_hosts = @('*://admin.example/*') }
        $result = Invoke-Shield -Extensions @(New-HazardRow -Id $script:bridgeId -HazardApi @('nativeMessaging'))
        $result.changed | Should Be $true
        $written = Get-WrittenSettings
        @($written.'*'.runtime_blocked_hosts) -join ',' | Should Be '*://wildcard.example/*'
        @($written.$($script:bridgeId).runtime_blocked_hosts) -join ',' | Should Be '*://admin.example/*'
        @($written.$($script:bridgeId).blocked_permissions) -join ',' | Should Be 'nativeMessaging'
    }

    It 'must_fail_guard_state_detects_any_guarded_entry_change' {
        $base = [ordered]@{ '*' = [ordered]@{ installation_mode = 'allowed' } }
        $base[$script:bridgeId] = [ordered]@{ blocked_permissions = @('nativeMessaging') }
        $before = Get-SynapsePolicyShieldGuardedState -Settings $base -BridgeExtensionId $script:bridgeId

        $base[$script:thirdPartyId] = [ordered]@{ runtime_blocked_hosts = @('*://x/*'); blocked_permissions = @('debugger') }
        (Get-SynapsePolicyShieldGuardedState -Settings $base -BridgeExtensionId $script:bridgeId) | Should BeExactly $before

        $base[$script:bridgeId]['runtime_blocked_hosts'] = @('*://*/*')
        (Get-SynapsePolicyShieldGuardedState -Settings $base -BridgeExtensionId $script:bridgeId) | Should Not BeExactly $before
        $base[$script:bridgeId].Remove('runtime_blocked_hosts')

        $base[$script:bridgeId]['blocked_permissions'] = @('debugger', 'nativeMessaging')
        (Get-SynapsePolicyShieldGuardedState -Settings $base -BridgeExtensionId $script:bridgeId) | Should Not BeExactly $before
        $base[$script:bridgeId]['blocked_permissions'] = @('nativeMessaging')

        $base['*']['runtime_blocked_hosts'] = @('*://*/*')
        (Get-SynapsePolicyShieldGuardedState -Settings $base -BridgeExtensionId $script:bridgeId) | Should Not BeExactly $before
        $base['*'].Remove('runtime_blocked_hosts')

        $base['*']['blocked_permissions'] = @('debugger')
        (Get-SynapsePolicyShieldGuardedState -Settings $base -BridgeExtensionId $script:bridgeId) | Should Not BeExactly $before

        # The shield compares the guarded state read before its merge with the
        # state it is about to write, and refuses the write on any difference.
        $shieldAst = $script:installerAst.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Set-SynapseChromeExternalDebuggerPolicy'
            }, $true)
        $shieldText = $shieldAst.Extent.Text
        ([regex]::Matches($shieldText, 'Get-SynapsePolicyShieldGuardedState')).Count | Should Be 2
        $shieldText | Should Match 'SYNAPSE_CHROME_POLICY_SHIELD_GUARDED_ENTRY_CHANGED'
    }
}
