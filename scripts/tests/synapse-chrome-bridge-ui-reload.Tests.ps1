$ErrorActionPreference = 'Stop'

Describe 'Synapse Chrome bridge existing-extension UI reload deferral' {
    BeforeAll {
        $installerPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'install-synapse-chrome-debugger.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $installerPath,
            [ref]$tokens,
            [ref]$errors
        )
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $functionAst = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Invoke-SynapseChromeBridgeExistingUiReload'
            }, $true)
        if ($null -eq $functionAst) { throw 'installer function is missing: Invoke-SynapseChromeBridgeExistingUiReload' }
        Invoke-Expression $functionAst.Extent.Text
        $autoInstallAst = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Invoke-SynapseChromeBridgeAutoInstall'
            }, $true)
        if ($null -eq $autoInstallAst) { throw 'installer function is missing: Invoke-SynapseChromeBridgeAutoInstall' }
        Invoke-Expression $autoInstallAst.Extent.Text
        # Multi-browser bridge: the installer reads the extensions page URL
        # from the chrome browser row (crates\synapse-browsers\browsers).
        . (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-browser-rows.ps1')
        $script:BrowserExtensionsPageUrl = [string](Get-SynapseBrowserRow -Browser 'chrome').extension.extensions_page_url

        function Initialize-SynapseChromeBridgeAutoInstallInterop {
            if (-not ('SynapseChromeBridgeAutoInstall.Win32' -as [type])) {
                Add-Type -TypeDefinition @'
namespace SynapseChromeBridgeAutoInstall
{
    public static class Win32
    {
        public static bool ShowWindowAsync(System.IntPtr hWnd, int nCmdShow) { return false; }
        public static bool SetForegroundWindow(System.IntPtr hWnd) { return false; }
    }
}
'@
            }
        }
        function Get-SynapseChromeTopLevelWindows {
            param([string]$ChromeUserDataRoot)
            $script:chromeWindows
        }
        function Format-SynapseChromeWindowRejectSummary {
            param($Windows)
            (@($Windows) | ForEach-Object { [string]$_.reject_reason }) -join ';'
        }
        function Set-SynapseChromeAddressBarUrl {
            param($Window, [string]$Url)
            $script:addressBarUrl = $Url
            $script:addressBarWindowHwnd = $Window.hwnd
        }
        # Mirrors the real Wait-SynapseUntil contract (Probe, Deadline,
        # SleepMilliseconds) and actually invokes the probe, so the reload
        # path is exercised through the mocked Find/Read functions exactly
        # as in production: a test exposes or hides the Reload button by
        # setting $script:reloadButton.
        function Wait-SynapseUntil {
            param(
                [Parameter(Mandatory = $true)][scriptblock]$Probe,
                [Parameter(Mandatory = $true)][datetime]$Deadline,
                [int]$SleepMilliseconds = 1
            )
            do {
                $value = & $Probe
                if ($value) { return $value }
                Start-Sleep -Milliseconds $SleepMilliseconds
            } while ((Get-Date) -lt $Deadline)
            return $null
        }
        function Get-SynapseChromeWindowByHwnd { param([long]$Hwnd) $script:windowByHwnd }
        function Find-SynapseAutomationElementByAutomationId {
            param($Root, [string]$AutomationId, $ControlType)
            $script:reloadButton
        }
        function Read-SynapseChromeExtensionDetailsUiState { param($Window) $script:uiState }
        function Invoke-SynapseAutomationElement {
            param($Element, [string]$Description)
            $script:clickedReloadDescriptions += @($Description)
        }
        function Test-SynapseChromeBridgeProfileRow { $script:profileRow }
        function Get-SynapseActiveChromeProfileName { param([string]$ChromeUserDataRoot, [string]$ExtensionId, [string]$ExtensionDir) 'Default' }
        # The real reload probe references ControlType by fully-qualified
        # name; on desktop PowerShell ControlType resolves only after loading
        # UIAutomationTypes (UIAutomationClient alone is not enough).
        Add-Type -AssemblyName UIAutomationTypes
    }

    $readyRow = [pscustomobject]@{
        installed = $true
        manifest_path_matches = $true
        ready = $true
        enabled_and_permissioned = $true
        manifest_dir_exists = $true
        manifest_path = "$TestDrive\old\manifest.json"
        missing_active_api_permissions = @()
        disable_reasons = @()
    }
    $notReadyRow = [pscustomobject]@{
        installed = $true
        manifest_path_matches = $true
        ready = $false
        enabled_and_permissioned = $false
        manifest_dir_exists = $false
        manifest_path = "$TestDrive\broken\manifest.json"
        missing_active_api_permissions = @('cookies')
        disable_reasons = @('disabled_by_user')
    }
    $automationWindow = [pscustomobject]@{
        hwnd = 4242
        pid = 9001
        title = 'Playwright automation Chrome'
        chrome_profile_eligible = $false
        reject_reason = 'dedicated_automation_user_data_dir'
        is_foreground = $true
        chrome_user_data_dir = 'C:\ms-playwright\chrome-profile'
        chrome_profile_match_reason = 'automation_profile'
    }
    $eligibleWindow = [pscustomobject]@{
        hwnd = 8484
        pid = 9002
        title = 'Google Chrome'
        chrome_profile_eligible = $true
        reject_reason = $null
        is_foreground = $true
        chrome_user_data_dir = "$env:LOCALAPPDATA\Google\Chrome\User Data"
        chrome_profile_match_reason = 'default_profile_directory'
    }

    It 'defers nonfatally when only rejected automation Chrome windows exist' {
        $script:chromeWindows = @($automationWindow)
        $script:addressBarUrl = $null
        $script:reloadButton = $null

        $result = Invoke-SynapseChromeBridgeExistingUiReload `
            -ChromeUserDataRoot "$env:LOCALAPPDATA\Google\Chrome\User Data" `
            -ExtensionId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
            -ExtensionDir "$TestDrive\extension" `
            -ActiveProfile 'Default' `
            -Before $readyRow `
            -TimeoutSeconds 5

        $result.attempted | Should Be $false
        $result.changed | Should Be $false
        $result.reason | Should Be 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
        $result.required_foreground | Should Be $false
        $result.bridge_self_reload_command | Should Be 'browser_debugger.reload_bridge'
        $result.rejected_windows | Should Match 'dedicated_automation_user_data_dir'
        $result.active_profile | Should Be 'Default'
        $result.before.ready | Should Be $true
        $result.after.ready | Should Be $true
        $script:addressBarUrl | Should Be $null
    }

    It 'still attempts the foreground UI reload when an eligible Chrome window exists' {
        $script:chromeWindows = @($eligibleWindow)
        $script:addressBarUrl = $null
        $script:addressBarWindowHwnd = $null
        $script:reloadButton = $null

        $message = $null
        try {
            Invoke-SynapseChromeBridgeExistingUiReload `
                -ChromeUserDataRoot "$env:LOCALAPPDATA\Google\Chrome\User Data" `
                -ExtensionId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
                -ExtensionDir "$TestDrive\extension" `
                -ActiveProfile 'Default' `
                -Before $readyRow `
                -TimeoutSeconds 5 | Out-Null
        } catch {
            $message = $_.Exception.Message
        }

        $script:addressBarUrl | Should Be 'chrome://extensions/?id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
        $script:addressBarWindowHwnd | Should Be 8484
        $message | Should Match '^SYNAPSE_CHROME_BRIDGE_UI_RELOAD_BUTTON_NOT_FOUND '
    }

    It 'completes the foreground UI reload when the Reload button is exposed' {
        $script:chromeWindows = @($eligibleWindow)
        $script:addressBarUrl = $null
        $script:addressBarWindowHwnd = $null
        $script:uiState = [pscustomobject]@{
            enable_toggle_present = $true
            enable_toggle_on = $true
            enable_toggle_name = 'Enable'
        }
        $script:windowByHwnd = $eligibleWindow
        $script:profileRow = $readyRow
        $script:clickedReloadDescriptions = @()
        $script:reloadButton = [pscustomobject]@{ name = 'Reload' }

        $result = Invoke-SynapseChromeBridgeExistingUiReload `
            -ChromeUserDataRoot "$env:LOCALAPPDATA\Google\Chrome\User Data" `
            -ExtensionId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
            -ExtensionDir "$TestDrive\extension" `
            -ActiveProfile 'Default' `
            -Before $readyRow `
            -TimeoutSeconds 5

        $result.attempted | Should Be $true
        $result.changed | Should Be $true
        $result.reason | Should Be 'existing_ready_extension_ui_reload_invoked'
        $result.required_foreground | Should Be $true
        $result.chrome_window_hwnd | Should Be 8484
        $script:addressBarUrl | Should Be 'chrome://extensions/?id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
        @($script:clickedReloadDescriptions) -contains 'Synapse Chrome Bridge Reload' | Should Be $true
        $result.after.ready | Should Be $true
    }

    It 'still fails fatally on first-time install with no eligible Chrome window' {
        $script:chromeWindows = @($automationWindow)
        $script:profileRow = [pscustomobject]@{
            installed = $false
            manifest_path_matches = $false
            ready = $false
            enabled_and_permissioned = $false
            manifest_dir_exists = $false
            manifest_path = "$TestDrive\absent\manifest.json"
            missing_active_api_permissions = @()
            disable_reasons = @()
        }
        $script:addressBarUrl = $null

        $message = $null
        try {
            Invoke-SynapseChromeBridgeAutoInstall `
                -ChromeUserDataRoot "$env:LOCALAPPDATA\Google\Chrome\User Data" `
                -ExtensionId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
                -ExtensionDir "$TestDrive\extension" `
                -TimeoutSeconds 5 | Out-Null
        } catch {
            $message = $_.Exception.Message
        }

        $message | Should Match '^SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_NO_ELIGIBLE_CHROME_WINDOW '
        $message | Should Match 'rejected_windows=.*dedicated_automation_user_data_dir'
        $message | Should Match 'remediation=open the already-authenticated Chrome profile for this user-data root; setup refuses to drive ms-playwright-mcp, remote-debugging, unreadable, or other dedicated Chrome user-data-dir windows'
        $script:addressBarUrl | Should Be $null
    }

    It 'still refuses a profile row that is not installed and enabled' {
        $script:chromeWindows = @($eligibleWindow)

        $message = $null
        try {
            Invoke-SynapseChromeBridgeExistingUiReload `
                -ChromeUserDataRoot "$env:LOCALAPPDATA\Google\Chrome\User Data" `
                -ExtensionId 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' `
                -ExtensionDir "$TestDrive\extension" `
                -ActiveProfile 'Default' `
                -Before $notReadyRow `
                -TimeoutSeconds 5 | Out-Null
        } catch {
            $message = $_.Exception.Message
        }

        $message | Should Match '^SYNAPSE_CHROME_BRIDGE_UI_RELOAD_ROW_NOT_READY '
        $message | Should Match 'missing_active_api_permissions=cookies'
    }
}
