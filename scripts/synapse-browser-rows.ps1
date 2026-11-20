# Shared readers for the declared browser rows (crates\synapse-browsers\browsers\*.json).
# Dot-source this file. The daemon (Rust, synapse-browsers crate) and every
# installer read the SAME rows (Iron Law 9: browser names, paths, registry keys
# and wire ids live in data, not in scripts).

function Get-SynapseBrowserAppPathsKey {
    # Windows App Paths registration root (OS constant, not browser data).
    'Software\Microsoft\Windows\CurrentVersion\App Paths'
}

function Get-SynapseBridgeConfigSchemaVersion {
    # Must equal BRIDGE_CONFIG_SCHEMA_VERSION in bridge_config.js.
    1
}

function Get-SynapseBrowserRowsDir {
    param([string]$BrowsersDir)

    if (-not [string]::IsNullOrWhiteSpace($BrowsersDir)) {
        return [System.IO.Path]::GetFullPath($BrowsersDir)
    }
    $repoRoot = Split-Path -Parent $PSScriptRoot
    $candidates = @(
        (Join-Path $repoRoot 'crates\synapse-browsers\browsers'),
        (Join-Path $PSScriptRoot 'browsers')
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Container) {
            return [System.IO.Path]::GetFullPath($candidate)
        }
    }
    throw "SYNAPSE_BROWSER_ROWS_DIR_MISSING candidates=$($candidates -join ';') remediation=run from a Synapse checkout or pass -BrowsersDir pointing at the deployed browsers directory"
}

function Get-SynapseBrowserRows {
    param([string]$BrowsersDir)

    $dir = Get-SynapseBrowserRowsDir -BrowsersDir $BrowsersDir
    $rows = @(Get-ChildItem -LiteralPath $dir -Filter '*.json' -File | Sort-Object Name | ForEach-Object {
        $row = Get-Content -Raw -LiteralPath $_.FullName | ConvertFrom-Json
        if ([string]$row.id -cne $_.BaseName) {
            throw "SYNAPSE_BROWSER_ROW_ID_MISMATCH file=$($_.FullName) id=$($row.id) remediation=the row id must equal its file stem"
        }
        $row
    })
    if ($rows.Count -eq 0) {
        throw "SYNAPSE_BROWSER_ROWS_EMPTY dir=$dir"
    }
    return $rows
}

function Get-SynapseBrowserRow {
    param(
        [Parameter(Mandatory = $true)][string]$Browser,
        [string]$BrowsersDir
    )

    if ($Browser -cnotmatch '^[a-z0-9_]+$') {
        throw "SYNAPSE_BROWSER_ID_INVALID browser=$Browser remediation=browser ids match [a-z0-9_]+ (the browsers\*.json file stem)"
    }
    $dir = Get-SynapseBrowserRowsDir -BrowsersDir $BrowsersDir
    $path = Join-Path $dir "$Browser.json"
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        $known = @(Get-ChildItem -LiteralPath $dir -Filter '*.json' -File | ForEach-Object { $_.BaseName }) -join ','
        throw "SYNAPSE_BROWSER_ROW_UNKNOWN browser=$Browser dir=$dir known=$known"
    }
    $row = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json
    if ([string]$row.id -cne $Browser) {
        throw "SYNAPSE_BROWSER_ROW_ID_MISMATCH file=$path id=$($row.id)"
    }
    return $row
}

function Expand-SynapseBrowserPath {
    # Expands %VAR% references; $null when a variable is unset (never a
    # half-expanded path), mirroring synapse_browsers::expand_env_vars.
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Path)

    $result = [System.Text.StringBuilder]::new()
    $rest = $Path
    while ($true) {
        $start = $rest.IndexOf('%')
        if ($start -lt 0) {
            [void]$result.Append($rest)
            break
        }
        [void]$result.Append($rest.Substring(0, $start))
        $after = $rest.Substring($start + 1)
        $end = $after.IndexOf('%')
        if ($end -le 0) { return $null }
        $value = [Environment]::GetEnvironmentVariable($after.Substring(0, $end))
        if ([string]::IsNullOrEmpty($value)) { return $null }
        [void]$result.Append($value)
        $rest = $after.Substring($end + 1)
    }
    return $result.ToString()
}

function Resolve-SynapseBrowserExecutable {
    # App Paths (HKLM, then HKCU), then the row's exe_candidates; returns
    # path + source_of_truth, or $null when the browser is not installed.
    param([Parameter(Mandatory = $true)][object]$Row)

    if (-not [string]::IsNullOrWhiteSpace([string]$Row.app_paths_name)) {
        foreach ($hive in @('HKLM', 'HKCU')) {
            $key = "${hive}:\$(Get-SynapseBrowserAppPathsKey)\$($Row.app_paths_name)"
            $value = (Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).'(default)'
            $candidate = ([string]$value).Trim().Trim('"')
            if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
                return [pscustomobject]@{ path = $candidate; source_of_truth = "registry $key (default value)" }
            }
        }
    }
    foreach ($rawCandidate in @($Row.exe_candidates)) {
        $candidate = Expand-SynapseBrowserPath -Path ([string]$rawCandidate)
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return [pscustomobject]@{ path = $candidate; source_of_truth = "browsers/$($Row.id).json exe_candidates $rawCandidate" }
        }
    }
    return $null
}

