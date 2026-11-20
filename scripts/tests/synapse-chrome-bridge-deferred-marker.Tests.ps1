$ErrorActionPreference = 'Stop'

Describe 'Synapse Chrome bridge deferred-repair marker (#132)' {
    BeforeAll {
        $installerPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'install-synapse-chrome-debugger.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($installerPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $functionAst = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Set-SynapseChromeBridgeDeferredRepairMarker'
            }, $true)
        if ($null -eq $functionAst) { throw 'installer function is missing: Set-SynapseChromeBridgeDeferredRepairMarker' }
        Invoke-Expression $functionAst.Extent.Text
        $script:marker = Join-Path ([System.IO.Path]::GetTempPath()) ("synapse-132-" + [guid]::NewGuid().ToString('N') + '\sub\marker.txt')
    }

    AfterAll {
        $root = Split-Path -Parent (Split-Path -Parent $script:marker)
        if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
    }

    It 'writes the reason for a window-less deferral' {
        $result = [pscustomobject]@{ reason = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon' }
        Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall $result -MarkerPath $script:marker
        $written = [System.IO.File]::ReadAllText($script:marker)
        $written | Should Be 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
    }

    It 'writes the reason for the reloadself deferral' {
        $result = [pscustomobject]@{ reason = 'existing_ready_extension_code_reload_deferred_to_daemon_reloadself' }
        Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall $result -MarkerPath $script:marker
        $written = [System.IO.File]::ReadAllText($script:marker)
        $written | Should Be 'existing_ready_extension_code_reload_deferred_to_daemon_reloadself'
    }

    It 'clears the marker on a non-deferred result and tolerates it being absent' {
        $result = [pscustomobject]@{ reason = 'installed_unpacked_extension_in_active_profile' }
        Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall $result -MarkerPath $script:marker
        Test-Path -LiteralPath $script:marker | Should Be $false
        Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall $result -MarkerPath $script:marker
        Test-Path -LiteralPath $script:marker | Should Be $false
    }
}
