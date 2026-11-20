$ErrorActionPreference = 'Stop'
# NEEDS-SEAM: production top level must use both functions below.
# Install-SynapseChromeBridgeFiles -SourceDir <string> -Destination <string>
#   -BrowserRow <object> -ProvisionAuth <scriptblock> -> deployment object.
# Get-SynapseChromeInstallOutcome -AutoInstall <object> -> object with ok/reason.
# Only function ASTs loaded. All deployment/credential fixtures under TestDrive.
Describe 'Chrome installer validation and truthful outcome #251' {
    # Describe-scope setup works with Pester 3.4 (no BeforeAll dependency).
        $installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install-synapse-chrome-debugger.ps1'
        $tokens = $null; $errors = $null
        $script:Ast251 = [System.Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw ($errors | Out-String) }
        . (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-browser-rows.ps1')
        foreach ($name in @('Install-SynapseChromeBridgeFiles', 'Get-SynapseChromeInstallOutcome', 'Get-SynapseFileSha256', 'Assert-SynapseChromeBridgeManifest', 'Get-SynapseChromeRuntimeStamp')) {
            $fn = $script:Ast251.Find({ param($n)
                $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
            }, $true)
            if ($fn) { Invoke-Expression $fn.Extent.Text }
        }
        $script:repo251 = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        function New-BridgeCase([string]$Name) {
            $root = Join-Path $TestDrive $Name
            $src = Join-Path $root 'source'; $dst = Join-Path $root 'installed'
            New-Item -ItemType Directory -Path $src,$dst -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $script:repo251 'extensions\synapse-chrome-debugger\manifest.json') -Destination $src
            $runtime = Join-Path $script:repo251 'extensions\synapse-chrome-debugger'
            Get-ChildItem -LiteralPath $runtime -Recurse -File -Filter '*.js' | ForEach-Object {
                $relative = $_.FullName.Substring($runtime.Length + 1)
                $target = Join-Path $src $relative
                New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
                Copy-Item -LiteralPath $_.FullName -Destination $target
            }
            [IO.File]::WriteAllText((Join-Path $dst 'sentinel.txt'), 'KEEP_251')
            $script:AuthCalls251 = 0
            return @{ SourceDir=$src; Destination=$dst; BrowserRow=[pscustomobject]@{
                id='chrome'; tab_target_prefix='chrome-tab:'; extension=[pscustomobject]@{
                    id=('a'*32); deploy_subdir='chrome-extension'; name='Synapse Chrome Bridge'
                }
            }; ProvisionAuth={ param($Path,$Row) $script:AuthCalls251++; [pscustomobject]@{ok=$true} } }
        }
        function Get-ExpectedRuntimeStamp251([string]$Source) {
            # Independent oracle for #237 canonical dependency fingerprint.
            $paths = [string[]]@(Get-ChildItem -LiteralPath $Source -Recurse -File -Filter '*.js' |
                ForEach-Object { $_.FullName.Substring($Source.Length + 1).Replace('\', '/') })
            [Array]::Sort($paths, [StringComparer]::Ordinal)
            $rows = foreach ($path in $paths) {
                $hash = (Get-FileHash -LiteralPath (Join-Path $Source $path) -Algorithm SHA256).Hash.ToLowerInvariant()
                "$path`0$hash`n"
            }
            $sha = [Security.Cryptography.SHA256]::Create()
            try {
                ([BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($rows -join ''))))).Replace('-', '').ToLowerInvariant()
            } finally { $sha.Dispose() }
        }
        function Read-InstalledStamp251([string]$Destination) {
            $worker = [IO.File]::ReadAllText((Join-Path $Destination 'service_worker.js'))
            if ($worker -notmatch 'const BRIDGE_CODE_STAMP = "([a-f0-9]{64})";') { throw 'Missing deployed dependency stamp' }
            return $Matches[1]
        }

    It 'must_fail_invalid_manifest_preserves_files_and_auth' {
        $case = New-BridgeCase 'bad-permission'
        $manifestPath = Join-Path $case.SourceDir 'manifest.json'
        $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
        $manifest.permissions = @($manifest.permissions | Where-Object { $_ -ne 'management' })
        $manifest | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $manifestPath
        [IO.File]::ReadAllText((Join-Path $case.Destination 'sentinel.txt')) | Should Be 'KEEP_251'
        $message = $null
        try { $null = Install-SynapseChromeBridgeFiles @case } catch { $message=$_.Exception.Message }
        $message | Should Match '^SYNAPSE_CHROME_EXTENSION_MANAGEMENT_PERMISSION_REQUIRED'
        $script:AuthCalls251 | Should Be 0
        @(Get-ChildItem -LiteralPath $case.Destination).Count | Should Be 1
        [IO.File]::ReadAllText((Join-Path $case.Destination 'sentinel.txt')) | Should Be 'KEEP_251'
    }

    It 'must_pass_deployment_preserves_dependency_fingerprint_and_browser_identity' {
        foreach ($browser in @('chrome','edge')) {
            $case = New-BridgeCase ('valid-' + $browser)
            $case.BrowserRow.id = $browser
            $case.BrowserRow.tab_target_prefix = $browser + '-tab:'
            Test-Path -LiteralPath (Join-Path $case.Destination 'service_worker.js') | Should Be $false
            $null = Install-SynapseChromeBridgeFiles @case
            Read-InstalledStamp251 $case.Destination | Should Be (Get-ExpectedRuntimeStamp251 $case.SourceDir)
            $cfg = Get-Content -Raw -LiteralPath (Join-Path $case.Destination 'bridge-config.json') | ConvertFrom-Json
            $cfg.browserKind | Should Be $browser
            $cfg.tabTargetPrefix | Should Be ($browser + '-tab:')
            $script:AuthCalls251 | Should Be 1
        }
    }

    It 'must_fail_worker_or_import_only_change_cannot_reuse_old_stamp' {
        foreach ($relative in @('service_worker.js', 'bridge_config.js', 'adapters/index.js')) {
            $case = New-BridgeCase ('dependency-' + $relative.Replace('/', '-'))
            $sourcePath = Join-Path $case.SourceDir $relative
            Test-Path -LiteralPath $sourcePath | Should Be $true
            $before = [IO.File]::ReadAllText($sourcePath)
            $null = Install-SynapseChromeBridgeFiles @case
            $old = Read-InstalledStamp251 $case.Destination
            $old | Should Be (Get-ExpectedRuntimeStamp251 $case.SourceDir)
            [IO.File]::WriteAllText($sourcePath, ($before + "`n// dependency mutation #251"))
            [IO.File]::ReadAllText($sourcePath) | Should Be ($before + "`n// dependency mutation #251")
            $null = Install-SynapseChromeBridgeFiles @case
            $new = Read-InstalledStamp251 $case.Destination
            $new | Should Be (Get-ExpectedRuntimeStamp251 $case.SourceDir)
            ($new -eq $old) | Should Be $false
            [IO.File]::ReadAllText((Join-Path $case.SourceDir 'service_worker.js')) | Should Match 'BRIDGE_CODE_STAMP = "unstamped"'
            # An invalid dependency update must not replace this verified runtime.
            $workerPath = Join-Path $case.Destination 'service_worker.js'
            $installedWorker = [IO.File]::ReadAllText($workerPath)
            $installedDependency = [IO.File]::ReadAllText((Join-Path $case.Destination $relative))
            [IO.File]::WriteAllText($sourcePath, ([IO.File]::ReadAllText($sourcePath) + "`n// rejected update #251"))
            $manifestPath = Join-Path $case.SourceDir 'manifest.json'
            $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
            $manifest.permissions = @($manifest.permissions | Where-Object { $_ -ne 'management' })
            $manifest | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $manifestPath
            $script:AuthCalls251 = 0
            $message = $null
            try { $null = Install-SynapseChromeBridgeFiles @case } catch { $message = $_.Exception.Message }
            $message | Should Match '^SYNAPSE_CHROME_EXTENSION_MANAGEMENT_PERMISSION_REQUIRED'
            [IO.File]::ReadAllText($workerPath) | Should BeExactly $installedWorker
            [IO.File]::ReadAllText((Join-Path $case.Destination $relative)) | Should BeExactly $installedDependency
            $script:AuthCalls251 | Should Be 0
        }
    }

    It 'must_pass_generated_bridge_config_json_is_excluded_from_code_identity' {
        $case = New-BridgeCase 'generated-config'
        $null = Install-SynapseChromeBridgeFiles @case
        $old = Read-InstalledStamp251 $case.Destination
        [IO.File]::WriteAllText((Join-Path $case.SourceDir 'bridge-config.json'), '{"synthetic":"changed"}')
        [IO.File]::ReadAllText((Join-Path $case.SourceDir 'bridge-config.json')) | Should Be '{"synthetic":"changed"}'
        $null = Install-SynapseChromeBridgeFiles @case
        Read-InstalledStamp251 $case.Destination | Should Be $old
        $old | Should Be (Get-ExpectedRuntimeStamp251 $case.SourceDir)
        $cfg = Get-Content -Raw -LiteralPath (Join-Path $case.Destination 'bridge-config.json') | ConvertFrom-Json
        $cfg.browserKind | Should Be 'chrome'
    }

    It 'must_fail_deferred_or_failed_runtime_cannot_report_success' {
        foreach ($reason in @('deferred','runtime_failed','no_profile_dirs')) {
            $input = [pscustomobject]@{ok=$false;reason=$reason}
            $result = Get-SynapseChromeInstallOutcome -AutoInstall $input
            $result.ok | Should Be $false
            $result.reason | Should Be $reason
            $input.ok | Should Be $false
        }
    }

    It 'must_pass_verified_runtime_retains_successful_outcome' {
        $input = [pscustomobject]@{ok=$true;reason='live_runtime_verified'}
        $result = Get-SynapseChromeInstallOutcome -AutoInstall $input
        $result.ok | Should Be $true
        $result.reason | Should Be 'live_runtime_verified'
    }

    It 'must_fail_removing_validation_or_path_contract_guards' {
        # Last-resort source guard for protections surrounding extracted deployment.
        $source = $script:Ast251.Extent.Text
        $source | Should Match 'SYNAPSE_CHROME_EXTENSION_STABLE_PATH_INVALID'
        $source | Should Match 'SYNAPSE_CHROME_EXTENSION_CODE_STAMP_PLACEHOLDER_INVALID'
        $source | Should Match 'SYNAPSE_BROWSER_BRIDGE_CONFIG_READBACK_FAILED'
        $source | Should Match 'SYNAPSE_CHROME_EXTENSION_MANAGEMENT_PERMISSION_REQUIRED'
        # Guard presence elsewhere is insufficient: validation must precede the
        # first file mutation in the real deployment function (#251).
        $deploy = $script:Ast251.Find({ param($n)
            $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $n.Name -eq 'Install-SynapseChromeBridgeFiles'
        }, $true)
        $commands = @($deploy.FindAll({ param($n)
            $n -is [System.Management.Automation.Language.CommandAst]
        }, $true))
        $validate = @($commands | Where-Object { $_.GetCommandName() -eq 'Assert-SynapseChromeBridgeManifest' })
        $copy = @($commands | Where-Object { $_.GetCommandName() -eq 'Copy-Item' })
        $validate.Count | Should Be 1
        $copy.Count | Should Be 1
        ($validate[0].Extent.StartOffset -lt $copy[0].Extent.StartOffset) | Should Be $true
    }

    It 'must_fail_installer_entrypoint_cannot_bypass_deployment_or_outcome_seams' {
        foreach ($name in @('Install-SynapseChromeBridgeFiles', 'Get-SynapseChromeInstallOutcome')) {
            $calls = @($script:Ast251.FindAll({ param($n)
                $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $name
            }, $true) | Where-Object {
                $parent = $_.Parent
                while ($null -ne $parent -and $parent -isnot [System.Management.Automation.Language.FunctionDefinitionAst]) {
                    $parent = $parent.Parent
                }
                $null -eq $parent
            })
            $calls.Count | Should Be 1
        }
        # Verify the actual outcome call statement preserves a failed runtime;
        # merely counting calls misses a seam that unconditionally returns ok.
        $outcomeCall = $calls[0]
        $runtimeOutcome = [pscustomobject]@{ ok=$false; reason='deferred' }
        $entryOutcome = & ([scriptblock]::Create($outcomeCall.Extent.Text))
        $entryOutcome.ok | Should Be $false
        $entryOutcome.reason | Should Be 'deferred'
    }
}
