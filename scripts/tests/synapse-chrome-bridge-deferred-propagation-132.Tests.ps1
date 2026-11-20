$ErrorActionPreference = 'Stop'

# #132 raid tests (Pester 3.4.0). A deferred bridge repair must reach daemon bridge health:
# installer -> marker file %APPDATA%\synapse\chrome-bridge-deferred-repair.txt -> Rust reader
# (crates/synapse-mcp/src/chrome_bridge_deferred_repair.rs) -> `deferred_repair=<reason>`.
# These tests pin the cross-language contract with the REAL writer and the real constants:
# exact marker bytes (ASCII reason, no BOM, no newline), file name, location, reason format.
Describe 'Chrome bridge deferred repair marker propagation contract (#132)' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:repoRoot = Split-Path -Parent $scriptsDir
        $script:installerPath = Join-Path $scriptsDir 'install-synapse-chrome-debugger.ps1'
        $script:setupPath = Join-Path $scriptsDir 'synapse-setup.ps1'
        $script:rustPath = Join-Path $script:repoRoot 'crates\synapse-mcp\src\chrome_bridge_deferred_repair.rs'
        $tokens = $null
        $errors = $null
        $script:installerAst = [System.Management.Automation.Language.Parser]::ParseFile($script:installerPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $fn = $script:installerAst.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Set-SynapseChromeBridgeDeferredRepairMarker'
            }, $true)
        if ($null -eq $fn) { throw 'installer function is missing: Set-SynapseChromeBridgeDeferredRepairMarker' }
        Invoke-Expression $fn.Extent.Text

        $setupAst = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        $assign = $setupAst.Find({
                param($n)
                $n -is [System.Management.Automation.Language.AssignmentStatementAst] -and
                $n.Left.Extent.Text -eq '$SynapseChromeBridgeUiReloadDeferredReason'
            }, $true)
        if ($null -eq $assign) { throw 'setup constant is missing: $SynapseChromeBridgeUiReloadDeferredReason' }
        $script:reason = [string](Invoke-Expression $assign.Right.Extent.Text)

        $script:rust = [System.IO.File]::ReadAllText($script:rustPath)
        $script:rustMarkerFile = [regex]::Match($script:rust, 'const MARKER_FILE: &str = "([^"]+)";').Groups[1].Value
        $script:rustMaxLen = [int][regex]::Match($script:rust, 'const MAX_REASON_LEN: usize = (\d+);').Groups[1].Value
    }

    It 'must_pass the setup deferral reason is written as exact ASCII bytes the Rust reader accepts' {
        $script:reason | Should Be 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
        $marker = Join-Path $TestDrive 'pass\synapse\marker.txt'

        Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall ([pscustomobject]@{ reason = $script:reason }) -MarkerPath $marker

        # Assert publication before readback: a no-op writer must fail for #132.
        (Test-Path -LiteralPath $marker -PathType Leaf) | Should Be $true
        $bytes = [System.IO.File]::ReadAllBytes($marker)
        ([System.Text.Encoding]::ASCII.GetString($bytes)) | Should BeExactly $script:reason
        $bytes.Length | Should Be $script:reason.Length
        ($script:reason.Length -le $script:rustMaxLen) | Should Be $true
        $script:reason | Should Match '^[A-Za-z0-9_]+$'
    }

    It 'must_pass the installer writes the marker at the exact location the daemon reads' {
        $script:rustMarkerFile | Should Be 'chrome-bridge-deferred-repair.txt'
        $script:rust | Should Match 'join\("synapse"\)\.join\(MARKER_FILE\)'
        $call = $script:installerAst.FindAll({
                param($n)
                $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'Set-SynapseChromeBridgeDeferredRepairMarker'
            }, $true) | Select-Object -First 1
        $null -eq $call | Should Be $false
        $call.Extent.Text | Should Match ([regex]::Escape("Join-Path `$env:APPDATA 'synapse\$($script:rustMarkerFile)'"))
        $installerText = [System.IO.File]::ReadAllText($script:installerPath)
        $installerText | Should Match ([regex]::Escape("reason = '$($script:reason)'"))
    }

    It 'must_fail a non-deferred result clears a stale marker' {
        $marker = Join-Path $TestDrive 'clear\synapse\marker.txt'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marker) | Out-Null
        [System.IO.File]::WriteAllText($marker, $script:reason)

        Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall ([pscustomobject]@{ reason = 'existing_ready_extension_ui_reload_invoked' }) -MarkerPath $marker

        Test-Path -LiteralPath $marker | Should Be $false
    }

    It 'must_fail a malformed deferred reason is never written into the health text' {
        $marker = Join-Path $TestDrive 'bad\synapse\marker.txt'
        foreach ($bad in @("x y_deferred_to_daemon", "a_deferred_to_daemon`r`nok=true", 'semi;colon_deferred_to_daemon', '')) {
            # A malformed result must also invalidate an existing health marker.
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $marker) | Out-Null
            [IO.File]::WriteAllText($marker, $script:reason)
            Set-SynapseChromeBridgeDeferredRepairMarker -AutoInstall ([pscustomobject]@{ reason = $bad }) -MarkerPath $marker
            Test-Path -LiteralPath $marker | Should Be $false
        }
    }
}
