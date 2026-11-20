$ErrorActionPreference = 'Stop'

# #140: the maintenance-pause health probe must tolerate slow /health and skip
# (not fail) when the Chrome bridge is not connected.
Describe 'Request-SynapseChromeBridgeMaintenancePause health handling (#140)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }

        $functionAst = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Request-SynapseChromeBridgeMaintenancePause'
            }, $true)
        if ($null -eq $functionAst) { throw 'Request-SynapseChromeBridgeMaintenancePause missing' }
        Invoke-Expression $functionAst.Extent.Text

        $timeoutAst = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                $node.Left.Extent.Text -eq '$SynapseChromeBridgeMaintenancePauseHealthTimeoutSec'
            }, $true)
        if ($null -eq $timeoutAst) { throw 'health timeout config variable missing' }
        Invoke-Expression $timeoutAst.Extent.Text
        # #140 rework: degraded-health retry budget (optional so the red phase
        # of the test commit fails on behavior, not on a missing variable).
        foreach ($name in @('$SynapseChromeBridgeMaintenancePauseDegradedRetries', '$SynapseChromeBridgeMaintenancePauseDegradedRetryDelaySec')) {
            $assignAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                    $node.Left.Extent.Text -eq $name
                }, $true)
            if ($null -ne $assignAst) { Invoke-Expression $assignAst.Extent.Text }
        }
        $script:SynapseChromeBridgeMaintenancePauseMs = 1000
    }

    It 'uses a health timeout well above the observed >4 s deploy-time latency' {
        ($SynapseChromeBridgeMaintenancePauseHealthTimeoutSec -ge 15) | Should Be $true
        Mock Invoke-RestMethod {
            [pscustomobject]@{ subsystems = [pscustomobject]@{ chrome_bridge = [pscustomobject]@{ status = 'unavailable'; detail = 'x' } } }
        }
        $null = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        # PowerShell 7.4+ makes -TimeoutSec an alias of -ConnectionTimeoutSeconds;
        # Pester 3 binds the canonical name, so accept the value under either.
        Assert-MockCalled Invoke-RestMethod -Times 1 -Exactly -Scope It -ParameterFilter {
            $expected = $SynapseChromeBridgeMaintenancePauseHealthTimeoutSec
            ($null -ne $expected) -and (($TimeoutSec -eq $expected) -or ($ConnectionTimeoutSeconds -eq $expected))
        }
    }

    It 'skips the pause when chrome_bridge status is <Status>' -TestCases @(
        @{ Status = 'unavailable' }, @{ Status = 'disconnected' }, @{ Status = 'disabled' }
    ) {
        param($Status)
        Mock Invoke-RestMethod {
            if ($Method -eq 'Post') { throw 'pause POST must not be sent' }
            [pscustomobject]@{ subsystems = [pscustomobject]@{ chrome_bridge = [pscustomobject]@{ status = $Status; detail = 'tab_control_available=false' } } }
        }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Ok | Should Be $true
        $result.Skipped | Should Be $true
        Assert-MockCalled Invoke-RestMethod -Times 1 -Exactly -Scope It
    }

    It 'still fails closed when /health itself fails' {
        Mock Invoke-RestMethod { throw 'The operation has timed out.' }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Ok | Should Be $false
        $result.Code | Should Be 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_HEALTH_FAILED'
    }

    It 'empty chrome_bridge status with an unknown detail stays unreadable' {
        Mock Invoke-RestMethod {
            [pscustomobject]@{ subsystems = [pscustomobject]@{ chrome_bridge = [pscustomobject]@{ status = ''; detail = 'x' } } }
        }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Code | Should Be 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_STATUS_UNREADABLE'
    }

    # Old daemons answer the 3 s degraded fallback WITHOUT chrome_bridge.
    $degradedNoBridge = {
        [pscustomobject]@{ ok = $false; subsystems = [pscustomobject]@{ health = [pscustomobject]@{ status = 'degraded'; detail = 'health payload exceeded 3000 ms' } } }
    }
    # New daemons carry chrome_bridge status unknown, reason health_timeout.
    $degradedWithBridge = {
        [pscustomobject]@{ ok = $false; subsystems = [pscustomobject]@{
                health = [pscustomobject]@{ status = 'degraded'; detail = 'health payload exceeded 3000 ms' }
                chrome_bridge = [pscustomobject]@{ status = 'unknown'; detail = 'health_timeout' }
            } }
    }

    It 'degraded health without chrome_bridge is non-fatal: retried, then skipped with a warning' {
        Mock Start-Sleep { }
        Mock Write-Warning { }
        Mock Invoke-RestMethod {
            if ($Method -eq 'Post') { throw 'pause POST must not be sent' }
            & $degradedNoBridge
        }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Ok | Should Be $true
        $result.Skipped | Should Be $true
        $result.Code | Should Be 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SKIPPED_HEALTH_DEGRADED'
        Assert-MockCalled Invoke-RestMethod -Scope It -ParameterFilter { $Method -eq 'Get' } -Times 2
        Assert-MockCalled Write-Warning -Scope It -Times 1 -Exactly
    }

    It 'chrome_bridge status unknown (health_timeout) is non-fatal' {
        Mock Start-Sleep { }
        Mock Write-Warning { }
        Mock Invoke-RestMethod {
            if ($Method -eq 'Post') { throw 'pause POST must not be sent' }
            & $degradedWithBridge
        }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Ok | Should Be $true
        $result.Skipped | Should Be $true
        $result.Code | Should Be 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SKIPPED_HEALTH_DEGRADED'
    }

    It 'retries a degraded health and uses the first full payload' {
        Mock Start-Sleep { }
        $global:PauseHealthCalls = 0
        Mock Invoke-RestMethod {
            $global:PauseHealthCalls++
            if ($global:PauseHealthCalls -lt 3) { return (& $degradedNoBridge) }
            [pscustomobject]@{ ok = $true; subsystems = [pscustomobject]@{ chrome_bridge = [pscustomobject]@{ status = 'unavailable'; detail = 'no_active_chrome_bridge_host' } } }
        }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Ok | Should Be $true
        $result.Code | Should Be 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SKIPPED_NO_ACTIVE_HOST'
        $global:PauseHealthCalls | Should Be 3
    }

    It 'a healthy payload that lacks chrome_bridge stays fatal' {
        Mock Start-Sleep { }
        Mock Invoke-RestMethod {
            [pscustomobject]@{ ok = $true; subsystems = [pscustomobject]@{ health = [pscustomobject]@{ status = 'ok' } } }
        }
        $result = Request-SynapseChromeBridgeMaintenancePause -Bind '127.0.0.1:1' -Token 't' -Reason 'test'
        $result.Ok | Should Be $false
        $result.Code | Should Be 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_HEALTH_MISSING'
        Assert-MockCalled Invoke-RestMethod -Scope It -Times 1 -Exactly
    }
}
