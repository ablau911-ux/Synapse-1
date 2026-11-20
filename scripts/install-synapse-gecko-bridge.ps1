param(
    # Gecko-family browser row (crates\synapse-browsers\browsers\<id>.json).
    [string]$Browser = 'firefox',
    [string]$BrowsersDir,
    # Read-only: print the install plan and exit; writes nothing.
    [switch]$PlanOnly,
    # Define the functions only (dot-source for tests); runs nothing.
    [switch]$LibraryOnly,
    # The AMO-signed XPI (see extensions\synapse-firefox-bridge\SIGNING.md).
    # Without it the installer only stages the unsigned package for signing.
    [string]$SignedXpiPath,
    # manual: the owner installs the signed XPI once from about:addons.
    # policy: force-install through the row's ExtensionSettings policy
    # (HKCU\Software\Policies is admin-only, so this needs an elevated shell).
    [ValidateSet('manual', 'policy')]
    [string]$InstallMode = 'manual',
    # Only create the token and its managed-storage delivery (development
    # loop with a temporarily loaded or unsigned package); no XPI is touched.
    [switch]$ManagedStorageOnly
)

# Firefox (gecko) bridge installer. Firefox release builds load only signed
# extensions, and the signed package is uploaded to AMO, so:
#   1. stage   - assemble the unsigned package (shared worker + gecko adapter +
#                Firefox manifest + bridge-config.json), stamp the worker code,
#                prove no secret is inside, print the web-ext sign command;
#   2. install - with -SignedXpiPath: verify the signed XPI, create the
#                per-kind registration token, deliver it through Firefox
#                managed storage (HKCU, never inside the package), copy the XPI
#                to a stable path, optionally write the force-install policy,
#                read every value back.
# Firefox itself is never started, stopped or restarted here, and its profile
# directories are never written: the owner restarts Firefox at a chosen time.

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'synapse-browser-rows.ps1')
$script:GeckoRow = Get-SynapseBrowserRow -Browser $Browser -BrowsersDir $BrowsersDir
if ([string]$script:GeckoRow.family -cne 'gecko') {
    throw "SYNAPSE_BROWSER_BRIDGE_FAMILY_MISMATCH browser=$Browser family=$($script:GeckoRow.family) installer=gecko remediation=use scripts\install-synapse-browser-bridge.ps1 -Browser $Browser"
}
$script:GeckoRepoRoot = Split-Path -Parent $PSScriptRoot
# Code layout (not browser data): the shared worker lives in the Chromium
# extension dir; only these files of it belong in the Firefox package.
$script:GeckoSharedWorkerDir = Join-Path $script:GeckoRepoRoot 'extensions\synapse-chrome-debugger'
$script:GeckoSharedFiles = @('service_worker.js', 'bridge_config.js', 'adapters\index.js', 'adapters\chromium.js', 'adapters\gecko.js')
# Firefox engine fact (MDN "Native manifests", managed storage on Windows).
$script:GeckoManagedStorageRoot = 'Software\Mozilla\ManagedStorage'
$script:GeckoCodeStampPlaceholder = 'const BRIDGE_CODE_STAMP = "unstamped";'
# Entries AMO signing adds; an XPI with neither is unsigned.
$script:GeckoSignatureEntries = @('META-INF/cose.sig', 'META-INF/mozilla.rsa')

function Get-SynapseGeckoBuildId {
    $workerPath = Join-Path $script:GeckoSharedWorkerDir 'service_worker.js'
    $source = [System.IO.File]::ReadAllText($workerPath)
    if ($source -notmatch 'const\s+BRIDGE_BUILD_ID\s*=\s*"([A-Za-z0-9._-]+)";') {
        throw "SYNAPSE_GECKO_BRIDGE_BUILD_ID_MISSING path=$workerPath"
    }
    return [string]$Matches[1]
}

