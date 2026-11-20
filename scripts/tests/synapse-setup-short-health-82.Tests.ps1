$ErrorActionPreference = 'Stop'

Describe 'Setup consumes public short bridge health (#82)' {
    BeforeAll {
        $path = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null; $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw ($errors | Out-String) }
        $fn = $ast.Find({ param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $n.Name -eq 'Assert-SynapseChromeBridgeLiveAfterSetup'
        }, $true)
        if ($null -eq $fn) { throw 'Missing real setup readiness entry point' }
        Invoke-Expression $fn.Extent.Text
        function Info { param($Message) }
        function Die { param($Message) throw $Message }
        function Invoke-SynapseSetupMcpTool { param($Bind, $Token, $Name, $Arguments, $Profile, $ProfileReason, $TimeoutSec) }
        $SynapseChromeBridgeDefaultPostStartWaitMs = 0
        $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = $null
        $script:healthy = [pscustomobject]@{ subsystems = @{ chrome_bridge = @{
            status = 'ok'
            detail = 'tab_control_available=true host_count=1 queued_count=0 pending_count=0 extension_stale=false extension_capabilities=alarmReconnect,pageScreenshot'
        } } }
        $script:stale = [pscustomobject]@{ subsystems = @{ chrome_bridge = @{
            status = 'stale'
            detail = 'tab_control_available=false host_count=1 queued_count=0 pending_count=0 extension_stale=true extension_capabilities=pageScreenshot extension_stale_reasons=missing_capabilities=alarmReconnect'
        } } }
    }

    It 'must_pass healthy short summary without private diagnostics or reload' {
        Mock Invoke-SynapseSetupMcpTool { throw 'Healthy summary must not request reload' }
        Mock Invoke-RestMethod { throw 'Healthy summary must return immediately' }
        $actual = Assert-SynapseChromeBridgeLiveAfterSetup -Bind '127.0.0.1:1' -Token 'synthetic' -Health $script:healthy -ChromeBridgeInstallerPath 'unused' -ChromeNativeHostExePath 'unused'
        [object]::ReferenceEquals($actual, $script:healthy) | Should Be $true
        Assert-MockCalled Invoke-SynapseSetupMcpTool -Times 0 -Exactly
        Assert-MockCalled Invoke-RestMethod -Times 0 -Exactly
    }

    It 'must_fail stale short summary after reload stays stale' {
        Mock Invoke-SynapseSetupMcpTool { [pscustomobject]@{ Json = @{ reload_bridge = @{ after = @{ extension_build_id = 'synthetic' } } } } }
        Mock Invoke-RestMethod { $script:stale }
        { Assert-SynapseChromeBridgeLiveAfterSetup -Bind '127.0.0.1:1' -Token 'synthetic' -Health $script:stale -ChromeBridgeInstallerPath 'unused' -ChromeNativeHostExePath 'unused' } | Should Throw 'SYNAPSE_CHROME_BRIDGE_STALE_AFTER_SETUP_RELOAD'
        Assert-MockCalled Invoke-SynapseSetupMcpTool -Times 1 -Exactly
        Assert-MockCalled Invoke-RestMethod -Times 1 -Exactly
    }

    It 'must_fail ok status cannot hide stale fact or missing screenshot capability' {
        Mock Invoke-SynapseSetupMcpTool { [pscustomobject]@{ Json = @{} } }
        Mock Invoke-RestMethod { $script:invalid }
        foreach ($detail in @(
            'tab_control_available=true extension_stale=true extension_capabilities=alarmReconnect,pageScreenshot',
            'tab_control_available=true extension_stale=false extension_capabilities=alarmReconnect'
        )) {
            $script:invalid = [pscustomobject]@{ subsystems = @{ chrome_bridge = @{
                status = 'ok'; detail = $detail
            } } }
            { Assert-SynapseChromeBridgeLiveAfterSetup -Bind '127.0.0.1:1' -Token 'synthetic' -Health $script:invalid -ChromeBridgeInstallerPath 'unused' -ChromeNativeHostExePath 'unused' } | Should Throw 'SYNAPSE_CHROME_BRIDGE_STALE_AFTER_SETUP_RELOAD'
        }
    }
}