function Test-SynapseBrowserProtectedExtensionId {
    # True when the row declares the id in protected_extension_ids (browser UI
    # or wallet extensions that must never be counted as hazards or shielded).
    param(
        [Parameter(Mandatory = $true)][object]$Row,
        [AllowEmptyString()][AllowNull()][string]$ExtensionId
    )

    if ([string]::IsNullOrWhiteSpace($ExtensionId)) { return $false }
    if ($Row.PSObject.Properties.Name -notcontains 'protected_extension_ids') { return $false }
    foreach ($protectedId in @($Row.protected_extension_ids)) {
        if ([string]$protectedId -ceq $ExtensionId) { return $true }
    }
    return $false
}

function Get-SynapseBrowserProfileDirs {
    # Profile directories under a Chromium user-data root, per the row's
    # profile_layout (mirrors synapse_browsers::chromium::profile_scan::profile_dirs):
    # single = the root itself when it holds a preference file; multi = child
    # dirs except Snapshots holding a preference file, sorted by name.
    param(
        [Parameter(Mandatory = $true)][object]$Row,
        [Parameter(Mandatory = $true)][string]$UserDataRoot
    )

    $prefFiles = @('Preferences', 'Secure Preferences')
    $hasPrefs = {
        param([string]$Dir)
        foreach ($file in $prefFiles) {
            if (Test-Path -LiteralPath (Join-Path $Dir $file) -PathType Leaf) { return $true }
        }
        return $false
    }
    switch -CaseSensitive ([string]$Row.profile_layout) {
        'single' {
            if (-not (Test-Path -LiteralPath $UserDataRoot -PathType Container)) { return @() }
            if (-not (& $hasPrefs $UserDataRoot)) { return @() }
            return @(Get-Item -LiteralPath $UserDataRoot)
        }
        'multi' {
            return @(Get-ChildItem -LiteralPath $UserDataRoot -Directory -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -ne 'Snapshots' -and (& $hasPrefs $_.FullName) } |
                    Sort-Object Name)
        }
        default {
            throw "SYNAPSE_BROWSER_PROFILE_LAYOUT_UNSUPPORTED browser=$($Row.id) profile_layout=$($Row.profile_layout) supported=single,multi"
        }
    }
}

function Resolve-SynapseBrowserProfilePath {
    # Directory holding one profile's Preferences: the root itself for the
    # single layout, <root>\<ProfileName> for multi.
    param(
        [Parameter(Mandatory = $true)][object]$Row,
        [Parameter(Mandatory = $true)][string]$UserDataRoot,
        [Parameter(Mandatory = $true)][string]$ProfileName
    )

    switch -CaseSensitive ([string]$Row.profile_layout) {
        'single' { return $UserDataRoot }
        'multi' { return Join-Path $UserDataRoot $ProfileName }
        default {
            throw "SYNAPSE_BROWSER_PROFILE_LAYOUT_UNSUPPORTED browser=$($Row.id) profile_layout=$($Row.profile_layout) supported=single,multi"
        }
    }
}

function New-SynapseBridgeConfig {
    # The bridge-config.json object the shared service worker reads
    # (extensions\synapse-chrome-debugger\bridge_config.js parseBridgeConfig).
    param([Parameter(Mandatory = $true)][object]$Row)

    [ordered]@{
        schemaVersion = Get-SynapseBridgeConfigSchemaVersion
        browserKind = [string]$Row.id
        family = [string]$Row.family
        expectedExtensionId = [string]$Row.extension.id
        tabTargetPrefix = [string]$Row.tab_target_prefix
        windowTitleSuffixes = @($Row.window_title_suffixes | ForEach-Object { [string]$_ })
        protectedExtensionIds = @($Row.protected_extension_ids | ForEach-Object { [string]$_ })
        policyRoot = if ($null -eq $Row.policy) { $null } else { [string]$Row.policy.root }
    }
}

function Get-SynapseBrowserFamilyInstaller {
    # Family -> installer script; packages add their family script (gecko).
    param([Parameter(Mandatory = $true)][object]$Row)

    switch ([string]$Row.family) {
        'chromium' { return Join-Path $PSScriptRoot 'install-synapse-chrome-debugger.ps1' }
        'gecko' { return Join-Path $PSScriptRoot 'install-synapse-gecko-bridge.ps1' }
        default { throw "SYNAPSE_BROWSER_FAMILY_UNKNOWN browser=$($Row.id) family=$($Row.family)" }
    }
}