function Get-SynapseGeckoInstallPlan {
    foreach ($name in @('LOCALAPPDATA', 'APPDATA')) {
        if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
            throw "SYNAPSE_GECKO_BRIDGE_ENV_MISSING variable=$name"
        }
    }
    $row = $script:GeckoRow
    $geckoId = [string]$row.extension.id
    $deployRoot = Join-Path $env:LOCALAPPDATA ('synapse\' + [string]$row.extension.deploy_subdir)
    $policyKey = if ($null -eq $row.policy) { $null } else { "HKCU:\$($row.policy.root)" }
    [pscustomobject]@{
        browser = [string]$row.id
        gecko_id = $geckoId
        package_source_dir = Join-Path $script:GeckoRepoRoot ('extensions\' + [string]$row.extension.manifest_dir)
        shared_worker_dir = $script:GeckoSharedWorkerDir
        stage_dir = Join-Path $deployRoot ((Get-SynapseGeckoBuildId) + '\source')
        signed_dir = Join-Path $deployRoot 'signed'
        token_path = Join-Path $env:APPDATA ('synapse\' + [string]$row.registration_token_file)
        managed_storage_manifest_path = Join-Path $env:APPDATA "synapse\browser-managed-storage\$geckoId.json"
        managed_storage_key = "HKCU:\$($script:GeckoManagedStorageRoot)\$geckoId"
        policy_key = $policyKey
        policy_value = if ($null -eq $row.policy) { $null } else { [string]$row.policy.extension_settings_value }
        extensions_page_url = [string]$row.extension.extensions_page_url
        install_mode = $InstallMode
        firefox_process_control = 'never'
        restart_required = $true
    }
}

function Assert-SynapseGeckoPackageHasNoSecret {
    param([Parameter(Mandatory = $true)][string]$PackageDir, [string]$Token)
    foreach ($file in @(Get-ChildItem -LiteralPath $PackageDir -Recurse -File -Force)) {
        if ($file.Name -ieq 'bridge-auth.json') {
            throw "SYNAPSE_GECKO_BRIDGE_SECRET_IN_PACKAGE file=$($file.FullName) reason=bundled_registration_credential remediation=the gecko bridge reads its token from Firefox managed storage only"
        }
        if (-not [string]::IsNullOrEmpty($Token) -and [System.IO.File]::ReadAllText($file.FullName).Contains($Token)) {
            throw "SYNAPSE_GECKO_BRIDGE_SECRET_IN_PACKAGE file=$($file.FullName) reason=registration_token_bytes remediation=remove the token from the package before signing"
        }
    }
}

function Copy-SynapseGeckoPackageFiles {
    param([Parameter(Mandatory = $true)][object]$Plan)
    $stageRoot = Split-Path -Parent (Split-Path -Parent $Plan.stage_dir)
    $stageFull = [System.IO.Path]::GetFullPath($Plan.stage_dir)
    if (-not $stageFull.StartsWith([System.IO.Path]::GetFullPath($stageRoot) + '\', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "SYNAPSE_GECKO_BRIDGE_STAGE_PATH_INVALID stage=$stageFull root=$stageRoot"
    }
    if (Test-Path -LiteralPath $stageFull) {
        Remove-Item -LiteralPath $stageFull -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path (Join-Path $stageFull 'adapters') | Out-Null
    foreach ($relative in $script:GeckoSharedFiles) {
        Copy-Item -LiteralPath (Join-Path $Plan.shared_worker_dir $relative) -Destination (Join-Path $stageFull $relative)
    }
    # Every Firefox package file except documentation (SIGNING.md).
    foreach ($file in @(Get-ChildItem -LiteralPath $Plan.package_source_dir -File | Where-Object { $_.Extension -ine '.md' })) {
        Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $stageFull $file.Name)
    }
    return $stageFull
}

function Set-SynapseGeckoCodeStamp {
    param([Parameter(Mandatory = $true)][string]$StageDir, [Parameter(Mandatory = $true)][string]$SourceWorkerPath)
    $workerPath = Join-Path $StageDir 'service_worker.js'
    $text = [System.IO.File]::ReadAllText($workerPath)
    $count = ([regex]::Matches($text, [regex]::Escape($script:GeckoCodeStampPlaceholder))).Count
    if ($count -ne 1) {
        throw "SYNAPSE_GECKO_BRIDGE_CODE_STAMP_PLACEHOLDER_INVALID path=$SourceWorkerPath count=$count"
    }
    # Same path/hash encoding as Chrome; fingerprint the assembled runtime,
    # not checkout paths or generated bridge-config.json data.
    $root = [System.IO.Path]::GetFullPath($StageDir).TrimEnd('\', '/')
    $runtimePaths = [string[]]@(Get-ChildItem -LiteralPath $root -Recurse -File -Filter '*.js' | ForEach-Object {
        $_.FullName.Substring($root.Length + 1).Replace('\', '/')
    })
    [Array]::Sort($runtimePaths, [StringComparer]::Ordinal)
    $runtimeRows = foreach ($relative in $runtimePaths) {
        $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $root $relative)).Hash.ToLowerInvariant()
        "$relative`0$digest`n"
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes(($runtimeRows -join ''))
        $stamp = ([BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
    $stamped = $text.Replace($script:GeckoCodeStampPlaceholder, "const BRIDGE_CODE_STAMP = `"$stamp`";")
    [System.IO.File]::WriteAllText($workerPath, $stamped, [System.Text.UTF8Encoding]::new($false))
    return $stamp
}

function Write-SynapseGeckoBridgeConfig {
    param([Parameter(Mandatory = $true)][string]$StageDir)
    $path = Join-Path $StageDir 'bridge-config.json'
    $json = ConvertTo-Json -Depth 4 -InputObject (New-SynapseBridgeConfig -Row $script:GeckoRow)
    [System.IO.File]::WriteAllText($path, $json, [System.Text.UTF8Encoding]::new($false))
    $readback = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
    if ([string]$readback.browserKind -cne [string]$script:GeckoRow.id -or
        [string]$readback.family -cne 'gecko' -or
        [string]$readback.expectedExtensionId -cne [string]$script:GeckoRow.extension.id) {
        throw "SYNAPSE_GECKO_BRIDGE_CONFIG_READBACK_FAILED path=$path"
    }
}

function Invoke-SynapseGeckoStage {
    param([Parameter(Mandatory = $true)][object]$Plan)
    $stageDir = Copy-SynapseGeckoPackageFiles -Plan $Plan
    $stamp = Set-SynapseGeckoCodeStamp -StageDir $stageDir -SourceWorkerPath (Join-Path $Plan.shared_worker_dir 'service_worker.js')
    Write-SynapseGeckoBridgeConfig -StageDir $stageDir
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $stageDir 'manifest.json') | ConvertFrom-Json
    if ([string]$manifest.browser_specific_settings.gecko.id -cne $Plan.gecko_id) {
        throw "SYNAPSE_GECKO_BRIDGE_MANIFEST_ID_MISMATCH manifest=$($manifest.browser_specific_settings.gecko.id) row=$($Plan.gecko_id)"
    }
    $token = if (Test-Path -LiteralPath $Plan.token_path -PathType Leaf) { [System.IO.File]::ReadAllText($Plan.token_path).Trim() } else { '' }
    Assert-SynapseGeckoPackageHasNoSecret -PackageDir $stageDir -Token $token
    [pscustomobject]@{
        stage_dir = $stageDir
        code_stamp = $stamp
        manifest_version = [string]$manifest.version
        web_ext_sign_command = "web-ext sign --channel=unlisted --source-dir `"$stageDir`" --artifacts-dir `"$($Plan.signed_dir)`""
    }
}

function Open-SynapseGeckoZip {
    param([Parameter(Mandatory = $true)][string]$Path)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    try {
        return [System.IO.Compression.ZipFile]::OpenRead($Path)
    } catch {
        throw "SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_INVALID path=$Path reason=not_a_zip error=$($_.Exception.Message)"
    }
}

function Read-SynapseGeckoZipEntryText {
    param([Parameter(Mandatory = $true)][object]$Entry)
    $reader = [System.IO.StreamReader]::new($Entry.Open())
    try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
}

function Test-SynapseGeckoSignedXpi {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Token)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_MISSING path=$Path remediation=sign the staged package (extensions\synapse-firefox-bridge\SIGNING.md)"
    }
    $zip = Open-SynapseGeckoZip -Path $Path
    try {
        $names = @($zip.Entries | ForEach-Object { $_.FullName })
        $signed = @($script:GeckoSignatureEntries | Where-Object { $names -contains $_ }).Count -gt 0
        $manifestEntry = $zip.GetEntry('manifest.json')
        if (-not $signed -or $null -eq $manifestEntry -or $names -notcontains 'service_worker.js') {
            throw "SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_INVALID path=$Path signed=$signed reason=missing_signature_manifest_or_worker"
        }
        $geckoId = [string](((Read-SynapseGeckoZipEntryText -Entry $manifestEntry) | ConvertFrom-Json).browser_specific_settings.gecko.id)
        if ($geckoId -cne [string]$script:GeckoRow.extension.id) {
            throw "SYNAPSE_GECKO_BRIDGE_SIGNED_XPI_INVALID path=$Path reason=gecko_id_mismatch gecko_id=$geckoId"
        }
        foreach ($entry in $zip.Entries) {
            if ($entry.Name -ieq 'bridge-auth.json' -or (-not [string]::IsNullOrEmpty($Token) -and (Read-SynapseGeckoZipEntryText -Entry $entry).Contains($Token))) {
                throw "SYNAPSE_GECKO_BRIDGE_SECRET_IN_PACKAGE path=$Path entry=$($entry.FullName)"
            }
        }
        return [pscustomobject]@{ path = $Path; signed = $signed; entry_count = $names.Count }
    } finally {
        $zip.Dispose()
    }
}

function ConvertTo-SynapseGeckoFileUrl {
    param([Parameter(Mandatory = $true)][string]$Path)
    $segments = [System.IO.Path]::GetFullPath($Path).Split('\')
    $escaped = @($segments[0]) + @($segments | Select-Object -Skip 1 | ForEach-Object { [System.Uri]::EscapeDataString($_) })
    return 'file:///' + ($escaped -join '/')
}

function New-SynapseGeckoManagedStorageManifest {
    param([Parameter(Mandatory = $true)][string]$ExtensionId, [Parameter(Mandatory = $true)][string]$Token)
    [ordered]@{
        name = $ExtensionId
        description = 'Synapse bridge registration credential (written by scripts\install-synapse-gecko-bridge.ps1)'
        type = 'storage'
        data = [ordered]@{ registrationToken = $Token }
    }
}

function Merge-SynapseGeckoExtensionSettings {
    param([AllowNull()][string]$ExistingJson, [Parameter(Mandatory = $true)][string]$ExtensionId, [Parameter(Mandatory = $true)][string]$InstallUrl)
    $merged = [ordered]@{}
    if (-not [string]::IsNullOrWhiteSpace($ExistingJson)) {
        try { $existing = ConvertFrom-Json -InputObject $ExistingJson } catch { $existing = $null }
        if ($existing -isnot [System.Management.Automation.PSCustomObject]) {
            throw "SYNAPSE_GECKO_BRIDGE_POLICY_EXISTING_INVALID reason=existing ExtensionSettings is not a JSON object; refusing to overwrite another administrator's policy"
        }
        foreach ($property in $existing.PSObject.Properties) { $merged[$property.Name] = $property.Value }
    }
    $merged[$ExtensionId] = [ordered]@{ installation_mode = 'force_installed'; install_url = $InstallUrl }
    return ConvertTo-Json -InputObject $merged -Depth 10 -Compress
}

function Test-SynapseGeckoElevated {
    $principal = [System.Security.Principal.WindowsPrincipal]::new([System.Security.Principal.WindowsIdentity]::GetCurrent())
    return $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Assert-SynapseGeckoPolicyWritable {
    param([Parameter(Mandatory = $true)][string]$InstallMode)
    if ($InstallMode -eq 'policy' -and -not (Test-SynapseGeckoElevated)) {
        throw 'SYNAPSE_GECKO_BRIDGE_POLICY_REQUIRES_ELEVATION reason=HKCU\Software\Policies grants users ReadKey only remediation=rerun from an elevated PowerShell, or use -InstallMode manual'
    }
}

function Initialize-SynapseGeckoToken {
    param([Parameter(Mandatory = $true)][object]$Plan)
    & (Join-Path $PSScriptRoot 'provision-synapse-chrome-auth.ps1') -TokenOnly -Browser $Plan.browser -BrowsersDir $BrowsersDir | Out-Null
    $token = [System.IO.File]::ReadAllText($Plan.token_path).Trim()
    if ($token -cnotmatch '\A[0-9a-f]{64}\z') {
        throw "SYNAPSE_GECKO_BRIDGE_TOKEN_INVALID path=$($Plan.token_path)"
    }
    return $token
}

function Write-SynapseGeckoManagedStorage {
    param([Parameter(Mandatory = $true)][object]$Plan, [Parameter(Mandatory = $true)][string]$Token)
    . (Join-Path $PSScriptRoot 'synapse-private-file.ps1')
    $json = ConvertTo-Json -Depth 4 -InputObject (New-SynapseGeckoManagedStorageManifest -ExtensionId $Plan.gecko_id -Token $Token)
    Write-PrivateAtomicFile -Path $Plan.managed_storage_manifest_path -Content $json -Replace $true
    New-Item -Path $Plan.managed_storage_key -Force | Out-Null
    Set-ItemProperty -LiteralPath $Plan.managed_storage_key -Name '(default)' -Value $Plan.managed_storage_manifest_path
    $registered = (Get-ItemProperty -LiteralPath $Plan.managed_storage_key).'(default)'
    $readback = Get-Content -Raw -LiteralPath $Plan.managed_storage_manifest_path | ConvertFrom-Json
    if ($registered -ne $Plan.managed_storage_manifest_path -or $readback.data.registrationToken -cne $Token -or $readback.name -cne $Plan.gecko_id) {
        throw "SYNAPSE_GECKO_BRIDGE_MANAGED_STORAGE_READBACK_FAILED key=$($Plan.managed_storage_key) manifest=$($Plan.managed_storage_manifest_path)"
    }
    return [pscustomobject]@{ key = $Plan.managed_storage_key; manifest = $Plan.managed_storage_manifest_path; readback_matches = $true }
}

function Write-SynapseGeckoForceInstallPolicy {
    param([Parameter(Mandatory = $true)][object]$Plan, [Parameter(Mandatory = $true)][string]$InstallUrl)
    if ($null -eq $Plan.policy_key) {
        throw "SYNAPSE_GECKO_BRIDGE_POLICY_ROOT_MISSING browser=$($Plan.browser) remediation=declare policy.root in browsers\$($Plan.browser).json or use -InstallMode manual"
    }
    $existing = (Get-ItemProperty -LiteralPath $Plan.policy_key -ErrorAction SilentlyContinue).($Plan.policy_value)
    $merged = Merge-SynapseGeckoExtensionSettings -ExistingJson (@($existing) -join "`n") -ExtensionId $Plan.gecko_id -InstallUrl $InstallUrl
    New-Item -Path $Plan.policy_key -Force | Out-Null
    # Firefox's GPO reader parses a REG_MULTI_SZ policy value as JSON.
    New-ItemProperty -LiteralPath $Plan.policy_key -Name $Plan.policy_value -PropertyType MultiString -Value @($merged) -Force | Out-Null
    $readback = ((Get-ItemProperty -LiteralPath $Plan.policy_key).($Plan.policy_value) -join "`n") | ConvertFrom-Json
    $entry = $readback.($Plan.gecko_id)
    if ($entry.installation_mode -cne 'force_installed' -or $entry.install_url -cne $InstallUrl) {
        throw "SYNAPSE_GECKO_BRIDGE_POLICY_READBACK_FAILED key=$($Plan.policy_key) value=$($Plan.policy_value)"
    }
    return [pscustomobject]@{ key = $Plan.policy_key; value = $Plan.policy_value; install_url = $InstallUrl; readback_matches = $true }
}

function Install-SynapseGeckoSignedXpi {
    param([Parameter(Mandatory = $true)][object]$Plan, [Parameter(Mandatory = $true)][string]$SignedXpiPath, [Parameter(Mandatory = $true)][string]$InstallMode)
    # Structural check first: a refused XPI writes nothing (not even the token).
    Test-SynapseGeckoSignedXpi -Path $SignedXpiPath | Out-Null
    Assert-SynapseGeckoPolicyWritable -InstallMode $InstallMode
    $token = Initialize-SynapseGeckoToken -Plan $Plan
    $xpi = Test-SynapseGeckoSignedXpi -Path $SignedXpiPath -Token $token
    New-Item -ItemType Directory -Force -Path $Plan.signed_dir | Out-Null
    $stablePath = Join-Path $Plan.signed_dir "$($Plan.gecko_id).xpi"
    Copy-Item -LiteralPath $SignedXpiPath -Destination $stablePath -Force
    $sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $stablePath).Hash.ToLowerInvariant()
    $managed = Write-SynapseGeckoManagedStorage -Plan $Plan -Token $token
    $installUrl = ConvertTo-SynapseGeckoFileUrl -Path $stablePath
    $policy = if ($InstallMode -eq 'policy') { Write-SynapseGeckoForceInstallPolicy -Plan $Plan -InstallUrl $installUrl } else { $null }
    $nextStep = if ($InstallMode -eq 'policy') {
        'restart Firefox at a time you choose; the policy force-installs the bridge on startup'
    } else {
        "restart Firefox at a time you choose, then open $($Plan.extensions_page_url) > gear > Install Add-on From File > $stablePath"
    }
    [pscustomobject]@{
        ok = $true
        browser = $Plan.browser
        status = 'installed_pending_firefox_restart'
        signed_xpi = $stablePath
        signed_xpi_sha256 = $sha256
        signed_xpi_entries = $xpi.entry_count
        install_url = $installUrl
        managed_storage = $managed
        policy = $policy
        restart_required = $true
        next_step = $nextStep
    }
}

if ($LibraryOnly) {
    return
}
$plan = Get-SynapseGeckoInstallPlan
if ($PlanOnly) {
    [pscustomobject]@{ ok = $true; browser = $plan.browser; status = 'planned'; plan = $plan }
    return
}
if ($ManagedStorageOnly) {
    $managed = Write-SynapseGeckoManagedStorage -Plan $plan -Token (Initialize-SynapseGeckoToken -Plan $plan)
    [pscustomobject]@{ ok = $true; browser = $plan.browser; status = 'managed_storage_written'; managed_storage = $managed; restart_required = $true }
    return
}
if (-not [string]::IsNullOrWhiteSpace($SignedXpiPath)) {
    Install-SynapseGeckoSignedXpi -Plan $plan -SignedXpiPath $SignedXpiPath -InstallMode $InstallMode
    return
}
$staged = Invoke-SynapseGeckoStage -Plan $plan
[pscustomobject]@{
    ok = $true
    browser = $plan.browser
    status = 'signing_required'
    reason = 'firefox_release_loads_only_signed_extensions'
    staged = $staged
    plan = $plan
    next_step = "owner signs the staged package (extensions\synapse-firefox-bridge\SIGNING.md), then rerun with -SignedXpiPath <signed .xpi>"
}
