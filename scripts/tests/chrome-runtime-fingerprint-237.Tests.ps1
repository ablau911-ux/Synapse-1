# #237: execute the real deployment function, never execute installer top level.
# Pester TestDrive contains all synthetic source/deployed files.
Describe '#237 imported runtime code participates in installer stamp' {
    $installer = Join-Path $PSScriptRoot '../install-synapse-chrome-debugger.ps1'
    $tokens = $null
    $errors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
    foreach ($name in @('Get-SynapseFileSha256', 'Assert-SynapseChromeBridgeManifest', 'Get-SynapseChromeRuntimeStamp', 'Install-SynapseChromeBridgeFiles')) {
        $fn = $ast.Find({ param($node)
            $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
        }, $true)
        if ($null -eq $fn) { throw "Missing real installer $name" }
        . ([scriptblock]::Create($fn.Extent.Text))
    }
    . (Join-Path $PSScriptRoot '../synapse-browser-rows.ps1')
    $geckoPath = Join-Path $PSScriptRoot '../install-synapse-gecko-bridge.ps1'
    $geckoAst = [Management.Automation.Language.Parser]::ParseFile($geckoPath, [ref]$tokens, [ref]$errors)
    $geckoStamp = $geckoAst.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Set-SynapseGeckoCodeStamp'
    }, $true)
    if ($null -eq $geckoStamp) { throw 'Missing real Gecko stamp function' }
    . ([scriptblock]::Create($geckoStamp.Extent.Text))
    $script:GeckoCodeStampPlaceholder = 'const BRIDGE_CODE_STAMP = "unstamped";'

    function New-StampFixture([string]$Root) {
        New-Item -ItemType Directory -Path "$Root/source/adapters", "$Root/deployed/adapters" -Force | Out-Null
        [IO.File]::WriteAllText("$Root/source/service_worker.js", "import './adapters/index.js';`nimport './bridge_config.js';`nconst BRIDGE_CODE_STAMP = `"unstamped`";")
        [IO.File]::WriteAllText("$Root/source/adapters/index.js", 'export const version = 1;')
        [IO.File]::WriteAllText("$Root/source/bridge_config.js", 'export const config = 1;')
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot '../../extensions/synapse-chrome-debugger/manifest.json') -Destination "$Root/source/manifest.json"
        return $Root
    }

    function Invoke-FixtureStamp([string]$Root) {
        $row = [pscustomobject]@{
            id='chrome'; tab_target_prefix='chrome-tab:'
            extension=[pscustomobject]@{ id=('a'*32); deploy_subdir='chrome-extension'; name='Synapse Chrome Bridge' }
        }
        $null = Install-SynapseChromeBridgeFiles -SourceDir "$Root/source" -Destination "$Root/deployed" -BrowserRow $row -ProvisionAuth {
            param($Path, $Row)
            [pscustomobject]@{ok=$true}
        }
        $readback = [IO.File]::ReadAllText("$Root/deployed/service_worker.js")
        if ($readback -notmatch 'const BRIDGE_CODE_STAMP = "([A-Fa-f0-9]{64})";') { throw 'No stamped worker readback' }
        return $Matches[1]
    }
    It 'must_pass_identical_runtime_trees_produce_identical_embedded_stamps' {
        $a = New-StampFixture "$TestDrive/a"
        $b = New-StampFixture "$TestDrive/b"
        $first = Invoke-FixtureStamp $a
        $second = Invoke-FixtureStamp $b
        $second | Should BeExactly $first
        [IO.File]::ReadAllText("$a/source/service_worker.js") | Should Match 'BRIDGE_CODE_STAMP = "unstamped"'
    }

    It 'must_fail_adapter_only_update_cannot_keep_running_worker_stamp_fresh' {
        $root = New-StampFixture "$TestDrive/adapter"
        $old = Invoke-FixtureStamp $root
        [IO.File]::WriteAllText("$root/source/adapters/index.js", 'export const version = 2;')
        [IO.File]::ReadAllText("$root/source/adapters/index.js") | Should BeExactly 'export const version = 2;'
        $new = Invoke-FixtureStamp $root
        ($new -eq $old) | Should Be $false
        [IO.File]::ReadAllText("$root/deployed/adapters/index.js") | Should BeExactly 'export const version = 2;'
    }

    It 'must_fail_config_module_only_update_cannot_keep_running_worker_stamp_fresh' {
        $root = New-StampFixture "$TestDrive/config"
        $old = Invoke-FixtureStamp $root
        [IO.File]::WriteAllText("$root/source/bridge_config.js", 'export const config = 2;')
        $new = Invoke-FixtureStamp $root
        ($new -eq $old) | Should Be $false
        [IO.File]::ReadAllText("$root/deployed/bridge_config.js") | Should BeExactly 'export const config = 2;'
    }

    It 'must_fail_gecko_adapter_update_cannot_reuse_previous_runtime_stamp' {
        $root = New-StampFixture "$TestDrive/gecko"
        Copy-Item "$root/source/*" "$root/deployed" -Recurse -Force
        $old = Set-SynapseGeckoCodeStamp -StageDir "$root/deployed" -SourceWorkerPath "$root/source/service_worker.js"
        [IO.File]::WriteAllText("$root/source/adapters/index.js", 'export const version = 2;')
        Copy-Item "$root/source/*" "$root/deployed" -Recurse -Force
        [IO.File]::ReadAllText("$root/deployed/adapters/index.js") | Should BeExactly 'export const version = 2;'
        $new = Set-SynapseGeckoCodeStamp -StageDir "$root/deployed" -SourceWorkerPath "$root/source/service_worker.js"
        ($new -eq $old) | Should Be $false
    }

    It 'must_fail_invalid_placeholder_refuses_before_modifying_deployed_worker' {
        $root = New-StampFixture "$TestDrive/invalid"
        $sourceServiceWorkerPath = Join-Path (Join-Path $root 'source') 'service_worker.js'
        $path = "$root/deployed/service_worker.js"
        [IO.File]::WriteAllText($path, 'const BRIDGE_CODE_STAMP = "already-stamped";')
        [IO.File]::WriteAllText($sourceServiceWorkerPath, 'const BRIDGE_CODE_STAMP = "invalid";')
        $before = [IO.File]::ReadAllText($path)
        $caught = $null
        try { Invoke-FixtureStamp $root } catch { $caught = $_.Exception.Message }
        $expected = "SYNAPSE_CHROME_EXTENSION_CODE_STAMP_PLACEHOLDER_INVALID path=$sourceServiceWorkerPath count=0"
        $caught | Should BeExactly $expected
        [IO.File]::ReadAllText($path) | Should BeExactly $before
        # The shared fingerprint entry point must reject malformed source too;
        # deployment's earlier guard otherwise hides a worker-only hash revert.
        $stampError = $null
        try { $null = Get-SynapseChromeRuntimeStamp -SourceDir "$root/source" }
        catch { $stampError = $_.Exception.Message }
        $stampError | Should Match ('^' + [regex]::Escape($expected) + '(?: remediation=|$)')
        [IO.File]::ReadAllText($path) | Should BeExactly $before
    }
}
