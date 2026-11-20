$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites
# except maintenance-pause-health. Run with:
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# Check the Pester that actually runs this file: the module that owns the `Should`
# command in scope, not the highest version loaded. pwsh may autoload 5.x first and
# `Import-Module Pester -RequiredVersion 3.4.0` then loads 3.4 side by side; the
# newest import wins command resolution, so `Should` identifies the active one.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-chrome-bridge-ui-repair-gate.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse setup Chrome bridge UI repair deferred-result gates' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $setupPath,
            [ref]$tokens,
            [ref]$errors
        )
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }

        function Info($m) { $script:infoLines += "[info] $m" }
        function Die($m) { throw $m }
        function Format-SynapseChromeBridgeProfileInstallState { param($Readback) 'profile_install_state=stubbed' }

        foreach ($name in @('Invoke-SynapseChromeBridgeUiRepair', 'Assert-SynapseChromeBridgeLiveAfterSetup')) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "setup function is missing: $name" }
            # Invoke-Expression directly in BeforeAll scope so the extracted
            # function persists for the It blocks (a helper function would
            # define it in its own vanishing local scope).
            Invoke-Expression $functionAst.Extent.Text
        }

        $script:infoLines = @()
        $script:SynapseChromeBridgeUiReloadDeferredReason = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
        $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = $null

        # Mock installer: a real script file that accepts the setup invocation
        # arguments and emits a canned readback selected by an env var.
        $script:mockInstaller = Join-Path $TestDrive 'mock-installer.ps1'
        @'
