$ErrorActionPreference = 'Stop'

# Pester 3.4 / Windows PowerShell 5.1 and pwsh 7. Never execute either installer.
Describe '#251 setup live bridge identity acceptance' {
    $scripts = Split-Path -Parent $PSScriptRoot
    . (Join-Path $scripts 'synapse-bridge-live-identity.ps1')

    $tokens = $null; $errors = $null
    $setupAst = [System.Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $scripts 'synapse-setup.ps1'), [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
    $setupFunction = $setupAst.Find({ param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Assert-SynapseBridgeLiveIdentityAfterSetup'
    }, $true)
    if ($null -eq $setupFunction) { throw 'Missing setup acceptance entry point' }
    Invoke-Expression $setupFunction.Extent.Text

    $installerAst = [System.Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $scripts 'install-synapse-chrome-debugger.ps1'), [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
    # Run only the production expectation construction, never installer top-level code.
    # This catches regressions in strict/advisory selection and comparison order too.
    $assignment = $installerAst.Find({ param($node)
        $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left.Extent.Text -eq '$expectedLiveIdentity'
    }, $true)
    if ($null -eq $assignment) { throw 'Missing installer expected identity construction' }
    $script:identityConstruction251 = $assignment.Extent.Text

    function Info($Message) { $script:messages251 += [string]$Message }
    function Die($Message) { throw $Message }
    function New-Expected251([string]$Browser = 'chrome') {
        $script:BrowserRow = [pscustomobject]@{id=$Browser; health_key=($Browser + '_bridge'); extension=[pscustomobject]@{origin_scheme='chrome-extension'}}
        $extensionDeploy = [pscustomobject]@{code_stamp=('a' * 64); service_worker_sha256=('b' * 64)}
        $ExtensionId = 'abcdefghijklmnopabcdefghijklmnop'
        $bridgeOrigin = "chrome-extension://$ExtensionId/"
        $bridgeBuildId = 'current-build'
        $extensionManifest = [pscustomobject]@{version='1.0.0'}
        $activeChromeProfile = 'Default'
        $extensionDir = Join-Path $TestDrive 'installed-current'
        Invoke-Expression $script:identityConstruction251
        return $expectedLiveIdentity
    }
    function New-Health251([string]$Browser = 'chrome', [string]$Stamp = ('a' * 64),
        [string]$Folder = (Join-Path $TestDrive 'installed-current')) {
        # Independent health fixture, deliberately not generated from Expected.fields.
        $id = 'abcdefghijklmnopabcdefghijklmnop'
        $detail = "extension_code_stamp=$Stamp extension_id=$id origin=chrome-extension://$id/ transport=direct_http extension_build_id=current-build tab_control_available=true"
        if ($Browser -eq 'chrome') {
            $worker = Join-Path $Folder 'service_worker.js'
            $detail += ' active_host_id=host-chrome expected_code_stamp=' + ('a' * 64) +
                ' extension_version=1.0.0 extension_protocol_version=1 extension_service_worker_sha256=' + ('b' * 64) +
                ' expected_extension_service_worker_sha256=' + ('b' * 64) +
                " extension_service_worker_sha256_status=ok extension_service_worker_sha256_source=chrome-extension://$id/service_worker.js" +
                ' active_profile=Default active_profile_installed=true extension_stale=false' +
                " active_profile_extension_path=`"$Folder`" expected_extension_service_worker_path=`"$worker`""
        } else {
            $detail += " bridge_host_id=host-$Browser browser_kind=$Browser extension_browser_kind=$Browser"
        }
        $rows = @{}; $rows[($Browser + '_bridge')] = [pscustomobject]@{status='ok';detail=$detail}
        [pscustomobject]@{subsystems=[pscustomobject]$rows}
    }
    function Invoke-Setup251($Expected, $Health, [object[]]$Optional = @()) {
        $script:health251 = $Health
        Assert-SynapseBridgeLiveIdentityAfterSetup -Bind '127.0.0.1:1' -Token 'synthetic-test-token' `
            -Health $Health -ChromeReadback ([pscustomobject]@{runtime_verification=[pscustomobject]@{expected_identity=$Expected}}) `
            -OptionalBridges $Optional
    }

    BeforeEach {
        $script:messages251 = @()
        $script:SynapseLegacyBrowserId = 'chrome'
        Mock Read-SynapseChromeBridgeDetailedHealth { return $script:health251 }
        Mock Test-SynapseBridgeDaemonListening { return $true }
        Mock Invoke-WebRequest { throw 'Test must never reach the network' }
    }

    It 'must_fail_repo_checkout_unstamped_load_on_code_stamp_before_other_mismatches' {
        $expected = New-Expected251
        $health = New-Health251 -Stamp 'unstamped' -Folder (Join-Path $TestDrive 'repo-checkout')
        # Also mismatch alphabetically earlier fields: code identity must win.
        $health.subsystems.chrome_bridge.detail = $health.subsystems.chrome_bridge.detail.Replace('active_profile=Default', 'active_profile=Old')
        $outcome = Test-SynapseBridgeLiveIdentity -Health $health -Expected $expected
        $outcome.ok | Should Be $false
        $outcome.reason | Should BeExactly 'active_bridge_identity_mismatch:extension_code_stamp'
        { Invoke-Setup251 $expected $health } | Should Throw 'reason=active_bridge_identity_mismatch:extension_code_stamp'
        ($script:messages251 -join "`n") | Should Not Match 'Bridge live identity verified'
    }

    It 'must_fail_older_build_folder_stale_stamp_on_code_stamp' {
        $expected = New-Expected251
        $health = New-Health251 -Stamp ('c' * 64) -Folder (Join-Path $TestDrive 'older-build')
        $health.subsystems.chrome_bridge.detail = $health.subsystems.chrome_bridge.detail.Replace('extension_build_id=current-build', 'extension_build_id=old-build')
        $outcome = Test-SynapseBridgeLiveIdentity -Health $health -Expected $expected
        $outcome.ok | Should Be $false
        $outcome.reason | Should BeExactly 'active_bridge_identity_mismatch:extension_code_stamp'
        { Invoke-Setup251 $expected $health } | Should Throw 'reason=active_bridge_identity_mismatch:extension_code_stamp'
    }

    It 'must_pass_same_installer_stamped_code_from_another_folder_with_setup_warning' {
        $expected = New-Expected251
        $folder = Join-Path $TestDrive 'copied deployment with spaces'
        $health = New-Health251 -Folder $folder
        $outcome = Test-SynapseBridgeLiveIdentity -Health $health -Expected $expected
        $outcome.ok | Should Be $true
        $outcome.reason | Should BeExactly 'live_runtime_verified'
        @($outcome.warnings).Count | Should Be 2
        $outcome.warnings[0] | Should Match ([regex]::Escape("active_profile_extension_path=$folder"))
        Invoke-Setup251 $expected $health
        @($script:messages251 | Where-Object { $_ -match '^Bridge live identity verified browser=chrome ' }).Count | Should Be 1
        @($script:messages251 | Where-Object { $_ -match '^WARN: SYNAPSE_BRIDGE_LOADED_FROM_NONSTANDARD_FOLDER browser=chrome ' }).Count | Should Be 2
        ($script:messages251 -join "`n") | Should Match ([regex]::Escape($folder))
    }

    It 'must_pass_no_daemon_listening_as_deferred_without_health_request_or_failure' {
        Mock Test-SynapseBridgeDaemonListening { return $false }
        $outcome = Read-SynapseBridgeLiveOutcome -Expected (New-Expected251) -Token '' -Bind '127.0.0.1:1'
        $outcome.ok | Should Be $false
        $outcome.deferred | Should Be $true
        $outcome.reason | Should BeExactly 'live_runtime_verification_deferred_daemon_not_running'
        Assert-MockCalled Read-SynapseChromeBridgeDetailedHealth -Times 0 -Exactly -Scope It
        Assert-MockCalled Invoke-WebRequest -Times 0 -Exactly -Scope It
    }

    It 'must_pass_non_chrome_identity_without_chrome_only_keys_and_warn_only_on_row_failure' {
        $chrome = New-Expected251
        $edge = New-Expected251 'edge'
        $health = New-Health251
        $edgeHealth = New-Health251 'edge'
        $health.subsystems | Add-Member -NotePropertyName edge_bridge -NotePropertyValue $edgeHealth.subsystems.edge_bridge
        $edge.contract | Should BeExactly 'browser_row_identity'
        $outcome = Test-SynapseBridgeLiveIdentity -Health $health -Expected $edge
        $outcome.ok | Should Be $true
        @($outcome.warnings).Count | Should Be 0
        $optional = @([pscustomobject]@{browser='edge';readback=[pscustomobject]@{runtime_verification=[pscustomobject]@{expected_identity=$edge}}})
        Invoke-Setup251 $chrome $health $optional
        @($script:messages251 | Where-Object { $_ -match '^Bridge live identity verified browser=edge ' }).Count | Should Be 1
        # Even a genuinely missing required row key must remain optional WARN-only.
        $health.subsystems.edge_bridge.detail = $health.subsystems.edge_bridge.detail.Replace('extension_code_stamp=' + ('a' * 64), '')
        $script:messages251 = @()
        Invoke-Setup251 $chrome $health $optional
        @($script:messages251 | Where-Object { $_ -match '^WARN: SYNAPSE_BROWSER_BRIDGE_LIVE_IDENTITY_UNVERIFIED browser=edge reason=detailed_bridge_identity_missing:extension_code_stamp ' }).Count | Should Be 1
        @($script:messages251 | Where-Object { $_ -match '^Bridge live identity verified browser=chrome ' }).Count | Should Be 1
    }

    It 'must_pass_standard_folder_case_difference_without_nonstandard_warning' {
        $expected = New-Expected251
        $health = New-Health251 -Folder ((Join-Path $TestDrive 'installed-current').ToUpperInvariant())
        Invoke-Setup251 $expected $health
        @($script:messages251 | Where-Object { $_ -match '^Bridge live identity verified ' }).Count | Should Be 1
        @($script:messages251 | Where-Object { $_ -match '^WARN:' }).Count | Should Be 0
    }

    It 'must_fail_missing_running_code_stamp_even_if_every_other_identity_field_matches' {
        $expected = New-Expected251
        $health = New-Health251
        $health.subsystems.chrome_bridge.detail = $health.subsystems.chrome_bridge.detail.Replace('extension_code_stamp=' + ('a' * 64) + ' ', '')
        { Invoke-Setup251 $expected $health } | Should Throw 'reason=detailed_bridge_identity_missing:extension_code_stamp'
    }
}