param(
    [string]$SynapseNativeHostExe,
    [switch]$ReloadExistingExtensionViaUi,
    [int]$AutoInstallTimeoutSeconds
)
$reason = $env:SYNAPSE_TEST_MOCK_REPAIR_REASON
$profileInstalled = if ($env:SYNAPSE_TEST_MOCK_PROFILE_INSTALLED -eq 'false') { $false } else { $true }
[pscustomobject]@{
    ok = $true
    synapse_chrome_auto_install = [pscustomobject]@{
        attempted = ($reason -ne 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon')
        changed = ($reason -eq 'installed_unpacked_extension_in_active_profile')
        reason = $reason
        active_profile = 'Default'
        bridge_self_reload_command = 'browser_debugger.reload_bridge'
        rejected_windows = 'dedicated_automation_user_data_dir'
        remediation = 'open the already-authenticated Chrome profile for this user-data root and rerun setup'
    }
    synapse_chrome_profile_install_state = [pscustomobject]@{
        active_profile = 'Default'
        active_profile_installed = $profileInstalled
    }
}
'@ | Set-Content -Path $script:mockInstaller -Encoding UTF8

        function New-DeferredHealth {
            [pscustomobject]@{
                ok = $true
                subsystems = [pscustomobject]@{
                    chrome_bridge = [pscustomobject]@{
                        status = 'unavailable'
                        detail = 'no_active_chrome_bridge_host host_count=0 queued_count=0 pending_count=0'
                    }
                }
            }
        }
        function New-CleanHealth {
            [pscustomobject]@{
                ok = $true
                subsystems = [pscustomobject]@{
                    chrome_bridge = [pscustomobject]@{
                        status = 'ok'
                        detail = 'tab_control_available=true host_count=1 queued_count=0 pending_count=0 extension_stale=false extension_capabilities=alarmReconnect,pageScreenshot'
                    }
                }
            }
        }
    }

    Context 'Invoke-SynapseChromeBridgeUiRepair reason gate' {
        It 'accepts the no-eligible-window deferred reason without dying' {
            $env:SYNAPSE_TEST_MOCK_REPAIR_REASON = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
            $env:SYNAPSE_TEST_MOCK_PROFILE_INSTALLED = 'true'

            $readback = Invoke-SynapseChromeBridgeUiRepair `
                -InstallerPath $script:mockInstaller `
                -NativeHostExePath "$TestDrive\native-host.exe"

            $readback.ok | Should Be $true
            $readback.synapse_chrome_auto_install.reason | Should Be 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
            ($script:infoLines | Where-Object { $_ -match 'deferred to daemon' }).Count | Should BeGreaterThan 0
        }

        It 'still dies on an unknown non-deferred reason' {
            $env:SYNAPSE_TEST_MOCK_REPAIR_REASON = 'nothing_attempted_at_all'

            $message = $null
            try {
                Invoke-SynapseChromeBridgeUiRepair `
                    -InstallerPath $script:mockInstaller `
                    -NativeHostExePath "$TestDrive\native-host.exe" | Out-Null
            } catch {
                $message = $_.Exception.Message
            }
            $message | Should Match '^SYNAPSE_CHROME_BRIDGE_UI_REPAIR_NOT_PERFORMED '
            $message | Should Match 'reason=nothing_attempted_at_all'
        }

        It 'still dies when the deferred reason masks a missing profile row' {
            $env:SYNAPSE_TEST_MOCK_REPAIR_REASON = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
            $env:SYNAPSE_TEST_MOCK_PROFILE_INSTALLED = 'false'

            $message = $null
            try {
                Invoke-SynapseChromeBridgeUiRepair `
                    -InstallerPath $script:mockInstaller `
                    -NativeHostExePath "$TestDrive\native-host.exe" | Out-Null
            } catch {
                $message = $_.Exception.Message
            }
            $message | Should Match '^SYNAPSE_CHROME_BRIDGE_UI_REPAIR_PROFILE_NOT_INSTALLED '
        }

        It 'still accepts a performed reload reason' {
            $env:SYNAPSE_TEST_MOCK_REPAIR_REASON = 'existing_ready_extension_ui_reload_invoked'
            $env:SYNAPSE_TEST_MOCK_PROFILE_INSTALLED = 'true'

            $readback = Invoke-SynapseChromeBridgeUiRepair `
                -InstallerPath $script:mockInstaller `
                -NativeHostExePath "$TestDrive\native-host.exe"

            $readback.synapse_chrome_auto_install.reason | Should Be 'existing_ready_extension_ui_reload_invoked'
        }
    }

    Context 'Assert-SynapseChromeBridgeLiveAfterSetup deferred completion' {
        BeforeEach {
            function Invoke-SynapseChromeBridgeUiRepair {
                param([string]$InstallerPath, [string]$NativeHostExePath)
                [pscustomobject]@{
                    ok = $true
                    synapse_chrome_auto_install = [pscustomobject]@{
                        attempted = $false
                        changed = $false
                        reason = $env:SYNAPSE_TEST_MOCK_REPAIR_REASON
                    }
                    synapse_chrome_profile_install_state = [pscustomobject]@{
                        active_profile = 'Default'
                        active_profile_installed = $true
                    }
                }
            }
        }

        It 'completes setup with the healthy daemon health when the repair defers' {
            $env:SYNAPSE_TEST_MOCK_REPAIR_REASON = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
            # Maintenance pause still active shortcuts the alarmReconnect wait
            # exactly like a real restart-adjacent run.
            $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() + 600000
            $health = New-DeferredHealth

            $returned = Assert-SynapseChromeBridgeLiveAfterSetup `
                -Bind '127.0.0.1:7700' `
                -Token 'test-token' `
                -Health $health `
                -ChromeBridgeInstallerPath $script:mockInstaller `
                -ChromeNativeHostExePath "$TestDrive\native-host.exe"

            $returned | Should Be $health
            ($script:infoLines | Where-Object { $_ -match 'setup completes with the deferred bridge repair' }).Count | Should BeGreaterThan 0
        }

        It 'still waits and accepts a performed repair that turns bridge health clean' {
            $env:SYNAPSE_TEST_MOCK_REPAIR_REASON = 'existing_ready_extension_ui_reload_invoked'
            $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() + 600000
            $health = New-DeferredHealth

            function Invoke-RestMethod {
                param([string]$Uri, $Headers, $TimeoutSec)
                New-CleanHealth
            }

            $returned = Assert-SynapseChromeBridgeLiveAfterSetup `
                -Bind '127.0.0.1:7700' `
                -Token 'test-token' `
                -Health $health `
                -ChromeBridgeInstallerPath $script:mockInstaller `
                -ChromeNativeHostExePath "$TestDrive\native-host.exe"

            $returned.subsystems.chrome_bridge.status | Should Be 'ok'
        }
    }
}
