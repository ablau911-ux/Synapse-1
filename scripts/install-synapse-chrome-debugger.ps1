param(
    [string]$SynapseNativeHostExe = "$env:USERPROFILE\.cargo\bin\synapse-chrome-native-host.exe",
    # Chromium-family browser row (crates\synapse-browsers\browsers\<id>.json).
    # Every browser-specific name, path and registry key below comes from it.
    [string]$Browser = 'chrome',
    [string]$BrowsersDir,
    # Defaults to the row's extension.id.
    [string]$ExtensionId = '',
    # Maintenance/self-heal entry point: run ONLY the one-way removal of any
    # debugger/nativeMessaging blockers Synapse wrote into the Chrome
    # ExtensionSettings policy, print the result, and exit.
    [switch]$RemoveExternalDebuggerPolicyOnly,
    # Deprecated no-op kept so existing callers still bind: other extensions
    # are always preserved; setup never writes blockers for them.
    [switch]$PreserveExternalDebuggerExtensions,
    # Default behavior auto-loads the bundled unpacked extension into the
    # already-open active Chrome profile when the profile row is absent.
    [switch]$SkipAutoInstall,
    # Maintenance/self-heal entry point: when the expected unpacked extension row
    # is already installed and ready but no daemon bridge host is registered, use
    # the already-open Chrome extensions page UI to invoke its Reload button.
    [switch]$ReloadExistingExtensionViaUi,
    [ValidateRange(5, 300)]
    [int]$AutoInstallTimeoutSeconds = 90,
    # Setup entry point (#251): the caller verifies the live bridge identity
    # itself once its daemon is running, so this run reports the check as
    # deferred (ok=false, runtime_verification.deferred=true) together with the
    # expected identity instead of reading a daemon that is stopped or about to
    # be replaced.
    [switch]$DeferLiveVerification
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'synapse-browser-rows.ps1')
. (Join-Path $PSScriptRoot 'synapse-bridge-live-identity.ps1')
$script:BrowserRow = Get-SynapseBrowserRow -Browser $Browser -BrowsersDir $BrowsersDir
if ([string]$script:BrowserRow.family -ne 'chromium') {
    throw "SYNAPSE_BROWSER_BRIDGE_FAMILY_MISMATCH browser=$Browser family=$($script:BrowserRow.family) installer=chromium remediation=use scripts\install-synapse-browser-bridge.ps1 -Browser $Browser"
}
if ([string]::IsNullOrWhiteSpace($ExtensionId)) {
    $ExtensionId = [string]$script:BrowserRow.extension.id
}
$script:BrowserProcessNames = @($script:BrowserRow.process_names | ForEach-Object { [string]$_ })
$script:BrowserProcessWmiFilter = (@($script:BrowserProcessNames | ForEach-Object { "Name='$_'" }) -join ' OR ')
$script:BrowserExtensionsPageUrl = [string]$script:BrowserRow.extension.extensions_page_url
$script:BrowserUserDataRoot = Expand-SynapseBrowserPath -Path ([string]$script:BrowserRow.user_data_root)
if ([string]::IsNullOrWhiteSpace($script:BrowserUserDataRoot)) {
    throw "SYNAPSE_BROWSER_USER_DATA_ROOT_UNRESOLVED browser=$Browser user_data_root=$($script:BrowserRow.user_data_root)"
}
$script:BrowserInstallUiDeveloperMode = @($script:BrowserRow.install_ui.developer_mode_names)[0]
$script:BrowserInstallUiLoadUnpacked = @($script:BrowserRow.install_ui.load_unpacked_names)[0]
# `^Extensions( - <suffix>)?$` built from the row's window title suffixes.
$script:BrowserExtensionsWindowTitlePattern = '^Extensions(' + ((@($script:BrowserRow.window_title_suffixes) | ForEach-Object { [regex]::Escape([string]$_) }) -join '|') + ')?$'
if ($SkipAutoInstall -and $ReloadExistingExtensionViaUi) {
    throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_RELOAD_CONFLICT skip_auto_install=true reload_existing_extension_via_ui=true remediation=choose either the read-only skip path or the existing-extension UI reload path; setup must not silently ignore one of these mutually exclusive modes"
}
# `Get-Acl` is diagnostic-only below. Some hidden Windows PowerShell launch
# environments have Security type data loaded while the cmdlet is not callable;
# importing then fails with duplicate type-data errors. Let the diagnostic reader
# report an acl_error instead of aborting bridge installation.

function ConvertTo-CompressedJson {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Value,
        [int]$Depth = 12
    )
    ConvertTo-Json -InputObject $Value -Depth $Depth -Compress
}

function Get-SynapseFileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "SYNAPSE_CHROME_FILE_HASH_MISSING path=$Path remediation=write the extension artifact before hashing it"
    }
    try {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        try {
            $share = [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
            $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, $share)
            try {
                $hash = $sha.ComputeHash($stream)
            } finally {
                $stream.Dispose()
            }
        } finally {
            $sha.Dispose()
        }
        return (($hash | ForEach-Object { $_.ToString('X2') }) -join '')
    } catch {
        throw "SYNAPSE_CHROME_FILE_HASH_FAILED path=$Path error=$($_.Exception.Message) remediation=verify the extension artifact is readable and retry the Chrome bridge install"
    }
}

function Get-RegistryAclDiagnostic {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $cursor = $Path
    while (-not [string]::IsNullOrWhiteSpace($cursor) -and -not (Test-Path -LiteralPath $cursor)) {
        if ($cursor -notmatch '^(.*)\\[^\\]+$') {
            break
        }
        $cursor = $Matches[1]
    }
    if ([string]::IsNullOrWhiteSpace($cursor)) {
        $cursor = $Path
    }
    try {
        $acl = Get-Acl -LiteralPath $cursor -ErrorAction Stop
        $access = @($acl.Access | ForEach-Object {
            [pscustomobject]@{
                identity = [string]$_.IdentityReference
                rights = [string]$_.RegistryRights
                type = [string]$_.AccessControlType
                inherited = [bool]$_.IsInherited
            }
        })
        return ConvertTo-CompressedJson -Value ([ordered]@{
            requested_path = $Path
            inspected_path = $cursor
            owner = [string]$acl.Owner
            access = $access
        }) -Depth 8
    } catch {
        return "requested_path=$Path inspected_path=$cursor acl_error=$($_.Exception.Message)"
    }
}

function Get-ChromePolicyRoot {
    # The row's ExtensionSettings policy root; $null when the browser row
    # declares no policy (popup shield then relies on management suppression).
    param(
        [ValidateSet('HKCU', 'HKLM')]
        [string]$Hive
    )
    if ($null -eq $script:BrowserRow.policy) {
        return $null
    }
    return "${Hive}:\$($script:BrowserRow.policy.root)"
}

function Get-ChromePolicyHiveCandidates {
    param(
        [ValidateSet('Auto', 'HKCU', 'HKLM')]
        [string]$Hive
    )

    if ($Hive -eq 'Auto') {
        return @('HKCU', 'HKLM')
    }
    return @($Hive)
}

function Read-ChromeExtensionSettingsPolicy {
    param(
        [ValidateSet('HKCU', 'HKLM')]
        [string]$Hive
    )

    $policyRoot = Get-ChromePolicyRoot -Hive $Hive
    if (-not (Test-Path -LiteralPath $policyRoot)) {
        return [ordered]@{}
    }
    $raw = (Get-ItemProperty -LiteralPath $policyRoot -Name ExtensionSettings -ErrorAction SilentlyContinue).ExtensionSettings
    if ([string]::IsNullOrWhiteSpace([string]$raw)) {
        return [ordered]@{}
    }
    try {
        $parsed = $raw | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "SYNAPSE_CHROME_EXTENSION_SETTINGS_POLICY_INVALID hive=$Hive path=$policyRoot value_name=ExtensionSettings parse_error=$($_.Exception.Message) remediation=fix the existing Chrome ExtensionSettings policy JSON before Synapse can merge debugger/nativeMessaging blockers without overwriting policy state"
    }

    $settings = [ordered]@{}
    foreach ($property in $parsed.PSObject.Properties) {
        $entry = [ordered]@{}
        foreach ($entryProperty in $property.Value.PSObject.Properties) {
            if ($entryProperty.Value -is [System.Array]) {
                $entry[$entryProperty.Name] = @($entryProperty.Value)
            } else {
                $entry[$entryProperty.Name] = $entryProperty.Value
            }
        }
        $settings[$property.Name] = $entry
    }
    return $settings
}

# Exact blocked_install_message a previous Synapse version stamped onto every
# ExtensionSettings entry it authored. It is the ONLY reliable marker that lets
# the self-heal below distinguish Synapse-authored blockers from policy an
# enterprise admin or the user set themselves. Do not change this string: it
# must match byte-for-byte what shipped installers wrote, or old installs will
# not be healed.
$script:SynapseChromeBlockedInstallMessage = 'Synapse blocked this extension on this host because debugger/nativeMessaging permissions can surface Chrome debugger or native-host popups during background automation.'

function Remove-SynapseChromeExternalDebuggerPolicy {
    param(
        [string[]]$PreserveExtensionIds = @()
    )

    # Reversible self-heal. This only undoes debugger/nativeMessaging blockers
    # that Synapse wrote into Chrome ExtensionSettings, identified strictly by
    # the Synapse blocked_install_message marker. Entries Synapse did not author
    # are left byte-for-byte untouched. Best-effort per hive: a hive we cannot
    # write is reported with ACL evidence, never silently swallowed, and never
    # fatal.
    $preserveIds = @($PreserveExtensionIds | Where-Object {
        $_ -match '^[a-p]{32}$'
    } | Sort-Object -Unique)
    $results = @()
    foreach ($hive in (Get-ChromePolicyHiveCandidates -Hive 'Auto')) {
        $policyRoot = Get-ChromePolicyRoot -Hive $hive
        if ($null -eq $policyRoot) {
            $results += [pscustomobject]@{ hive = $hive; path = $null; changed = $false; reason = 'row_declares_no_policy_root' }
            continue
        }
        if (-not (Test-Path -LiteralPath $policyRoot)) {
            $results += [pscustomobject]@{ hive = $hive; path = $policyRoot; changed = $false; reason = 'policy_root_absent' }
            continue
        }
        $raw = (Get-ItemProperty -LiteralPath $policyRoot -Name ExtensionSettings -ErrorAction SilentlyContinue).ExtensionSettings
        if ([string]::IsNullOrWhiteSpace([string]$raw)) {
            $results += [pscustomobject]@{ hive = $hive; path = $policyRoot; changed = $false; reason = 'no_extension_settings' }
            continue
        }
        try {
            $settings = Read-ChromeExtensionSettingsPolicy -Hive $hive
        } catch {
            # A policy we cannot parse is NOT ours to rewrite; surface it loudly
            # and leave it intact rather than risk corrupting admin policy.
            $results += [pscustomobject]@{ hive = $hive; path = $policyRoot; changed = $false; reason = "parse_error:$($_.Exception.Message)" }
            continue
        }
        $changed = $false
        $cleaned = [ordered]@{}
        $removedEntries = @()
        $strippedEntries = @()
        $preservedEntries = @()
        foreach ($name in @($settings.Keys)) {
            $entry = $settings[$name]
            $isSynapseAuthored = ($entry -is [System.Collections.Specialized.OrderedDictionary]) -and
                $entry.Contains('blocked_install_message') -and
                ([string]$entry['blocked_install_message'] -eq $script:SynapseChromeBlockedInstallMessage)
            if (-not $isSynapseAuthored) {
                $cleaned[$name] = $entry
                continue
            }
            if ($preserveIds -contains $name) {
                $cleaned[$name] = $entry
                $preservedEntries += $name
                continue
            }
            $changed = $true
            $blocked = @()
            if ($entry.Contains('blocked_permissions')) {
                $blocked = @($entry['blocked_permissions'] | Where-Object { $_ -ne 'debugger' -and $_ -ne 'nativeMessaging' })
            }
            $entry.Remove('blocked_install_message')
            if ($blocked.Count -gt 0) {
                $entry['blocked_permissions'] = $blocked
            } elseif ($entry.Contains('blocked_permissions')) {
                $entry.Remove('blocked_permissions')
            }
            # Drop the entry entirely only if Synapse's blockers were all it held;
            # otherwise preserve whatever else (e.g. an admin installation_mode).
            if ($entry.Keys.Count -gt 0) {
                $cleaned[$name] = $entry
                $strippedEntries += $name
            } else {
                $removedEntries += $name
            }
        }
        if (-not $changed) {
            $reason = if ($preservedEntries.Count -gt 0) {
                'only_current_synapse_popup_shields_present'
            } else {
                'no_synapse_authored_blocks'
            }
            $results += [pscustomobject]@{
                hive = $hive
                path = $policyRoot
                changed = $false
                reason = $reason
                preserved_entries = @($preservedEntries)
            }
            continue
        }
        try {
            if ($cleaned.Keys.Count -gt 0) {
                $json = ConvertTo-CompressedJson -Value $cleaned
                New-ItemProperty -LiteralPath $policyRoot -Name ExtensionSettings -PropertyType String -Value $json -Force | Out-Null
            } else {
                Remove-ItemProperty -LiteralPath $policyRoot -Name ExtensionSettings -ErrorAction Stop
            }
            $results += [pscustomobject]@{
                hive = $hive
                path = $policyRoot
                changed = $true
                reason = 'removed_synapse_authored_blocks'
                removed_entries = @($removedEntries)
                stripped_entries = @($strippedEntries)
                preserved_entries = @($preservedEntries)
                extension_settings_remaining = ($cleaned.Keys.Count -gt 0)
            }
        } catch {
            $results += [pscustomobject]@{
                hive = $hive
                path = $policyRoot
                changed = $false
                reason = "write_failed:$($_.Exception.Message)"
                acl_detail = (Get-RegistryAclDiagnostic -Path $policyRoot)
            }
        }
    }
    return @($results)
}

function Get-SynapsePolicyShieldGuardedState {
    # Canonical snapshot of the ExtensionSettings fields that can disable the
    # bridge's chrome.debugger (Edge 154+: runtime_blocked_hosts on an extension
    # holding the debugger permission; any build: a debugger blocked_permissions)
    # for the two guarded entries: the bridge's own id and the "*" default.
    param(
        [Parameter(Mandatory = $true)]
        [System.Collections.IDictionary]$Settings,
        [Parameter(Mandatory = $true)]
        [string]$BridgeExtensionId
    )

    $parts = foreach ($guardedId in @('*', $BridgeExtensionId)) {
        $entry = if ($Settings.Contains($guardedId)) { $Settings[$guardedId] } else { $null }
        $hosts = '<absent>'
        $blocksDebugger = $false
        if ($entry -is [System.Collections.IDictionary]) {
            if ($entry.Contains('runtime_blocked_hosts')) {
                $hosts = '[' + (@($entry['runtime_blocked_hosts'] | ForEach-Object { [string]$_ }) -join ',') + ']'
            }
            if ($entry.Contains('blocked_permissions')) {
                $blocksDebugger = @($entry['blocked_permissions']) -contains 'debugger'
            }
        }
        "id=$guardedId runtime_blocked_hosts=$hosts blocks_debugger=$blocksDebugger"
    }
    return (@($parts) -join ';')
}

function Set-SynapseChromeExternalDebuggerPolicy {
    param(
        [object[]]$Extensions,
        # The bridge's own id: shielded for nativeMessaging only, never debugger.
        [Parameter(Mandatory = $true)]
        [string]$BridgeExtensionId,
        # The row's protected_extension_ids: never hazards, never shielded.
        [string[]]$ProtectedExtensionIds = @()
    )

    # '^[a-p]{32}$' also keeps the "*" default entry out of the shield.
    $hazards = @($Extensions | Where-Object {
        $id = [string]$_.extension_id
        ($id -match '^[a-p]{32}$') -and (@($ProtectedExtensionIds) -notcontains $id)
    } | Sort-Object extension_id -Unique)

    $policyRoot = Get-ChromePolicyRoot -Hive 'HKCU'
    if ($null -eq $policyRoot) {
        return [pscustomobject]@{
            hive = 'HKCU'
            path = $null
            changed = $false
            reason = 'row_declares_no_policy_root'
            shielded_entries = @()
        }
    }
    if ($hazards.Count -eq 0) {
        return [pscustomobject]@{
            hive = 'HKCU'
            path = $policyRoot
            changed = $false
            reason = 'no_debugger_or_native_hazards'
            shielded_entries = @()
            unchanged_entries = @()
        }
    }

    try {
        if (-not (Test-Path -LiteralPath $policyRoot)) {
            New-Item -Path $policyRoot -Force | Out-Null
        }
        $settings = Read-ChromeExtensionSettingsPolicy -Hive 'HKCU'
    } catch {
        $acl = Get-RegistryAclDiagnostic -Path $policyRoot
        return [pscustomobject]@{
            hive = 'HKCU'
            path = $policyRoot
            changed = $false
            reason = 'external_popup_shield_write_denied_requires_bridge_management'
            warning_code = 'SYNAPSE_CHROME_POLICY_POPUP_SHIELD_WRITE_DENIED'
            blocking = $true
            phase = 'read_or_create'
            error = $_.Exception.Message
            acl = $acl
            remediation = 'do not weaken the admin-only HKCU\Software\Policies\Google\Chrome ACL for non-elevated setup; run scripts\synapse-setup.ps1 from an elevated PowerShell only when the optional Chrome ExtensionSettings popup shield must be applied; otherwise the installed Synapse Chrome Bridge must suppress the hazard through chrome.management or normal bridge commands fail closed before touching Chrome'
            shielded_entries = @()
            unchanged_entries = @()
        }
    }
    $changed = $false
    $shieldedEntries = @()
    $unchangedEntries = @()
    $guardedBefore = Get-SynapsePolicyShieldGuardedState -Settings $settings -BridgeExtensionId $BridgeExtensionId

    foreach ($hazard in $hazards) {
        $id = [string]$hazard.extension_id
        $existing = $null
        if ($settings.Contains($id)) {
            $existing = $settings[$id]
        }
        if (-not ($existing -is [System.Collections.Specialized.OrderedDictionary])) {
            $existing = [ordered]@{}
        }

        $blocked = @()
        if ($existing.Contains('blocked_permissions')) {
            $blocked = @($existing['blocked_permissions'])
        }
        $hazardPermissions = @($hazard.hazard_api | Where-Object {
            $_ -eq 'debugger' -or $_ -eq 'nativeMessaging'
        })
        if ($hazardPermissions.Count -eq 0) {
            $hazardPermissions = @('debugger', 'nativeMessaging')
        }
        if ($id -ceq $BridgeExtensionId) {
            # Blocking debugger on the bridge itself would disable its
            # target-scoped chrome.debugger CDP input lane.
            $hazardPermissions = @($hazardPermissions | Where-Object { $_ -ne 'debugger' })
            if ($hazardPermissions.Count -eq 0) {
                $unchangedEntries += $id
                continue
            }
        }
        $nextBlocked = @($blocked + $hazardPermissions | Where-Object {
            -not [string]::IsNullOrWhiteSpace([string]$_)
        } | Sort-Object -Unique)

        $beforeBlocked = @($blocked | Sort-Object -Unique)
        $beforeMessage = $null
        if ($existing.Contains('blocked_install_message')) {
            $beforeMessage = [string]$existing['blocked_install_message']
        }

        $existing['blocked_permissions'] = $nextBlocked
        $existing['blocked_install_message'] = $script:SynapseChromeBlockedInstallMessage
        $settings[$id] = $existing

        if ((($beforeBlocked -join ',') -ne ($nextBlocked -join ',')) -or
            ($beforeMessage -ne $script:SynapseChromeBlockedInstallMessage)) {
            $changed = $true
            $shieldedEntries += [pscustomobject]@{
                extension_id = $id
                name = [string]$hazard.name
                active_api = @($hazard.active_api)
                granted_api = @($hazard.granted_api)
                manifest_api = @($hazard.manifest_api)
                hazard_api = @($hazard.hazard_api)
                runtime_enabled = [bool]$hazard.runtime_enabled
                source = [string]$hazard.source
            }
        } else {
            $unchangedEntries += $id
        }
    }

    if ($changed) {
        $guardedAfter = Get-SynapsePolicyShieldGuardedState -Settings $settings -BridgeExtensionId $BridgeExtensionId
        if ($guardedAfter -cne $guardedBefore) {
            throw "SYNAPSE_CHROME_POLICY_SHIELD_GUARDED_ENTRY_CHANGED before=$guardedBefore after=$guardedAfter remediation=the popup shield must never add runtime_blocked_hosts or a debugger block to the bridge's own ExtensionSettings entry or the * default (either disables chrome.debugger.attach for the bridge); nothing was written"
        }
        $json = ConvertTo-CompressedJson -Value $settings
        try {
            New-ItemProperty -LiteralPath $policyRoot -Name ExtensionSettings -PropertyType String -Value $json -Force | Out-Null
        } catch {
            $acl = Get-RegistryAclDiagnostic -Path $policyRoot
            return [pscustomobject]@{
                hive = 'HKCU'
                path = $policyRoot
                changed = $false
                reason = 'external_popup_shield_write_denied_requires_bridge_management'
                warning_code = 'SYNAPSE_CHROME_POLICY_POPUP_SHIELD_WRITE_DENIED'
                blocking = $true
                phase = 'write_extension_settings'
                error = $_.Exception.Message
                acl = $acl
                remediation = 'do not weaken the admin-only HKCU\Software\Policies\Google\Chrome ACL for non-elevated setup; run scripts\synapse-setup.ps1 from an elevated PowerShell only when the optional Chrome ExtensionSettings popup shield must be applied; otherwise the installed Synapse Chrome Bridge must suppress the hazard through chrome.management or normal bridge commands fail closed before touching Chrome'
                shielded_entries = @($shieldedEntries)
                unchanged_entries = @($unchangedEntries)
            }
        }
    }

    [pscustomobject]@{
        hive = 'HKCU'
        path = $policyRoot
        changed = $changed
        reason = if ($changed) { 'synapse_authored_popup_shield_applied' } else { 'synapse_authored_popup_shield_already_present' }
        shielded_entries = @($shieldedEntries)
        unchanged_entries = @($unchangedEntries)
    }
}

if ($RemoveExternalDebuggerPolicyOnly) {
    $cleanup = Remove-SynapseChromeExternalDebuggerPolicy -PreserveExtensionIds @()
    [pscustomobject]@{
        ok = $true
        mode = 'chrome_policy_cleanup_only'
        chrome_policy_cleanup = $cleanup
    }
    exit 0
}

function Get-SynapseNativeHostRegistryTargets {
    param(
        [Parameter(Mandatory = $true)]
        [string]$HostName
    )

    # Stale-key cleanup only (the bridge never uses native messaging); the
    # roots come from the row's native_messaging_cleanup_roots.
    foreach ($root in @($script:BrowserRow.native_messaging_cleanup_roots)) {
        foreach ($hive in @('HKCU', 'HKLM')) {
            [pscustomobject]@{
                hive = $hive
                registry_view = if ([string]$root -match '\\Wow6432Node\\') { '32' } else { '64' }
                path = "${hive}:\$root\$HostName"
            }
        }
    }
}

function Read-SynapseNativeHostRegistryEntries {
    param(
        [Parameter(Mandatory = $true)]
        [string]$HostName
    )

    $entries = @()
    foreach ($target in (Get-SynapseNativeHostRegistryTargets -HostName $HostName)) {
        if (-not (Test-Path -LiteralPath $target.path)) {
            continue
        }
        try {
            $key = Get-Item -LiteralPath $target.path -ErrorAction Stop
            $manifestPath = [string]$key.GetValue('')
            $entries += [pscustomobject]@{
                hive = $target.hive
                registry_view = $target.registry_view
                path = $target.path
                manifest_path = $manifestPath
                read_error = $null
            }
        } catch {
            $entries += [pscustomobject]@{
                hive = $target.hive
                registry_view = $target.registry_view
                path = $target.path
                manifest_path = $null
                read_error = $_.Exception.Message
            }
        }
    }
    return @($entries)
}

function Remove-SynapseNativeHostRegistryEntries {
    param(
        [Parameter(Mandatory = $true)]
        [string]$HostName
    )

    $before = @(Read-SynapseNativeHostRegistryEntries -HostName $HostName)
    $removed = @()
    $failures = @()

    foreach ($entry in $before) {
        try {
            Remove-Item -LiteralPath $entry.path -Force -ErrorAction Stop
            $removed += [pscustomobject]@{
                hive = $entry.hive
                registry_view = $entry.registry_view
                path = $entry.path
                manifest_path = $entry.manifest_path
            }
        } catch {
            $failures += [pscustomobject]@{
                hive = $entry.hive
                registry_view = $entry.registry_view
                path = $entry.path
                manifest_path = $entry.manifest_path
                error = $_.Exception.Message
                acl_detail = Get-RegistryAclDiagnostic -Path $entry.path
            }
        }
    }

    $after = @(Read-SynapseNativeHostRegistryEntries -HostName $HostName)
    if ($failures.Count -gt 0 -or $after.Count -gt 0) {
        $detail = ConvertTo-CompressedJson -Value ([ordered]@{
            host_name = $HostName
            before = $before
            removed = $removed
            after = $after
            failures = $failures
        }) -Depth 10
        throw "SYNAPSE_CHROME_NATIVE_HOST_REGISTRY_REMOVE_FAILED_ALL_HIVES detail=$detail remediation=normal bridge must not leave Synapse nativeMessaging host registration in any Chrome lookup hive because Chrome can launch visible native-host wrappers; remove the listed registry keys from a principal that can write them and rerun this verifier"
    }

    [pscustomobject]@{
        host_name = $HostName
        before = $before
        removed = $removed
        after = $after
    }
}

if ($ExtensionId -notmatch '^[a-p]{32}$') {
    throw "SYNAPSE_CHROME_EXTENSION_ID_INVALID extension_id=$ExtensionId remediation=Chrome extension IDs are 32 lowercase characters in the range a-p; refusing to inspect profiles with an ambiguous extension identity"
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$extensionSourceDir = Join-Path $repoRoot 'extensions\synapse-chrome-debugger'
$sourceManifestPath = Join-Path $extensionSourceDir 'manifest.json'
if (-not (Test-Path -LiteralPath $sourceManifestPath -PathType Leaf)) {
    throw "SYNAPSE_CHROME_EXTENSION_MANIFEST_MISSING path=$sourceManifestPath"
}
$sourceServiceWorkerPath = Join-Path $extensionSourceDir 'service_worker.js'
if (-not (Test-Path -LiteralPath $sourceServiceWorkerPath -PathType Leaf)) {
    throw "SYNAPSE_CHROME_EXTENSION_SERVICE_WORKER_MISSING path=$sourceServiceWorkerPath"
}
$serviceWorkerSource = Get-Content -Raw -LiteralPath $sourceServiceWorkerPath
if ($serviceWorkerSource -notmatch 'const\s+BRIDGE_BUILD_ID\s*=\s*"([^"]+)";') {
    throw "SYNAPSE_CHROME_EXTENSION_BUILD_ID_MISSING path=$sourceServiceWorkerPath remediation=service_worker.js must expose BRIDGE_BUILD_ID so setup can deploy the unpacked extension to a stable build-id directory"
}
$bridgeBuildId = [string]$Matches[1]
if ($bridgeBuildId -notmatch '^[A-Za-z0-9._-]+$') {
    throw "SYNAPSE_CHROME_EXTENSION_BUILD_ID_INVALID build_id=$bridgeBuildId remediation=BRIDGE_BUILD_ID must be safe for a Windows directory name"
}
if ($serviceWorkerSource -notmatch 'const\s+BRIDGE_DECLARED_BUILD_SHA256\s*=\s*"([^"]+)";') {
    throw "SYNAPSE_CHROME_EXTENSION_BUILD_SHA_MISSING path=$sourceServiceWorkerPath remediation=service_worker.js must expose BRIDGE_DECLARED_BUILD_SHA256 for declared build metadata; physical integrity comes from service_worker_sha256"
}
$bridgeDeclaredBuildSha256 = [string]$Matches[1]
if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    throw "SYNAPSE_CHROME_EXTENSION_STABLE_ROOT_UNAVAILABLE remediation=LOCALAPPDATA is required to deploy the unpacked Chrome bridge to a checkout-independent stable directory"
}
$stableExtensionRoot = Join-Path $env:LOCALAPPDATA ('synapse\' + [string]$script:BrowserRow.extension.deploy_subdir)
$extensionDir = Join-Path $stableExtensionRoot $bridgeBuildId
$stableRootFull = [System.IO.Path]::GetFullPath($stableExtensionRoot)
$extensionDirFull = [System.IO.Path]::GetFullPath($extensionDir)
if (-not $extensionDirFull.StartsWith($stableRootFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "SYNAPSE_CHROME_EXTENSION_STABLE_PATH_INVALID root=$stableRootFull path=$extensionDirFull"
}
function Install-SynapseChromeBridgeFiles {
    param([string]$SourceDir, [string]$Destination, [object]$BrowserRow, [scriptblock]$ProvisionAuth)
    $sourceServiceWorkerPath = Join-Path $SourceDir 'service_worker.js'
    $manifestPath = Join-Path $SourceDir 'manifest.json'
    $extensionManifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    Assert-SynapseChromeBridgeManifest -Manifest $extensionManifest -Path $manifestPath
    $codeStampPlaceholder = 'const BRIDGE_CODE_STAMP = "unstamped";'
    $sourceWorker = [IO.File]::ReadAllText($sourceServiceWorkerPath)
    $placeholderCount = ([regex]::Matches($sourceWorker, [regex]::Escape($codeStampPlaceholder))).Count
    if ($placeholderCount -ne 1) { throw "SYNAPSE_CHROME_EXTENSION_CODE_STAMP_PLACEHOLDER_INVALID path=$sourceServiceWorkerPath count=$placeholderCount" }
    $extensionDirFull = [IO.Path]::GetFullPath($Destination)
    # Hash only source runtime files, so stale destination files cannot affect identity.
    $bridgeCodeStamp = Get-SynapseChromeRuntimeStamp -SourceDir $SourceDir
    New-Item -ItemType Directory -Force -Path $extensionDirFull | Out-Null
    Get-ChildItem -LiteralPath $SourceDir -Force | Copy-Item -Destination $extensionDirFull -Recurse -Force
    $stamped = $sourceWorker.Replace($codeStampPlaceholder, "const BRIDGE_CODE_STAMP = `"$bridgeCodeStamp`";")
    [IO.File]::WriteAllText((Join-Path $extensionDirFull 'service_worker.js'), $stamped, [Text.UTF8Encoding]::new($false))
    $bridgeAuthProvisioning = & $ProvisionAuth $extensionDirFull $BrowserRow
    $bridgeConfigPath = Join-Path $extensionDirFull 'bridge-config.json'
    $config = New-SynapseBridgeConfig -Row $BrowserRow
    [IO.File]::WriteAllText($bridgeConfigPath, (ConvertTo-Json -Depth 4 -InputObject $config), [Text.UTF8Encoding]::new($false))
    $readback = Get-Content -Raw -LiteralPath $bridgeConfigPath | ConvertFrom-Json
    if ([string]$readback.browserKind -cne [string]$BrowserRow.id -or
        [string]$readback.tabTargetPrefix -cne [string]$BrowserRow.tab_target_prefix -or
        [string]$readback.expectedExtensionId -cne [string]$BrowserRow.extension.id) {
        throw "SYNAPSE_BROWSER_BRIDGE_CONFIG_READBACK_FAILED path=$bridgeConfigPath browser=$($BrowserRow.id)"
    }
    [pscustomobject]@{ source_dir=$SourceDir; deployed_dir=$extensionDirFull
        manifest_sha256=Get-SynapseFileSha256 -Path (Join-Path $extensionDirFull 'manifest.json')
        service_worker_sha256=Get-SynapseFileSha256 -Path (Join-Path $extensionDirFull 'service_worker.js')
        code_stamp=$bridgeCodeStamp; registration_auth=$bridgeAuthProvisioning }
}

function Get-SynapseChromeRuntimeStamp {
param([string]$SourceDir)
$sourceServiceWorkerPath = Join-Path $SourceDir 'service_worker.js'
# #237: stamp the executable JavaScript closure, including imported adapters.
# The running worker reports this stamp, so the daemon can detect (and reload)
# a Chrome-cached older worker that the on-disk hash check cannot see.
$codeStampPlaceholder = 'const BRIDGE_CODE_STAMP = "unstamped";'
$deployedServiceWorkerPath = Join-Path $SourceDir 'service_worker.js'
$deployedServiceWorkerText = [System.IO.File]::ReadAllText($deployedServiceWorkerPath)
$placeholderCount = ([regex]::Matches($deployedServiceWorkerText, [regex]::Escape($codeStampPlaceholder))).Count
if ($placeholderCount -ne 1) {
    throw "SYNAPSE_CHROME_EXTENSION_CODE_STAMP_PLACEHOLDER_INVALID path=$sourceServiceWorkerPath count=$placeholderCount remediation=service_worker.js must declare exactly one $codeStampPlaceholder"
}
# Relative paths + per-file hashes make identity independent of checkout root.
# Generated bridge-config.json is data with separate identity/readback validation.
$runtimePaths = [string[]]@(Get-ChildItem -LiteralPath $SourceDir -Recurse -File -Filter '*.js' | ForEach-Object {
    $_.FullName.Substring($SourceDir.TrimEnd('\', '/').Length + 1).Replace('\', '/')
})
[Array]::Sort($runtimePaths, [StringComparer]::Ordinal)
$runtimeRows = foreach ($relative in $runtimePaths) {
    $digest = (Get-SynapseFileSha256 -Path (Join-Path $SourceDir $relative)).ToLowerInvariant()
    "$relative`0$digest`n"
}
$runtimeSha = [System.Security.Cryptography.SHA256]::Create()
try {
    $runtimeBytes = [System.Text.Encoding]::UTF8.GetBytes(($runtimeRows -join ''))
    $bridgeCodeStamp = ([BitConverter]::ToString($runtimeSha.ComputeHash($runtimeBytes))).Replace('-', '').ToLowerInvariant()
} finally {
    $runtimeSha.Dispose()
}
return $bridgeCodeStamp
}

function Assert-SynapseChromeBridgeManifest {
param([object]$Manifest, [string]$Path)
$extensionManifest = $Manifest
$manifestPath = $Path
$requiredPermissions = @($extensionManifest.permissions)
$optionalPermissions = @($extensionManifest.optional_permissions)
$hostPermissions = @($extensionManifest.host_permissions)
if (@($extensionManifest.web_accessible_resources).Where({ $null -ne $_ }).Count -gt 0) {
    throw 'SYNAPSE_CHROME_BRIDGE_AUTH_WEB_ACCESSIBLE the private bridge must not declare web_accessible_resources; refusing to replace installed files'
}
if ($optionalPermissions -contains 'debugger') {
    throw "SYNAPSE_CHROME_EXTENSION_OPTIONAL_DEBUGGER_PERMISSION_FORBIDDEN path=$manifestPath remediation=the Synapse bridge must request debugger as a required permission so target-scoped CDP input support is deterministic after auto-install"
}
if (-not ($requiredPermissions -contains 'debugger')) {
    throw "SYNAPSE_CHROME_EXTENSION_DEBUGGER_PERMISSION_REQUIRED path=$manifestPath remediation=normal-profile hover/tap/active-tab drag and viewport emulation FSV require the bundled bridge to expose narrow chrome.debugger lanes for already-open Chrome tabs; inactive-tab drag uses the bundled chrome.scripting synthetic mouse path"
}
if ($requiredPermissions -contains 'nativeMessaging') {
    throw "SYNAPSE_CHROME_EXTENSION_NATIVE_MESSAGING_FORBIDDEN path=$manifestPath remediation=normal end-user bridge must use direct localhost HTTP registration plus WebSocket command delivery; nativeMessaging can launch a visible cmd.exe wrapper on Windows"
}
if ($optionalPermissions -contains 'nativeMessaging') {
    throw "SYNAPSE_CHROME_EXTENSION_OPTIONAL_NATIVE_MESSAGING_FORBIDDEN path=$manifestPath remediation=normal end-user bridge must not request nativeMessaging"
}
if (-not ($requiredPermissions -contains 'management')) {
    throw "SYNAPSE_CHROME_EXTENSION_MANAGEMENT_PERMISSION_REQUIRED path=$manifestPath remediation=normal end-user bridge must request chrome.management so it can disable external debugger/nativeMessaging extensions or fail closed with exact readback before any browser command"
}
if (-not ($requiredPermissions -contains 'alarms')) {
    throw "SYNAPSE_CHROME_EXTENSION_ALARMS_PERMISSION_MISSING path=$manifestPath remediation=normal end-user bridge requires chrome.alarms so an MV3 service worker suspended after daemon restart can wake and re-register without foreground Chrome automation"
}
if (-not ($requiredPermissions -contains 'webNavigation')) {
    throw "SYNAPSE_CHROME_EXTENSION_WEBNAVIGATION_PERMISSION_MISSING path=$manifestPath remediation=normal bridge requires chrome.webNavigation for target-scoped lifecycle and SPA route event readback without debugger attach"
}
if (-not ($requiredPermissions -contains 'webRequest')) {
    throw "SYNAPSE_CHROME_EXTENSION_WEBREQUEST_PERMISSION_MISSING path=$manifestPath remediation=normal bridge requires chrome.webRequest for target-scoped request/response wait event buffering without debugger attach"
}
if (-not ($requiredPermissions -contains 'downloads')) {
    throw "SYNAPSE_CHROME_EXTENSION_DOWNLOADS_PERMISSION_MISSING path=$manifestPath remediation=normal bridge requires chrome.downloads for real download event/readback capture and browser_downloads save/move verification"
}
if ($optionalPermissions -contains 'alarms') {
    throw "SYNAPSE_CHROME_EXTENSION_OPTIONAL_ALARMS_PERMISSION_FORBIDDEN path=$manifestPath remediation=alarms must be a required permission for deterministic bridge wake/readback, not an optional runtime prompt"
}
if ($hostPermissions -notcontains 'http://127.0.0.1:7700/*') {
    throw "SYNAPSE_CHROME_EXTENSION_LOCALHOST_PERMISSION_MISSING path=$manifestPath remediation=normal bridge requires host_permissions http://127.0.0.1:7700/* for direct daemon registration and message posting"
}
}

$extensionDeploy = Install-SynapseChromeBridgeFiles -SourceDir $extensionSourceDir -Destination $extensionDirFull -BrowserRow $script:BrowserRow -ProvisionAuth {
    param($Path, $Row)
    & (Join-Path $PSScriptRoot 'provision-synapse-chrome-auth.ps1') -ExtensionDir $Path -Browser $Browser -BrowsersDir $BrowsersDir
}
$extensionDir = $extensionDeploy.deployed_dir
$manifestPath = Join-Path $extensionDir 'manifest.json'
$extensionManifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$requiredPermissions = @($extensionManifest.permissions)
$optionalPermissions = @($extensionManifest.optional_permissions)
$hostPermissions = @($extensionManifest.host_permissions)
$bridgeAuthProvisioning = $extensionDeploy.registration_auth
$bridgeConfigPath = Join-Path $extensionDir 'bridge-config.json'
$extensionDeploy | Add-Member -NotePropertyName stable_root -NotePropertyValue $stableRootFull
$extensionDeploy | Add-Member -NotePropertyName build_id -NotePropertyValue $bridgeBuildId
$extensionDeploy | Add-Member -NotePropertyName declared_build_sha256 -NotePropertyValue $bridgeDeclaredBuildSha256
$extensionDeploy | Add-Member -NotePropertyName build_sha256 -NotePropertyValue $bridgeDeclaredBuildSha256

function Get-SynapseChromeInstallOutcome {
    param([object]$AutoInstall)
    [pscustomobject]@{ ok=($null -ne $AutoInstall -and $AutoInstall.ok -eq $true); reason=if ($null -ne $AutoInstall) { $AutoInstall.reason } else { 'runtime_not_verified' } }
}

function Read-SynapseChromeBridgeLiveOutcome {
    # Transport, profile switch, session cleanup and comparison live in
    # synapse-bridge-live-identity.ps1, shared with setup's post-start check.
    param([object]$Expected)
    $tokenPath = Join-Path $env:APPDATA 'synapse\token.txt'
    try {
        $daemonToken = if (Test-Path -LiteralPath $tokenPath -PathType Leaf) { [IO.File]::ReadAllText($tokenPath).Trim() } else { ([string]$env:SYNAPSE_BEARER_TOKEN).Trim() }
    } catch { return [pscustomobject]@{ok=$false;reason='daemon_health_credential_unavailable'} }
    try { return Read-SynapseBridgeLiveOutcome -Expected $Expected -Token $daemonToken }
    finally { $daemonToken = $null }
}

function Get-SynapseChromeRuntimeOutcome {
    # Truthful runtime outcome (#251): an auto-install that deferred, skipped or
    # failed keeps its own reason and never opens a daemon session; a caller
    # that verifies after its own daemon start gets a deferred outcome; only a
    # live identity readback can produce ok=true.
    param([object]$AutoInstall, [object]$Expected, [switch]$DeferLive)
    $reason = if ($null -ne $AutoInstall) { [string]$AutoInstall.reason } else { 'auto_install_readback_missing' }
    if ($null -eq $AutoInstall -or $reason -match 'deferred|skipped|no_profile|failed') {
        $outcome = [pscustomobject]@{ok=$false;deferred=($reason -match '_deferred_to_daemon');reason=$reason}
    } elseif ($DeferLive) {
        $outcome = [pscustomobject]@{ok=$false;deferred=$true;reason='live_runtime_verification_deferred_to_caller'}
    } else {
        $outcome = Read-SynapseChromeBridgeLiveOutcome -Expected $Expected
    }
    $outcome | Add-Member -NotePropertyName expected_identity -NotePropertyValue $Expected -Force
    return $outcome
}

function Initialize-SynapseChromeBridgeAutoInstallInterop {
    try {
        Add-Type -AssemblyName UIAutomationClient -ErrorAction Stop
        Add-Type -AssemblyName UIAutomationTypes -ErrorAction Stop
    } catch {
        throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_UIA_UNAVAILABLE error=$($_.Exception.Message) remediation=run from an interactive Windows desktop where UIAutomationClient is available, or load the deployed stable Chrome bridge directory manually once and rerun setup"
    }

    if (-not ('SynapseChromeBridgeAutoInstall.Win32' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace SynapseChromeBridgeAutoInstall {
    public static class Win32 {
        [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
        [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
        [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, UIntPtr dwExtraInfo);
        [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
        [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, UIntPtr dwExtraInfo);
    }
}
'@ -ErrorAction Stop
    }
}

function Get-SynapseActiveChromeProfileName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ChromeUserDataRoot,
        [string]$ExtensionId,
        [string]$ExtensionDir
    )

    if ([string]$script:BrowserRow.profile_layout -ceq 'single') {
        # Single-profile layout (Opera / Opera GX): the user-data root itself is
        # the one profile; its directory name is the profile name.
        if (-not (Test-Path -LiteralPath $ChromeUserDataRoot -PathType Container)) {
            return $null
        }
        return [string](Get-Item -LiteralPath $ChromeUserDataRoot).Name
    }

    $localStatePath = Join-Path $ChromeUserDataRoot 'Local State'
    $candidates = New-Object System.Collections.Generic.List[string]
    if (Test-Path -LiteralPath $localStatePath -PathType Leaf) {
        try {
            $localState = Get-Content -Raw -LiteralPath $localStatePath | ConvertFrom-Json -ErrorAction Stop
            if ($localState.profile -and $localState.profile.last_used) {
                $candidates.Add([string]$localState.profile.last_used) | Out-Null
            }
            if ($localState.profile -and $localState.profile.last_active_profiles) {
                foreach ($candidate in @($localState.profile.last_active_profiles)) {
                    $candidates.Add([string]$candidate) | Out-Null
                }
            }
        } catch {
            $candidates.Clear()
        }
    }

    $uniqueCandidates = @($candidates | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
    if (-not [string]::IsNullOrWhiteSpace($ExtensionId) -and -not [string]::IsNullOrWhiteSpace($ExtensionDir)) {
        foreach ($candidate in $uniqueCandidates) {
            if (-not (Test-Path -LiteralPath (Join-Path $ChromeUserDataRoot $candidate) -PathType Container)) {
                continue
            }
            $row = Test-SynapseChromeBridgeProfileRow `
                -ChromeUserDataRoot $ChromeUserDataRoot `
                -ProfileName $candidate `
                -ExtensionId $ExtensionId `
                -ExtensionDir $ExtensionDir
            if ($row.installed -and $row.manifest_path_matches) {
                return $candidate
            }
        }

        $installedProfiles = @()
        foreach ($profileDir in @(Get-SynapseBrowserProfileDirs -Row $script:BrowserRow -UserDataRoot $ChromeUserDataRoot)) {
            $row = Test-SynapseChromeBridgeProfileRow `
                -ChromeUserDataRoot $ChromeUserDataRoot `
                -ProfileName $profileDir.Name `
                -ExtensionId $ExtensionId `
                -ExtensionDir $ExtensionDir
            if ($row.installed -and $row.manifest_path_matches) {
                $installedProfiles += [string]$profileDir.Name
            }
        }
        $installedProfiles = @($installedProfiles | Sort-Object -Unique)
        if ($installedProfiles.Count -eq 1) {
            return [string]$installedProfiles[0]
        }
    }

    foreach ($candidate in $uniqueCandidates) {
        if (Test-Path -LiteralPath (Join-Path $ChromeUserDataRoot $candidate) -PathType Container) {
            return $candidate
        }
    }
    return $null
}

function Get-SynapseChromeBridgeManifestApiPermissions {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExtensionDir
    )

    $manifestPath = Join-Path $ExtensionDir 'manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
        return @()
    }
    try {
        $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json -ErrorAction Stop
    } catch {
        return @()
    }
    if (-not $manifest.permissions) {
        return @()
    }
    @($manifest.permissions | ForEach-Object { [string]$_ } | Where-Object {
            -not [string]::IsNullOrWhiteSpace($_) -and
            $_ -notmatch '://' -and
            $_ -ne '<all_urls>'
        } | Sort-Object -Unique)
}

function Test-SynapseChromeBridgeProfileRow {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ChromeUserDataRoot,
        [Parameter(Mandatory = $true)]
        [string]$ProfileName,
        [Parameter(Mandatory = $true)]
        [string]$ExtensionId,
        [Parameter(Mandatory = $true)]
        [string]$ExtensionDir
    )

    $profilePath = Resolve-SynapseBrowserProfilePath -Row $script:BrowserRow -UserDataRoot $ChromeUserDataRoot -ProfileName $ProfileName
    foreach ($prefFileName in @('Preferences', 'Secure Preferences')) {
        $prefPath = Join-Path $profilePath $prefFileName
        if (-not (Test-Path -LiteralPath $prefPath -PathType Leaf)) {
            continue
        }
        try {
            $pref = Get-Content -Raw -LiteralPath $prefPath | ConvertFrom-Json -ErrorAction Stop
        } catch {
            continue
        }
        if (-not $pref.extensions -or -not $pref.extensions.settings) {
            continue
        }
        $property = $pref.extensions.settings.PSObject.Properties[$ExtensionId]
        if (-not $property) {
            continue
        }
        $setting = $property.Value
        $manifestPath = if ($setting.PSObject.Properties.Name -contains 'path') { [string]$setting.path } else { $null }
        $manifestMatches = $false
        if (-not [string]::IsNullOrWhiteSpace($manifestPath)) {
            try {
                $manifestMatches = ((Resolve-Path -LiteralPath $manifestPath -ErrorAction Stop).Path -eq (Resolve-Path -LiteralPath $ExtensionDir -ErrorAction Stop).Path)
            } catch {
                $manifestMatches = ($manifestPath -ieq $ExtensionDir)
            }
        }
        $activeApiPermissions = @()
        if ($setting.PSObject.Properties.Name -contains 'active_permissions' -and $setting.active_permissions -and $setting.active_permissions.api) {
            $activeApiPermissions = @($setting.active_permissions.api | ForEach-Object { [string]$_ } | Sort-Object -Unique)
        }
        $grantedApiPermissions = @()
        if ($setting.PSObject.Properties.Name -contains 'granted_permissions' -and $setting.granted_permissions -and $setting.granted_permissions.api) {
            $grantedApiPermissions = @($setting.granted_permissions.api | ForEach-Object { [string]$_ } | Sort-Object -Unique)
        }
        $disableReasons = @()
        if ($setting.PSObject.Properties.Name -contains 'disable_reasons' -and $null -ne $setting.disable_reasons) {
            $disableReasons = @($setting.disable_reasons | ForEach-Object { [int]$_ })
        }
        $requiredApiPermissions = Get-SynapseChromeBridgeManifestApiPermissions -ExtensionDir $ExtensionDir
        $missingActiveApiPermissions = @($requiredApiPermissions | Where-Object {
                $activeApiPermissions -notcontains $_
            })
        # enabled_and_permissioned is PATH-INDEPENDENT: it proves the loaded extension
        # is enabled (no disable_reasons) and holds every required API permission,
        # without requiring its recorded load path to equal the latest stable dir. The
        # Synapse bridge extension ID is derived from the manifest key and is therefore
        # the same regardless of which directory Chrome loaded it from, so a non-stable
        # path does not make a working bridge any less the Synapse bridge.
        $enabledAndPermissioned = ($disableReasons.Count -eq 0) -and
            ($missingActiveApiPermissions.Count -eq 0)
        # manifest_dir_exists confirms the directory Chrome loaded the unpacked
        # extension from is still present on disk with a manifest. If a prior install's
        # directory was deleted, the row is stale and a genuine reinstall is required.
        $manifestDirExists = (-not [string]::IsNullOrWhiteSpace($manifestPath)) -and
            (Test-Path -LiteralPath $manifestPath -PathType Container) -and
            (Test-Path -LiteralPath (Join-Path $manifestPath 'manifest.json') -PathType Leaf)
        $ready = $manifestMatches -and $enabledAndPermissioned
        return [pscustomobject]@{
            installed = $true
            profile = $ProfileName
            pref_file = $prefFileName
            pref_path = $prefPath
            manifest_path = $manifestPath
            manifest_path_matches = $manifestMatches
            manifest_dir_exists = $manifestDirExists
            active_api_permissions = $activeApiPermissions
            granted_api_permissions = $grantedApiPermissions
            disable_reasons = $disableReasons
            required_api_permissions = $requiredApiPermissions
            missing_active_api_permissions = $missingActiveApiPermissions
            enabled_and_permissioned = $enabledAndPermissioned
            ready = $ready
        }
    }

    [pscustomobject]@{
        installed = $false
        profile = $ProfileName
        pref_file = $null
        pref_path = $null
        manifest_path = $null
        manifest_path_matches = $false
        manifest_dir_exists = $false
        active_api_permissions = @()
        granted_api_permissions = @()
        disable_reasons = @()
        required_api_permissions = Get-SynapseChromeBridgeManifestApiPermissions -ExtensionDir $ExtensionDir
        missing_active_api_permissions = Get-SynapseChromeBridgeManifestApiPermissions -ExtensionDir $ExtensionDir
        enabled_and_permissioned = $false
        ready = $false
    }
}

function ConvertTo-SynapseComparablePath {
    param(
        [AllowNull()]
        [string]$Path
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $null
    }
    $expanded = [System.Environment]::ExpandEnvironmentVariables($Path.Trim())
    try {
        $full = [System.IO.Path]::GetFullPath($expanded)
    } catch {
        $full = $expanded
    }
    $trimChars = [char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    return $full.TrimEnd($trimChars)
}

function Get-SynapseCommandLineSwitchValue {
    param(
        [AllowNull()]
        [string]$CommandLine,
        [Parameter(Mandatory = $true)]
        [string]$SwitchName
    )

    if ([string]::IsNullOrWhiteSpace($CommandLine)) {
        return $null
    }
    $pattern = '(?i)(?:^|\s)' + [regex]::Escape($SwitchName) + '(?:=|\s+)(?:"([^"]+)"|''([^'']+)''|([^\s"]+))'
    $match = [regex]::Match($CommandLine, $pattern)
    if (-not $match.Success) {
        return $null
    }
    foreach ($index in 1..3) {
        if ($match.Groups[$index].Success -and -not [string]::IsNullOrWhiteSpace($match.Groups[$index].Value)) {
            return $match.Groups[$index].Value
        }
    }
    return $null
}

function Get-SynapseChromeProcessProfileMatch {
    param(
        [AllowNull()]
        [string]$CommandLine,
        [AllowNull()]
        [string]$ChromeUserDataRoot
    )

    $commandLineReadable = -not [string]::IsNullOrWhiteSpace($CommandLine)
    $userDataDir = Get-SynapseCommandLineSwitchValue -CommandLine $CommandLine -SwitchName '--user-data-dir'
    $normalizedUserDataDir = ConvertTo-SynapseComparablePath -Path $userDataDir
    $expectedRoot = ConvertTo-SynapseComparablePath -Path $ChromeUserDataRoot
    $hasMsPlaywrightMcpDir = $commandLineReadable -and $CommandLine -match 'ms-playwright-mcp'
    $hasRemoteDebuggingPipe = $commandLineReadable -and $CommandLine -match '(^|\s)--remote-debugging-pipe(\s|=|$)'
    $hasRemoteDebuggingPort = $commandLineReadable -and $CommandLine -match '(^|\s)--remote-debugging-port(\s|=|$)'
    $hasAutomationControlledFlag = $commandLineReadable -and $CommandLine -match '(^|\s)--disable-blink-features=([^\s]*,)?AutomationControlled(,|\s|$)'

    $matches = $true
    $reason = 'not_checked'
    if (-not [string]::IsNullOrWhiteSpace($expectedRoot)) {
        if (-not $commandLineReadable) {
            $matches = $false
            $reason = 'command_line_unreadable'
        } elseif ($hasMsPlaywrightMcpDir) {
            $matches = $false
            $reason = 'dedicated_ms_playwright_mcp_user_data_dir'
        } elseif ([string]::IsNullOrWhiteSpace($normalizedUserDataDir)) {
            $matches = $true
            $reason = 'default_chrome_user_data_root'
        } elseif ([string]::Equals($normalizedUserDataDir, $expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
            $matches = $true
            $reason = 'user_data_root_matches_active_profile_root'
        } else {
            $matches = $false
            $reason = 'user_data_root_mismatch'
        }
    }

    [pscustomobject]@{
        command_line_readable = $commandLineReadable
        user_data_dir = $userDataDir
        user_data_dir_normalized = $normalizedUserDataDir
        expected_user_data_root = $expectedRoot
        matches_active_profile_root = $matches
        match_reason = $reason
        has_ms_playwright_mcp_dir = $hasMsPlaywrightMcpDir
        has_remote_debugging_pipe = $hasRemoteDebuggingPipe
        has_remote_debugging_port = $hasRemoteDebuggingPort
        has_automation_controlled_flag = $hasAutomationControlledFlag
    }
}

function Format-SynapseChromeWindowRejectSummary {
    param(
        [object[]]$Windows
    )

    $rows = @($Windows | Select-Object -First 8 | ForEach-Object {
        'hwnd={0},pid={1},title="{2}",eligible={3},reason={4},user_data_dir="{5}",remote_pipe={6},remote_port={7},ms_playwright_mcp={8}' -f `
            $_.hwnd,
            $_.pid,
            (([string]$_.title) -replace '"', '\"'),
            $_.chrome_profile_eligible,
            $_.chrome_profile_match_reason,
            (([string]$_.chrome_user_data_dir) -replace '"', '\"'),
            $_.has_remote_debugging_pipe,
            $_.has_remote_debugging_port,
            $_.has_ms_playwright_mcp_dir
    })
    $extra = [Math]::Max(0, @($Windows).Count - $rows.Count)
    $suffix = if ($extra -gt 0) { " | +$extra more" } else { '' }
    return (($rows -join ' | ') + $suffix)
}

function Get-SynapseChromeTopLevelWindows {
    param(
        [AllowNull()]
        [string]$ChromeUserDataRoot
    )

    Initialize-SynapseChromeBridgeAutoInstallInterop
    $chromeProcessRows = @(Get-CimInstance Win32_Process -Filter $script:BrowserProcessWmiFilter -ErrorAction SilentlyContinue |
        ForEach-Object {
            $commandLine = [string]$_.CommandLine
            $profileMatch = Get-SynapseChromeProcessProfileMatch -CommandLine $commandLine -ChromeUserDataRoot $ChromeUserDataRoot
            [pscustomobject]@{
                pid = [int]$_.ProcessId
                command_line_readable = $profileMatch.command_line_readable
                chrome_user_data_dir = $profileMatch.user_data_dir
                chrome_user_data_dir_normalized = $profileMatch.user_data_dir_normalized
                chrome_profile_eligible = $profileMatch.matches_active_profile_root
                chrome_profile_match_reason = $profileMatch.match_reason
                has_ms_playwright_mcp_dir = $profileMatch.has_ms_playwright_mcp_dir
                has_remote_debugging_pipe = $profileMatch.has_remote_debugging_pipe
                has_remote_debugging_port = $profileMatch.has_remote_debugging_port
                has_automation_controlled_flag = $profileMatch.has_automation_controlled_flag
            }
        })
    if ($chromeProcessRows.Count -eq 0) {
        return @()
    }
    $chromeProcesses = @($chromeProcessRows | ForEach-Object { [int]$_.pid })
    $chromeProcessByPid = @{}
    foreach ($row in $chromeProcessRows) {
        $chromeProcessByPid[[int]$row.pid] = $row
    }
    $foreground = [SynapseChromeBridgeAutoInstall.Win32]::GetForegroundWindow().ToInt64()
    $root = [System.Windows.Automation.AutomationElement]::RootElement
    $children = $root.FindAll(
        [System.Windows.Automation.TreeScope]::Children,
        [System.Windows.Automation.Condition]::TrueCondition
    )
    $windows = @()
    foreach ($child in $children) {
        $current = $child.Current
        $processId = [int]$current.ProcessId
        if ($chromeProcesses -notcontains $processId) {
            continue
        }
        $hwnd = [int64]$current.NativeWindowHandle
        $title = [string]$current.Name
        if ($hwnd -eq 0 -or [string]::IsNullOrWhiteSpace($title)) {
            continue
        }
        $processInfo = $chromeProcessByPid[$processId]
        $windows += [pscustomobject]@{
            hwnd = $hwnd
            pid = $processId
            title = $title
            class_name = [string]$current.ClassName
            is_foreground = ($hwnd -eq $foreground)
            command_line_readable = $processInfo.command_line_readable
            chrome_user_data_dir = $processInfo.chrome_user_data_dir
            chrome_user_data_dir_normalized = $processInfo.chrome_user_data_dir_normalized
            chrome_profile_eligible = $processInfo.chrome_profile_eligible
            chrome_profile_match_reason = $processInfo.chrome_profile_match_reason
            has_ms_playwright_mcp_dir = $processInfo.has_ms_playwright_mcp_dir
            has_remote_debugging_pipe = $processInfo.has_remote_debugging_pipe
            has_remote_debugging_port = $processInfo.has_remote_debugging_port
            has_automation_controlled_flag = $processInfo.has_automation_controlled_flag
            element = $child
        }
    }
    return @($windows)
}

function Find-SynapseAutomationElementByName {
    param(
        [Parameter(Mandatory = $true)]
        [System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [AllowNull()]
        [System.Windows.Automation.ControlType]$ControlType
    )

    $nameCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::NameProperty,
        $Name
    )
    $condition = $nameCondition
    if ($null -ne $ControlType) {
        $typeCondition = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            $ControlType
        )
        $condition = [System.Windows.Automation.AndCondition]::new($nameCondition, $typeCondition)
    }
    $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}

function Find-SynapseAutomationElementByAutomationId {
    param(
        [Parameter(Mandatory = $true)]
        [System.Windows.Automation.AutomationElement]$Root,
        [Parameter(Mandatory = $true)]
        [string]$AutomationId,
        [AllowNull()]
        [System.Windows.Automation.ControlType]$ControlType
    )

    $idCondition = [System.Windows.Automation.PropertyCondition]::new(
        [System.Windows.Automation.AutomationElement]::AutomationIdProperty,
        $AutomationId
    )
    $condition = $idCondition
    if ($null -ne $ControlType) {
        $typeCondition = [System.Windows.Automation.PropertyCondition]::new(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            $ControlType
        )
        $condition = [System.Windows.Automation.AndCondition]::new($idCondition, $typeCondition)
    }
    $Root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
}

function Invoke-SynapseAutomationElement {
    param(
        [Parameter(Mandatory = $true)]
        [System.Windows.Automation.AutomationElement]$Element,
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    try {
        $invoke = $Element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
        $invoke.Invoke()
        return
    } catch {
        try {
            $toggle = $Element.GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
            $toggle.Toggle()
            return
        } catch {
            throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_INVOKE_FAILED control=$Description error=$($_.Exception.Message)"
        }
    }
}

function Invoke-SynapseAutomationElementMouseClick {
    param(
        [Parameter(Mandatory = $true)]
        [System.Windows.Automation.AutomationElement]$Element,
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    $rect = $Element.Current.BoundingRectangle
    if ($rect.Width -le 0 -or $rect.Height -le 0) {
        throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_MOUSE_CLICK_FAILED control=$Description reason=empty_bounding_rect"
    }
    $x = [int]($rect.Left + ($rect.Width / 2))
    $y = [int]($rect.Top + ($rect.Height / 2))
    [SynapseChromeBridgeAutoInstall.Win32]::SetCursorPos($x, $y) | Out-Null
    Start-Sleep -Milliseconds 80
    [SynapseChromeBridgeAutoInstall.Win32]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
    [SynapseChromeBridgeAutoInstall.Win32]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
}

function Set-SynapseAutomationEditValue {
    param(
        [Parameter(Mandatory = $true)]
        [System.Windows.Automation.AutomationElement]$Element,
        [Parameter(Mandatory = $true)]
        [string]$Value,
        [Parameter(Mandatory = $true)]
        [string]$Description
    )

    try {
        $valuePattern = $Element.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        $valuePattern.SetValue($Value)
    } catch {
        throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_SET_FIELD_FAILED field=$Description error=$($_.Exception.Message)"
    }
}

function Set-SynapseChromeAddressBarUrl {
    param(
        [Parameter(Mandatory = $true)]
        $Window,
        [Parameter(Mandatory = $true)]
        [string]$Url
    )

    $addressBar = Find-SynapseAutomationElementByName `
        -Root $Window.element `
        -Name 'Address and search bar' `
        -ControlType ([System.Windows.Automation.ControlType]::Edit)
    if (-not $addressBar) {
        throw "SYNAPSE_CHROME_ADDRESS_BAR_NOT_FOUND chrome_window_hwnd=$($Window.hwnd) chrome_window_pid=$($Window.pid) title=$($Window.title) remediation=Chrome did not expose the address bar through UI Automation; setup refuses clipboard/key-only navigation or launching another Chrome profile"
    }

    try {
        $addressBar.SetFocus()
    } catch {
        throw "SYNAPSE_CHROME_ADDRESS_BAR_FOCUS_FAILED chrome_window_hwnd=$($Window.hwnd) chrome_window_pid=$($Window.pid) error=$($_.Exception.Message) remediation=repair the existing Chrome window focus/UIA state before bridge UI repair"
    }
    Start-Sleep -Milliseconds 100
    try {
        $valuePattern = $addressBar.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
        $valuePattern.SetValue($Url)
    } catch {
        throw "SYNAPSE_CHROME_ADDRESS_BAR_SET_VALUE_FAILED chrome_window_hwnd=$($Window.hwnd) chrome_window_pid=$($Window.pid) url=$Url error=$($_.Exception.Message) remediation=Chrome address bar did not accept UIA ValuePattern input; setup refuses to navigate by arbitrary coordinate clicks"
    }
    Start-Sleep -Milliseconds 100
    Send-SynapseNativeKeyTap -VirtualKey 0x0D
}

function Send-SynapseNativeKeyDown {
    param([byte]$VirtualKey)
    [SynapseChromeBridgeAutoInstall.Win32]::keybd_event($VirtualKey, 0, 0, [UIntPtr]::Zero)
}

function Send-SynapseNativeKeyUp {
    param([byte]$VirtualKey)
    [SynapseChromeBridgeAutoInstall.Win32]::keybd_event($VirtualKey, 0, 2, [UIntPtr]::Zero)
}

function Send-SynapseNativeKeyChord {
    param([byte[]]$VirtualKeys)
    foreach ($key in $VirtualKeys) {
        Send-SynapseNativeKeyDown -VirtualKey $key
        Start-Sleep -Milliseconds 25
    }
    [array]::Reverse($VirtualKeys)
    foreach ($key in $VirtualKeys) {
        Start-Sleep -Milliseconds 25
        Send-SynapseNativeKeyUp -VirtualKey $key
    }
}

function Send-SynapseNativeKeyTap {
    param([byte]$VirtualKey)
    Send-SynapseNativeKeyDown -VirtualKey $VirtualKey
    Start-Sleep -Milliseconds 60
    Send-SynapseNativeKeyUp -VirtualKey $VirtualKey
}

function Wait-SynapseUntil {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock]$Probe,
        [Parameter(Mandatory = $true)]
        [datetime]$Deadline,
        [int]$SleepMilliseconds = 250
    )

    do {
        $value = & $Probe
        if ($value) {
            return $value
        }
        Start-Sleep -Milliseconds $SleepMilliseconds
    } while ((Get-Date) -lt $Deadline)
    return $null
}

function Find-SynapseChromeFolderDialog {
    param(
        [Parameter(Mandatory = $true)]
        [int64]$ChromeWindowHwnd,
        [Parameter(Mandatory = $true)]
        [int]$ChromeWindowPid
    )

    $chromeWindows = @(Get-SynapseChromeTopLevelWindows)
    foreach ($window in $chromeWindows) {
        if ($window.title -match '^Select the extension directory\.?$' -and $window.pid -eq $ChromeWindowPid) {
            return $window
        }
        if ($window.hwnd -ne $ChromeWindowHwnd) {
            continue
        }
        $dialog = Find-SynapseAutomationElementByName `
            -Root $window.element `
            -Name 'Select the extension directory.' `
            -ControlType ([System.Windows.Automation.ControlType]::Window)
        if ($dialog) {
            $current = $dialog.Current
            return [pscustomobject]@{
                hwnd = [int64]$current.NativeWindowHandle
                pid = $window.pid
                title = [string]$current.Name
                class_name = [string]$current.ClassName
                is_foreground = $false
                element = $dialog
            }
        }
    }
    return $null
}

function Get-SynapseChromeWindowByHwnd {
    param(
        [Parameter(Mandatory = $true)]
        [int64]$Hwnd
    )

    @(Get-SynapseChromeTopLevelWindows | Where-Object { $_.hwnd -eq $Hwnd } | Select-Object -First 1)[0]
}

function Read-SynapseChromeExtensionDetailsUiState {
    param(
        [Parameter(Mandatory = $true)]
        $Window
    )

    $reloadButton = Find-SynapseAutomationElementByAutomationId `
        -Root $Window.element `
        -AutomationId 'dev-reload-button' `
        -ControlType ([System.Windows.Automation.ControlType]::Button)
    $enableToggle = Find-SynapseAutomationElementByAutomationId `
        -Root $Window.element `
        -AutomationId 'enableToggle' `
        -ControlType ([System.Windows.Automation.ControlType]::Button)
    $serviceWorker = Find-SynapseAutomationElementByName `
        -Root $Window.element `
        -Name 'service worker' `
        -ControlType ([System.Windows.Automation.ControlType]::Hyperlink)
    $extensionName = Find-SynapseAutomationElementByName `
        -Root $Window.element `
        -Name 'Synapse Chrome Bridge' `
        -ControlType $null

    $enableToggleName = if ($enableToggle) { [string]$enableToggle.Current.Name } else { '<missing>' }
    [pscustomobject]@{
        hwnd = [int64]$Window.hwnd
        pid = [int]$Window.pid
        title = [string]$Window.title
        class_name = [string]$Window.class_name
        reload_button_present = ($null -ne $reloadButton)
        enable_toggle_present = ($null -ne $enableToggle)
        enable_toggle_name = $enableToggleName
        enable_toggle_on = ($enableToggleName -match '^On\b|extension enabled')
        service_worker_link_present = ($null -ne $serviceWorker)
        extension_name_present = ($null -ne $extensionName)
    }
}

function Resolve-SynapseUiRepairHumanStateGate {
    # #2271: may foreground chrome://extensions UI repair run now? Pure: takes the
    # SHQueryUserNotificationState HRESULT and its QUERY_USER_NOTIFICATION_STATE value.
    param([int]$HResult, [int]$State)
    if ($HResult -ne 0) {
        return [pscustomobject]@{ Allowed = $false; Code = 'SYNAPSE_CHROME_BRIDGE_UI_REPAIR_HUMAN_STATE_UNREADABLE' }
    }
    # Allowlist: 5 = QUNS_ACCEPTS_NOTIFICATIONS, 6 = QUNS_QUIET_TIME (a normal desktop).
    if ($State -in @(5, 6)) {
        return [pscustomobject]@{ Allowed = $true; Code = '' }
    }
    # 2 = QUNS_BUSY, 3 = QUNS_RUNNING_D3D_FULL_SCREEN, 4 = QUNS_PRESENTATION_MODE, 7 = QUNS_APP.
    if ($State -in @(2, 3, 4, 7)) {
        return [pscustomobject]@{ Allowed = $false; Code = 'SYNAPSE_CHROME_BRIDGE_UI_REPAIR_REFUSED_HUMAN_FULLSCREEN' }
    }
    # 1 = QUNS_NOT_PRESENT (locked, screen saver, inactive session) and any unknown value.
    return [pscustomobject]@{ Allowed = $false; Code = 'SYNAPSE_CHROME_BRIDGE_UI_REPAIR_REFUSED_HUMAN_STATE' }
}

function Assert-SynapseHumanAllowsForegroundUiRepair {
    # Thin OS layer for Resolve-SynapseUiRepairHumanStateGate; runs before any
    # ShowWindowAsync/SetForegroundWindow of the UI repair paths.
    param([Parameter(Mandatory = $true)][string]$Stage)
    if (-not ('SynapseChromeBridgeHumanGuard.Shell' -as [type])) {
        Add-Type -Namespace SynapseChromeBridgeHumanGuard -Name Shell -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("shell32.dll")]
public static extern int SHQueryUserNotificationState(out int state);
'@
    }
    $state = 0
    $hr = [SynapseChromeBridgeHumanGuard.Shell]::SHQueryUserNotificationState([ref]$state)
    $gate = Resolve-SynapseUiRepairHumanStateGate -HResult $hr -State $state
    if (-not $gate.Allowed) {
        throw "$($gate.Code) stage=$Stage hresult=$hr user_notification_state=$state remediation=foreground chrome://extensions UI repair never runs while the human is fullscreen, busy, presenting, away/locked, or the state is unreadable; the bridge worker re-registers on its own wake alarm, re-read bridge health later"
    }
}

function Invoke-SynapseChromeBridgeExistingUiReload {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ChromeUserDataRoot,
        [Parameter(Mandatory = $true)]
        [string]$ExtensionId,
        [Parameter(Mandatory = $true)]
        [string]$ExtensionDir,
        [Parameter(Mandatory = $true)]
        [string]$ActiveProfile,
        [Parameter(Mandatory = $true)]
        $Before,
        [Parameter(Mandatory = $true)]
        [int]$TimeoutSeconds
    )

    $expectedReady = ($Before.installed -and $Before.manifest_path_matches -and $Before.ready)
    $nonstableReady = ($Before.installed -and $Before.enabled_and_permissioned -and $Before.manifest_dir_exists)
    if (-not $expectedReady -and -not $nonstableReady) {
        $missing = if ($Before.missing_active_api_permissions.Count -eq 0) { '<none>' } else { $Before.missing_active_api_permissions -join ',' }
        $disableReasons = if ($Before.disable_reasons.Count -eq 0) { '<none>' } else { $Before.disable_reasons -join ',' }
        throw "SYNAPSE_CHROME_BRIDGE_UI_RELOAD_ROW_NOT_READY active_profile=$ActiveProfile installed=$($Before.installed) manifest_path_matches=$($Before.manifest_path_matches) ready=$($Before.ready) enabled_and_permissioned=$($Before.enabled_and_permissioned) manifest_dir_exists=$($Before.manifest_dir_exists) missing_active_api_permissions=$missing disable_reasons=$disableReasons remediation=UI reload is only valid for an already-installed, enabled Synapse bridge row; repair the profile row before trying to reload it"
    }

    Initialize-SynapseChromeBridgeAutoInstallInterop
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $candidateWindows = @(Get-SynapseChromeTopLevelWindows -ChromeUserDataRoot $ChromeUserDataRoot | Where-Object {
        $_.title -notmatch '^Select the extension directory\.?$'
    })
    $windows = @($candidateWindows | Where-Object { $_.chrome_profile_eligible -eq $true })
    if ($windows.Count -eq 0) {
        # No eligible interactive Chrome window means the extension row is already
        # installed from disk state, but the live UI reload cannot run. This is a
        # bridge repair problem, not a daemon/install contract failure (#132):
        # a missing window must not abort setup after the daemon is healthy. The
        # profile row is unchanged, the daemon can reconverge the extension code
        # in place via browser_debugger.reload_bridge once a live bridge host
        # exists, and the deferred reason below surfaces in bridge health.
        $rejected = Format-SynapseChromeWindowRejectSummary -Windows $candidateWindows
        return [pscustomobject]@{
            attempted = $false
            changed = $false
            reason = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
            active_profile = $ActiveProfile
            required_foreground = $false
            bridge_self_reload_command = 'browser_debugger.reload_bridge'
            rejected_windows = $rejected
            remediation = 'open the already-authenticated Chrome profile for this user-data root and rerun setup, or call browser_debugger.reload_bridge through the live daemon once a bridge host exists; setup refuses to launch a second Chrome window or drive a dedicated automation profile'
            before = $Before
            after = $Before
        }
    }

    $chromeWindow = @($windows | Sort-Object @{ Expression = 'is_foreground'; Descending = $true }, @{ Expression = 'title'; Descending = $false } | Select-Object -First 1)[0]
    Assert-SynapseHumanAllowsForegroundUiRepair -Stage 'existing_ui_reload'
    [SynapseChromeBridgeAutoInstall.Win32]::ShowWindowAsync([IntPtr]$chromeWindow.hwnd, 9) | Out-Null
    [SynapseChromeBridgeAutoInstall.Win32]::SetForegroundWindow([IntPtr]$chromeWindow.hwnd) | Out-Null
    Start-Sleep -Milliseconds 300

    Set-SynapseChromeAddressBarUrl `
        -Window $chromeWindow `
        -Url "$($script:BrowserExtensionsPageUrl)/?id=$ExtensionId"

    $details = Wait-SynapseUntil -Deadline $deadline -Probe {
        $currentWindow = @(Get-SynapseChromeTopLevelWindows -ChromeUserDataRoot $ChromeUserDataRoot | Where-Object {
            $_.hwnd -eq $chromeWindow.hwnd
        } | Select-Object -First 1)
        if ($currentWindow.Count -eq 0) {
            return $null
        }
        $reloadButton = Find-SynapseAutomationElementByAutomationId `
            -Root $currentWindow[0].element `
            -AutomationId 'dev-reload-button' `
            -ControlType ([System.Windows.Automation.ControlType]::Button)
        if (-not $reloadButton) {
            return $null
        }
        [pscustomobject]@{
            window = $currentWindow[0]
            reload_button = $reloadButton
            ui_before = Read-SynapseChromeExtensionDetailsUiState -Window $currentWindow[0]
        }
    }
    if (-not $details) {
        $latestWindow = Get-SynapseChromeWindowByHwnd -Hwnd $chromeWindow.hwnd
        $latestTitle = if ($latestWindow) { [string]$latestWindow.title } else { '<missing>' }
        throw "SYNAPSE_CHROME_BRIDGE_UI_RELOAD_BUTTON_NOT_FOUND active_profile=$ActiveProfile timeout_s=$TimeoutSeconds chrome_window_hwnd=$($chromeWindow.hwnd) chrome_window_pid=$($chromeWindow.pid) latest_title=$latestTitle user_data_dir=$($chromeWindow.chrome_user_data_dir) match_reason=$($chromeWindow.chrome_profile_match_reason) remediation=Chrome did not expose the Synapse extension details Reload button through UI Automation; setup refuses to click arbitrary browser coordinates or launch another Chrome profile"
    }

    if (-not $details.ui_before.enable_toggle_present -or -not $details.ui_before.enable_toggle_on) {
        throw "SYNAPSE_CHROME_BRIDGE_UI_RELOAD_EXTENSION_DISABLED active_profile=$ActiveProfile chrome_window_hwnd=$($details.window.hwnd) chrome_window_pid=$($details.window.pid) enable_toggle_present=$($details.ui_before.enable_toggle_present) enable_toggle_name=$($details.ui_before.enable_toggle_name) remediation=the existing Synapse Chrome Bridge must be enabled before setup may reload it"
    }

    Invoke-SynapseAutomationElement -Element $details.reload_button -Description 'Synapse Chrome Bridge Reload'
    Start-Sleep -Milliseconds 1500
    $afterWindow = Get-SynapseChromeWindowByHwnd -Hwnd $chromeWindow.hwnd
    if (-not $afterWindow) {
        throw "SYNAPSE_CHROME_BRIDGE_UI_RELOAD_WINDOW_LOST active_profile=$ActiveProfile chrome_window_hwnd=$($chromeWindow.hwnd) chrome_window_pid=$($chromeWindow.pid) remediation=Chrome window disappeared after reload; keep the already-open authenticated profile available and rerun setup"
    }
    $uiAfter = Read-SynapseChromeExtensionDetailsUiState -Window $afterWindow
    $after = Test-SynapseChromeBridgeProfileRow `
        -ChromeUserDataRoot $ChromeUserDataRoot `
        -ProfileName $ActiveProfile `
        -ExtensionId $ExtensionId `
        -ExtensionDir $ExtensionDir
    $afterExpectedReady = ($after.installed -and $after.manifest_path_matches -and $after.ready)
    $afterNonstableReady = ($after.installed -and $after.enabled_and_permissioned -and $after.manifest_dir_exists)
    if (-not $afterExpectedReady -and -not $afterNonstableReady) {
        $missingAfter = if ($after.missing_active_api_permissions.Count -eq 0) { '<none>' } else { $after.missing_active_api_permissions -join ',' }
        $disableAfter = if ($after.disable_reasons.Count -eq 0) { '<none>' } else { $after.disable_reasons -join ',' }
        throw "SYNAPSE_CHROME_BRIDGE_UI_RELOAD_PROFILE_ROW_NOT_READY_AFTER active_profile=$ActiveProfile installed=$($after.installed) manifest_path_matches=$($after.manifest_path_matches) ready=$($after.ready) enabled_and_permissioned=$($after.enabled_and_permissioned) manifest_dir_exists=$($after.manifest_dir_exists) missing_active_api_permissions=$missingAfter disable_reasons=$disableAfter remediation=Chrome Reload changed the physical profile row unexpectedly; inspect the active profile Preferences/Secure Preferences before accepting bridge setup"
    }

    $reloadReason = if ($Before.manifest_path_matches) { 'existing_ready_extension_ui_reload_invoked' } else { 'existing_ready_extension_nonstable_path_ui_reload_invoked' }
    return [pscustomobject]@{
        attempted = $true
        changed = $true
        reason = $reloadReason
        active_profile = $ActiveProfile
        required_foreground = $true
        chrome_window_hwnd = $chromeWindow.hwnd
        chrome_window_pid = $chromeWindow.pid
        chrome_window_user_data_dir = $chromeWindow.chrome_user_data_dir
        chrome_window_profile_match_reason = $chromeWindow.chrome_profile_match_reason
        extension_details_url = "$($script:BrowserExtensionsPageUrl)/?id=$ExtensionId"
        ui_before = $details.ui_before
        ui_after = $uiAfter
        before = $Before
        after = $after
    }
}

# #132: record a repair this run deferred to the daemon so daemon bridge health
# can name it (`deferred_repair=<reason>`); any non-deferred result clears it.
function Set-SynapseChromeBridgeDeferredRepairMarker {
    param(
        [Parameter(Mandatory = $true)]
        $AutoInstall,
        [Parameter(Mandatory = $true)]
        [string]$MarkerPath
    )

    $reason = [string]$AutoInstall.reason
    if ($reason -match '^[A-Za-z0-9_]+_deferred_to_daemon[A-Za-z0-9_]*$') {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $MarkerPath) | Out-Null
        [System.IO.File]::WriteAllText($MarkerPath, $reason)
    }
    elseif (Test-Path -LiteralPath $MarkerPath) {
        Remove-Item -LiteralPath $MarkerPath -Force
    }
}

function Invoke-SynapseChromeBridgeAutoInstall {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ChromeUserDataRoot,
        [Parameter(Mandatory = $true)]
        [string]$ExtensionId,
        [Parameter(Mandatory = $true)]
        [string]$ExtensionDir,
        [Parameter(Mandatory = $true)]
        [int]$TimeoutSeconds
    )

    $activeProfile = Get-SynapseActiveChromeProfileName `
        -ChromeUserDataRoot $ChromeUserDataRoot `
        -ExtensionId $ExtensionId `
        -ExtensionDir $ExtensionDir
    if ([string]::IsNullOrWhiteSpace($activeProfile)) {
        throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_ACTIVE_PROFILE_UNKNOWN user_data_root=$ChromeUserDataRoot remediation=open the intended authenticated Chrome profile, then rerun setup"
    }
    $before = Test-SynapseChromeBridgeProfileRow `
        -ChromeUserDataRoot $ChromeUserDataRoot `
        -ProfileName $activeProfile `
        -ExtensionId $ExtensionId `
        -ExtensionDir $ExtensionDir
    if ($SkipAutoInstall) {
        return [pscustomobject]@{
            attempted = $false
            changed = $false
            reason = 'skip_auto_install_requested'
            active_profile = $activeProfile
            before = $before
            after = $before
        }
    }

    if ($before.installed -and $before.manifest_path_matches) {
        if ($before.ready) {
            if ($ReloadExistingExtensionViaUi) {
                return Invoke-SynapseChromeBridgeExistingUiReload `
                    -ChromeUserDataRoot $ChromeUserDataRoot `
                    -ExtensionId $ExtensionId `
                    -ExtensionDir $ExtensionDir `
                    -ActiveProfile $activeProfile `
                    -Before $before `
                    -TimeoutSeconds $TimeoutSeconds
            }
            return [pscustomobject]@{
                attempted = $true
                changed = $false
                reason = 'existing_ready_extension_code_reload_deferred_to_daemon_reloadself'
                active_profile = $activeProfile
                required_foreground = $false
                bridge_self_reload_command = 'browser_debugger.reload_bridge'
                before = $before
                after = $before
            }
        }

        $missing = if ($before.missing_active_api_permissions.Count -eq 0) { '<none>' } else { $before.missing_active_api_permissions -join ',' }
        $disableReasons = if ($before.disable_reasons.Count -eq 0) { '<none>' } else { $before.disable_reasons -join ',' }
        throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_EXISTING_EXTENSION_NOT_READY active_profile=$activeProfile ready=$($before.ready) missing_active_api_permissions=$missing disable_reasons=$disableReasons remediation=existing Synapse Chrome Bridge row is installed from the expected path but is not active/permissioned; setup refuses to steal foreground for chrome://extensions repair. Enable/permission the existing extension in the already-open profile or remove it and rerun setup for a first-time Load unpacked install; once a live bridge host exists, setup uses the public browser_debugger.reload_bridge facade for code reconvergence."
    }

    # Same extension ID already loaded, enabled, and fully permissioned, but registered
    # from a directory other than the current stable dir (e.g. a pre-#1307 repo-checkout
    # install). The bridge extension ID is derived from the manifest key and is therefore
    # path-independent: a working same-ID host IS the Synapse bridge no matter which
    # directory Chrome loaded it from. Chrome cannot relocate an already-loaded unpacked
    # extension in place — the only ways to change its load directory are re-running
    # "Load unpacked" (the fragile chrome://extensions UI-Automation path) or a Chrome
    # relaunch, and there is no supported in-place mechanism for an off-Web-Store
    # extension. Driving UI Automation to "re-install" a bridge that is already loaded and
    # healthy is precisely the failure mode that broke the daemon handoff (see #1313), so
    # we refuse to do it. The stable directory has already been deployed for future
    # first-time installs; the live daemon reconverges the extension's code in place via
    # the public browser_debugger.reload_bridge facade after handoff, and the
    # post-handoff /health check is the live
    # source of truth that fails loud if the bridge is not actually serving. We surface
    # the pending path migration in the readback so health and operators can see that the
    # bridge is still loaded from a non-stable directory.
    if ($before.installed -and $before.enabled_and_permissioned -and $before.manifest_dir_exists) {
        if ($ReloadExistingExtensionViaUi) {
            return Invoke-SynapseChromeBridgeExistingUiReload `
                -ChromeUserDataRoot $ChromeUserDataRoot `
                -ExtensionId $ExtensionId `
                -ExtensionDir $ExtensionDir `
                -ActiveProfile $activeProfile `
                -Before $before `
                -TimeoutSeconds $TimeoutSeconds
        }
        return [pscustomobject]@{
            attempted = $true
            changed = $false
            reason = 'existing_ready_extension_nonstable_path_code_reload_deferred_to_daemon'
            active_profile = $activeProfile
            required_foreground = $false
            bridge_self_reload_command = 'browser_debugger.reload_bridge'
            path_migration_pending = $true
            loaded_manifest_path = $before.manifest_path
            expected_stable_path = $ExtensionDir
            before = $before
            after = $before
        }
    }

    Initialize-SynapseChromeBridgeAutoInstallInterop
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $candidateWindows = @(Get-SynapseChromeTopLevelWindows -ChromeUserDataRoot $ChromeUserDataRoot | Where-Object {
        $_.title -notmatch '^Select the extension directory\.?$'
    })
    $windows = @($candidateWindows | Where-Object { $_.chrome_profile_eligible -eq $true })
    if ($windows.Count -eq 0) {
        $rejected = Format-SynapseChromeWindowRejectSummary -Windows $candidateWindows
        throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_NO_ELIGIBLE_CHROME_WINDOW active_profile=$activeProfile user_data_root=$ChromeUserDataRoot rejected_windows=$rejected remediation=open the already-authenticated Chrome profile for this user-data root; setup refuses to drive ms-playwright-mcp, remote-debugging, unreadable, or other dedicated Chrome user-data-dir windows"
    }
    $chromeWindow = @($windows | Sort-Object @{ Expression = 'is_foreground'; Descending = $true }, @{ Expression = 'title'; Descending = $false } | Select-Object -First 1)[0]
    Assert-SynapseHumanAllowsForegroundUiRepair -Stage 'first_install_ui'
    [SynapseChromeBridgeAutoInstall.Win32]::ShowWindowAsync([IntPtr]$chromeWindow.hwnd, 5) | Out-Null
    [SynapseChromeBridgeAutoInstall.Win32]::SetForegroundWindow([IntPtr]$chromeWindow.hwnd) | Out-Null
    Start-Sleep -Milliseconds 300

    $previousClipboardText = $null
    $restoreClipboard = $false
    try {
        try {
            $previousClipboardText = Get-Clipboard -Raw -ErrorAction Stop
            $restoreClipboard = $true
        } catch {
            $restoreClipboard = $false
        }
        Set-Clipboard -Value $script:BrowserExtensionsPageUrl
        Send-SynapseNativeKeyTap -VirtualKey 0x1B
        Start-Sleep -Milliseconds 200
        Send-SynapseNativeKeyChord -VirtualKeys ([byte[]](0x11, 0x4C))
        Start-Sleep -Milliseconds 250
        Send-SynapseNativeKeyChord -VirtualKeys ([byte[]](0x11, 0x56))
        Start-Sleep -Milliseconds 250
        Send-SynapseNativeKeyTap -VirtualKey 0x0D

        $loadUnpacked = Wait-SynapseUntil -Deadline $deadline -Probe {
            $currentWindow = @(Get-SynapseChromeTopLevelWindows -ChromeUserDataRoot $ChromeUserDataRoot | Where-Object {
                $_.hwnd -eq $chromeWindow.hwnd
            } | Select-Object -First 1)
            if ($currentWindow.Count -eq 0) {
                return $null
            }
            if ($currentWindow[0].title -notmatch $script:BrowserExtensionsWindowTitlePattern) {
                return $null
            }
            $button = Find-SynapseAutomationElementByName `
                -Root $currentWindow[0].element `
                -Name $script:BrowserInstallUiLoadUnpacked `
                -ControlType ([System.Windows.Automation.ControlType]::Button)
            if ($button) {
                return [pscustomobject]@{ window = $currentWindow[0]; button = $button }
            }
            $developerMode = Find-SynapseAutomationElementByName `
                -Root $currentWindow[0].element `
                -Name $script:BrowserInstallUiDeveloperMode `
                -ControlType $null
            if ($developerMode) {
                try {
                    Invoke-SynapseAutomationElement -Element $developerMode -Description 'Developer mode'
                } catch { }
            }
            return $null
        }
        if (-not $loadUnpacked) {
            $latestWindow = Get-SynapseChromeWindowByHwnd -Hwnd $chromeWindow.hwnd
            $latestTitle = if ($latestWindow) { [string]$latestWindow.title } else { '<missing>' }
            throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_LOAD_UNPACKED_NOT_FOUND active_profile=$activeProfile timeout_s=$TimeoutSeconds chrome_window_hwnd=$($chromeWindow.hwnd) chrome_window_pid=$($chromeWindow.pid) latest_title=$latestTitle user_data_dir=$($chromeWindow.chrome_user_data_dir) match_reason=$($chromeWindow.chrome_profile_match_reason) remediation=Chrome did not confirm chrome://extensions and expose the Load unpacked button in the selected active-profile window; setup refuses to hunt controls in any other Chrome window"
        }
        Invoke-SynapseAutomationElement -Element $loadUnpacked.button -Description 'Load unpacked'

        $dialog = Wait-SynapseUntil -Deadline $deadline -Probe {
            Find-SynapseChromeFolderDialog -ChromeWindowHwnd $chromeWindow.hwnd -ChromeWindowPid $chromeWindow.pid
        }
        if (-not $dialog) {
            throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_FOLDER_DIALOG_NOT_FOUND active_profile=$activeProfile timeout_s=$TimeoutSeconds remediation=Chrome did not open the folder picker after Load unpacked"
        }
        [SynapseChromeBridgeAutoInstall.Win32]::SetForegroundWindow([IntPtr]$dialog.hwnd) | Out-Null
        Start-Sleep -Milliseconds 200
        $folderEdit = Find-SynapseAutomationElementByName `
            -Root $dialog.element `
            -Name 'Folder:' `
            -ControlType ([System.Windows.Automation.ControlType]::Edit)
        if (-not $folderEdit) {
            throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_FOLDER_FIELD_NOT_FOUND active_profile=$activeProfile dialog_hwnd=$($dialog.hwnd) remediation=folder picker did not expose the Folder field through UI Automation"
        }
        Set-SynapseAutomationEditValue -Element $folderEdit -Value $ExtensionDir -Description 'Folder:'
        Start-Sleep -Milliseconds 200
        $selectFolder = Find-SynapseAutomationElementByName `
            -Root $dialog.element `
            -Name 'Select Folder' `
            -ControlType ([System.Windows.Automation.ControlType]::Button)
        if (-not $selectFolder) {
            throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_SELECT_FOLDER_NOT_FOUND active_profile=$activeProfile dialog_hwnd=$($dialog.hwnd) remediation=folder picker did not expose Select Folder through UI Automation"
        }
        Invoke-SynapseAutomationElement -Element $selectFolder -Description 'Select Folder'

        $after = Wait-SynapseUntil -Deadline $deadline -Probe {
            $row = Test-SynapseChromeBridgeProfileRow `
                -ChromeUserDataRoot $ChromeUserDataRoot `
                -ProfileName $activeProfile `
                -ExtensionId $ExtensionId `
                -ExtensionDir $ExtensionDir
            if ($row.installed -and $row.manifest_path_matches) {
                return $row
            }
            return $null
        }
        if (-not $after) {
            $latest = Test-SynapseChromeBridgeProfileRow `
                -ChromeUserDataRoot $ChromeUserDataRoot `
                -ProfileName $activeProfile `
                -ExtensionId $ExtensionId `
                -ExtensionDir $ExtensionDir
            throw "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_PROFILE_ROW_MISSING active_profile=$activeProfile timeout_s=$TimeoutSeconds installed=$($latest.installed) manifest_path=$($latest.manifest_path) expected_path=$ExtensionDir remediation=Chrome did not persist the Synapse unpacked extension row after Select Folder"
        }
        return [pscustomobject]@{
            attempted = $true
            changed = $true
            reason = 'installed_unpacked_extension_in_active_profile'
            active_profile = $activeProfile
            chrome_window_hwnd = $chromeWindow.hwnd
            chrome_window_pid = $chromeWindow.pid
            chrome_window_user_data_dir = $chromeWindow.chrome_user_data_dir
            chrome_window_profile_match_reason = $chromeWindow.chrome_profile_match_reason
            folder_dialog_hwnd = $dialog.hwnd
            before = $before
            after = $after
        }
    } finally {
        if ($restoreClipboard) {
            try {
                Set-Clipboard -Value $previousClipboardText
            } catch { }
        }
    }
}

$nativeRoot = Join-Path $env:APPDATA 'synapse\chrome-debugger'
New-Item -ItemType Directory -Force -Path $nativeRoot | Out-Null

$hostName = 'com.synapse.chrome_debugger'
$hostManifestPath = Join-Path $nativeRoot "$hostName.json"
$registryPath = "HKCU:\Software\Google\Chrome\NativeMessagingHosts\$hostName"
$nativeHostRegistryCleanup = Remove-SynapseNativeHostRegistryEntries -HostName $hostName
if (Test-Path -LiteralPath $hostManifestPath -PathType Leaf) {
    Remove-Item -LiteralPath $hostManifestPath -Force
}
if (Test-Path -LiteralPath $hostManifestPath -PathType Leaf) {
    throw "SYNAPSE_CHROME_NATIVE_HOST_MANIFEST_REMOVE_FAILED path=$hostManifestPath remediation=normal bridge must use direct localhost WebSocket command delivery only"
}

$chromeProcesses = @(Get-CimInstance Win32_Process -Filter $script:BrowserProcessWmiFilter -ErrorAction SilentlyContinue | ForEach-Object {
    $commandLine = [string]$_.CommandLine
    $hasRemoteDebuggingPipe = $commandLine -match '(^|\s)--remote-debugging-pipe(\s|=|$)'
    $hasRemoteDebuggingPort = $commandLine -match '(^|\s)--remote-debugging-port(\s|=|$)'
    $hasSilentDebuggerSwitch = $commandLine -match '(^|\s)--silent-debugger-extension-api(\s|=|$)'
    $hasAutomationControlledFlag = $commandLine -match '(^|\s)--disable-blink-features=([^\s]*,)?AutomationControlled(,|\s|$)'
    $hasMsPlaywrightMcpDir = $commandLine -match 'ms-playwright-mcp'
    $layoutInfobarReasons = @()
    if ($hasAutomationControlledFlag) {
        $layoutInfobarReasons += 'unsupported_flag_disable_blink_features_automation_controlled'
    }
    if (($hasRemoteDebuggingPipe -or $hasRemoteDebuggingPort) -and -not $hasSilentDebuggerSwitch) {
        $layoutInfobarReasons += 'remote_debugging_without_silent_debugger_extension_api'
    }
    if ($hasMsPlaywrightMcpDir -and $hasAutomationControlledFlag) {
        $layoutInfobarReasons += 'headed_ms_playwright_mcp_layout_banner'
    }
    [pscustomobject]@{
        pid = [int]$_.ProcessId
        parent_pid = [int]$_.ParentProcessId
        creation_date = [string]$_.CreationDate
        command_line_readable = -not [string]::IsNullOrWhiteSpace($commandLine)
        has_silent_debugger_switch = $hasSilentDebuggerSwitch
        has_remote_debugging_pipe = $hasRemoteDebuggingPipe
        has_remote_debugging_port = $hasRemoteDebuggingPort
        has_automation_controlled_flag = $hasAutomationControlledFlag
        has_ms_playwright_mcp_dir = $hasMsPlaywrightMcpDir
        layout_infobar_risk = ($layoutInfobarReasons.Count -gt 0)
        layout_infobar_reasons = $layoutInfobarReasons
    }
})

$chromeUserDataRoot = $script:BrowserUserDataRoot
$chromePolicyBootstrapCleanup = @(Remove-SynapseChromeExternalDebuggerPolicy -PreserveExtensionIds @())
$bootstrapFailures = @($chromePolicyBootstrapCleanup | Where-Object { ([string]$_.reason) -match '^(write_failed:|parse_error:)' })
if ($bootstrapFailures.Count -gt 0) {
    throw "SYNAPSE_CHROME_BRIDGE_BOOTSTRAP_POLICY_CLEANUP_FAILED remediation=remove stale Synapse-authored blockers from the configured browser policy before installation; do not weaken registry ACLs"
}
$chromeBridgeAutoInstall = Invoke-SynapseChromeBridgeAutoInstall `
    -ChromeUserDataRoot $chromeUserDataRoot `
    -ExtensionId $ExtensionId `
    -ExtensionDir $extensionDir `
    -TimeoutSeconds $AutoInstallTimeoutSeconds
Set-SynapseChromeBridgeDeferredRepairMarker `
    -AutoInstall $chromeBridgeAutoInstall `
    -MarkerPath (Join-Path $env:APPDATA 'synapse\chrome-bridge-deferred-repair.txt')
$profileDirs = @()
$synapseChromeProfileReadback = @()
$staleSynapseActivePermissions = @()
$staleSynapseGrantedPermissions = @()
$externalDebuggerOrNativeExtensions = @()
$externalDisabledDebuggerOrNativeExtensions = @()
$externalDebuggerExtensions = @()
function Get-ChromeExtensionRuntimeState {
    param(
        [Parameter(Mandatory = $true)]
        $Setting
    )

    $state = $null
    if ($Setting.PSObject.Properties.Name -contains 'state' -and $null -ne $Setting.state) {
        $state = [int]$Setting.state
    }
    $activeBit = $null
    if ($Setting.PSObject.Properties.Name -contains 'active_bit') {
        $activeBit = [bool]$Setting.active_bit
    }
    $disableReasons = @()
    if ($Setting.PSObject.Properties.Name -contains 'disable_reasons' -and $null -ne $Setting.disable_reasons) {
        $disableReasons = @($Setting.disable_reasons)
    }
    # Chromium persists extension state as DISABLED=0, ENABLED=1. Stale
    # permission rows can remain without state; the live chrome.management
    # bridge readback is the stronger authority for enabled hazards.
    $runtimeEnabled = (($state -eq 1) -and ($disableReasons.Count -eq 0))

    [pscustomobject]@{
        state = $state
        active_bit = $activeBit
        disable_reasons = $disableReasons
        runtime_enabled = $runtimeEnabled
    }
}

function Test-ExternalPopupRiskEnabled {
    param(
        [Parameter(Mandatory = $true)]
        [object]$RuntimeState,
        [Parameter(Mandatory = $true)]
        [bool]$HasActiveOrManifestHazard,
        [Parameter(Mandatory = $true)]
        [bool]$HasGrantedHazard
    )

    if ($RuntimeState.disable_reasons.Count -gt 0 -or $RuntimeState.state -eq 0) {
        return $false
    }
    if ($RuntimeState.state -eq 1) {
        return ($HasActiveOrManifestHazard -or $HasGrantedHazard)
    }
    # Stale granted-only residue is advisory, but an external active/manifest
    # debugger permission with no disable reason can still surface Chrome's
    # layout-shifting "started debugging this browser" infobar.
    return $HasActiveOrManifestHazard
}
if (Test-Path -LiteralPath $chromeUserDataRoot -PathType Container) {
    $profileDirs = @(Get-SynapseBrowserProfileDirs -Row $script:BrowserRow -UserDataRoot $chromeUserDataRoot)
    foreach ($profileDir in $profileDirs) {
        $extensionRuntimeById = @{}
        foreach ($prefFileName in @('Preferences', 'Secure Preferences')) {
            $prefPath = Join-Path $profileDir.FullName $prefFileName
            if (-not (Test-Path -LiteralPath $prefPath -PathType Leaf)) {
                continue
            }
            try {
                $pref = Get-Content -Raw -LiteralPath $prefPath | ConvertFrom-Json -ErrorAction Stop
            } catch {
                $synapseChromeProfileReadback += [pscustomobject]@{
                    profile = $profileDir.Name
                    pref_file = $prefFileName
                    path = $prefPath
                    parse_error = $_.Exception.Message
                }
                continue
            }
            if (-not $pref.extensions -or -not $pref.extensions.settings) {
                continue
            }
            foreach ($extensionProperty in $pref.extensions.settings.PSObject.Properties) {
                if (Test-SynapseBrowserProtectedExtensionId -Row $script:BrowserRow -ExtensionId ([string]$extensionProperty.Name)) { continue }
                $setting = $extensionProperty.Value
                $runtimeState = Get-ChromeExtensionRuntimeState -Setting $setting
                if ($prefFileName -eq 'Preferences') {
                    $extensionRuntimeById[$extensionProperty.Name] = $runtimeState
                } elseif ($extensionRuntimeById.ContainsKey($extensionProperty.Name)) {
                    $runtimeState = $extensionRuntimeById[$extensionProperty.Name]
                }
                $activeApi = @()
                if ($setting.active_permissions -and $setting.active_permissions.api) {
                    $activeApi = @($setting.active_permissions.api)
                }
                $grantedApi = @()
                if ($setting.granted_permissions -and $setting.granted_permissions.api) {
                    $grantedApi = @($setting.granted_permissions.api)
                }
                $manifestApi = @()
                if ($setting.manifest -and $setting.manifest.permissions) {
                    $manifestApi = @($setting.manifest.permissions | Where-Object {
                        $_ -is [string]
                    })
                }
                if ($extensionProperty.Name -eq $ExtensionId) {
                    $synapseActiveHazardApi = @(
                        @($activeApi)
                        @($manifestApi)
                    ) | Where-Object {
                        $_ -eq 'nativeMessaging'
                    } | Sort-Object -Unique
                    $synapseGrantedHazardApi = @($grantedApi | Where-Object {
                        $_ -eq 'nativeMessaging'
                    } | Sort-Object -Unique)
                    $row = [pscustomobject]@{
                        profile = $profileDir.Name
                        pref_file = $prefFileName
                        path = $prefPath
                        manifest_path = $setting.path
                        active_api = $activeApi
                        granted_api = $grantedApi
                        manifest_api = $manifestApi
                        active_or_manifest_hazard_api = $synapseActiveHazardApi
                        granted_hazard_api = $synapseGrantedHazardApi
                        state = $runtimeState.state
                        active_bit = $runtimeState.active_bit
                        disable_reasons = $runtimeState.disable_reasons
                        runtime_enabled = $runtimeState.runtime_enabled
                    }
                    $synapseChromeProfileReadback += $row
                    if ($synapseActiveHazardApi.Count -gt 0) {
                        $staleSynapseActivePermissions += $row
                    } elseif ($synapseGrantedHazardApi.Count -gt 0) {
                        $staleSynapseGrantedPermissions += $row
                    }
                } else {
                    $activeOrManifestHazardApi = @(
                        @($activeApi)
                        @($manifestApi)
                    ) | Where-Object {
                        $_ -eq 'debugger' -or $_ -eq 'nativeMessaging'
                    } | Sort-Object -Unique
                    $grantedHazardApi = @($grantedApi | Where-Object {
                        $_ -eq 'debugger' -or $_ -eq 'nativeMessaging'
                    } | Sort-Object -Unique)
                    $hazardApi = @(
                        @($activeOrManifestHazardApi)
                        @($grantedApi)
                    ) | Where-Object {
                        $_ -eq 'debugger' -or $_ -eq 'nativeMessaging'
                    } | Sort-Object -Unique
                    if ($hazardApi.Count -eq 0) {
                        continue
                    }
                    $popupRiskEnabled = Test-ExternalPopupRiskEnabled `
                        -RuntimeState $runtimeState `
                        -HasActiveOrManifestHazard ($activeOrManifestHazardApi.Count -gt 0) `
                        -HasGrantedHazard ($grantedHazardApi.Count -gt 0)
                    $externalRow = [pscustomobject]@{
                        profile = $profileDir.Name
                        pref_file = $prefFileName
                        extension_id = $extensionProperty.Name
                        name = $setting.manifest.name
                        location = $setting.location
                        manifest_path = $setting.path
                        active_api = $activeApi
                        granted_api = $grantedApi
                        manifest_api = $manifestApi
                        active_or_manifest_hazard_api = $activeOrManifestHazardApi
                        granted_hazard_api = $grantedHazardApi
                        hazard_api = $hazardApi
                        state = $runtimeState.state
                        active_bit = $runtimeState.active_bit
                        disable_reasons = $runtimeState.disable_reasons
                        runtime_enabled = $runtimeState.runtime_enabled
                        popup_risk_enabled = $popupRiskEnabled
                        risk_basis = if ($popupRiskEnabled) {
                            if ($runtimeState.state -eq 1) {
                                'state_enabled_hazard'
                            } elseif ($activeOrManifestHazardApi.Count -gt 0) {
                                'active_or_manifest_hazard_without_disable_reason'
                            } else {
                                'state_enabled_granted_hazard'
                            }
                        } else {
                            'disabled_or_granted_only_stale'
                        }
                    }
                    if ($popupRiskEnabled) {
                        $externalDebuggerOrNativeExtensions += $externalRow
                        if ($hazardApi -contains 'debugger') {
                            $externalDebuggerExtensions += $externalRow
                        }
                    } else {
                        $externalDisabledDebuggerOrNativeExtensions += $externalRow
                    }
                }
            }
        }
    }
}
$activeChromeProfile = Get-SynapseActiveChromeProfileName `
    -ChromeUserDataRoot $chromeUserDataRoot `
    -ExtensionId $ExtensionId `
    -ExtensionDir $extensionDir
$synapseChromeInstalledProfiles = @(
    $synapseChromeProfileReadback |
        Where-Object { $_.PSObject.Properties.Name -contains 'manifest_path' } |
        ForEach-Object { [string]$_.profile } |
        Sort-Object -Unique
)
$synapseChromeActiveProfileInstalled = $null
if (-not [string]::IsNullOrWhiteSpace($activeChromeProfile)) {
    $synapseChromeActiveProfileInstalled = @($synapseChromeInstalledProfiles) -contains $activeChromeProfile
}
$synapseChromeProfileInstallReason = if ($profileDirs.Count -eq 0) {
    'no_profile_dirs'
} elseif ($synapseChromeInstalledProfiles.Count -eq 0) {
    'extension_id_absent_from_preferences_and_secure_preferences'
} elseif ($synapseChromeActiveProfileInstalled -eq $false) {
    'active_profile_missing_extension'
} else {
    'extension_profile_row_present'
}
$synapseChromeProfileInstallState = [pscustomobject]@{
    scanned = $true
    installed = ($synapseChromeInstalledProfiles.Count -gt 0)
    auto_install = $chromeBridgeAutoInstall
    chrome_user_data_root = $chromeUserDataRoot
    profile_count = $profileDirs.Count
    installed_profile_count = $synapseChromeInstalledProfiles.Count
    installed_profiles = @($synapseChromeInstalledProfiles)
    active_profile = $activeChromeProfile
    active_profile_installed = $synapseChromeActiveProfileInstalled
    reason = $synapseChromeProfileInstallReason
    cdp_bridge_reload_can_install_absent_extension = $false
    remediation = 'run scripts\install-synapse-chrome-debugger.ps1 from the interactive Windows desktop with the target Chrome profile already open; the installer deploys the bundled bridge into %LOCALAPPDATA%\synapse\chrome-extension\<build-id> and auto-loads that stable unpacked directory in the active profile. browser_debugger.reload_bridge can only reload an already-registered bridge host and cannot install an absent Chrome extension'
}
$externalNativeMessagingProcesses = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
        $_.CommandLine -match 'chrome\.nativeMessaging' -and
        $_.CommandLine -notmatch [regex]::Escape($ExtensionId)
    } |
    Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine,
        @{Name = 'ExtensionId'; Expression = {
            if ($_.CommandLine -match 'chrome-extension://([a-p]{32})') {
                $Matches[1]
            } else {
                $null
            }
        }})

$externalNativeMessagingProcessRows = @($externalNativeMessagingProcesses | ForEach-Object {
    if ($_.ExtensionId -match '^[a-p]{32}$') {
        [pscustomobject]@{
            profile = 'process'
            pref_file = 'process'
            extension_id = $_.ExtensionId
            name = 'external nativeMessaging process'
            location = $null
            manifest_path = $null
            active_api = @('nativeMessaging')
            active_bit = $null
            disable_reasons = @()
            runtime_enabled = $true
            source = 'native_messaging_process'
        }
    }
})
$externalLayoutInfobarProcesses = @($chromeProcesses | Where-Object {
    $_.layout_infobar_risk
} | ForEach-Object {
    [pscustomobject]@{
        pid = $_.pid
        parent_pid = $_.parent_pid
        reasons = @($_.layout_infobar_reasons)
        has_remote_debugging_pipe = $_.has_remote_debugging_pipe
        has_remote_debugging_port = $_.has_remote_debugging_port
        has_silent_debugger_switch = $_.has_silent_debugger_switch
        has_automation_controlled_flag = $_.has_automation_controlled_flag
        has_ms_playwright_mcp_dir = $_.has_ms_playwright_mcp_dir
    }
})
$allExternalDebuggerOrNativeExtensions = @(
    @($externalDebuggerOrNativeExtensions | ForEach-Object {
        $_ | Add-Member -NotePropertyName source -NotePropertyValue 'chrome_profile_active' -Force -PassThru
    })
    @($externalDisabledDebuggerOrNativeExtensions | ForEach-Object {
        $_ | Add-Member -NotePropertyName source -NotePropertyValue 'chrome_profile_disabled_or_inactive' -Force -PassThru
    })
    @($externalNativeMessagingProcessRows)
)
$externalHazardExtensionIds = @(
    @($allExternalDebuggerOrNativeExtensions | ForEach-Object { $_.extension_id })
    @($externalNativeMessagingProcesses | ForEach-Object { $_.ExtensionId })
) | Where-Object { $_ -match '^[a-p]{32}$' } | Sort-Object -Unique

$synapseSelfShieldRow = [pscustomobject]@{
    extension_id = $ExtensionId
    name = 'Synapse Chrome Bridge'
    active_api = @()
    granted_api = @()
    manifest_api = @()
    hazard_api = @('nativeMessaging')
    runtime_enabled = $true
    source = 'synapse_self_bridge_invariant'
}

# External extensions and native-messaging hosts that use debugger/nativeMessaging
# can surface layout-changing Chrome popups independently of Synapse's tabs-only
# bridge. They belong to the owner: setup reports them and never blocks,
# disables or re-permissions them; the bridge only observes them through
# chrome.management.
#
# As a one-way remediation we remove stale Synapse-authored blockers from prior
# builds, then write the current self-shield for the Synapse extension ID. The
# current bridge intentionally requests debugger for a narrow target-scoped CDP
# input lane, so the self-shield blocks nativeMessaging only.
$chromePolicyCleanup = @(@($chromePolicyBootstrapCleanup); @(Remove-SynapseChromeExternalDebuggerPolicy -PreserveExtensionIds @($externalHazardExtensionIds)))
# Other extensions belong to the owner: setup never writes blockers for them
# (upstream #2265). Only the Synapse self-shield row is written.
$policyShieldExtensions = @($synapseSelfShieldRow)
$chromePolicyPopupShield = Set-SynapseChromeExternalDebuggerPolicy `
    -Extensions $policyShieldExtensions `
    -BridgeExtensionId $ExtensionId `
    -ProtectedExtensionIds @($script:BrowserRow.protected_extension_ids | ForEach-Object { [string]$_ })
# The popup shield writes HKCU\Software\Policies\Google\Chrome\ExtensionSettings — a
# Chrome MANAGED-POLICY key. On a correctly-secured Windows install that key is
# admin-only (owner SYSTEM, standard users get ReadKey) BY DESIGN, precisely so a
# non-admin process cannot grant itself Chrome policy. A non-elevated setup therefore
# cannot write it, and that denial is expected hardening — not a misconfiguration to
# "repair". The shield now covers only Synapse's own nativeMessaging permission,
# which the daemon also gates at runtime; other extensions are only observed.
# Making an admin-only, defense-in-depth policy write a hard
# dependency would brick `synapse-update` on every properly-secured machine. So a
# write-denied shield is surfaced LOUDLY (with exact failure + remediation) but is
# non-fatal; only an UNEXPECTED shield failure (not the known admin-only policy-root
# denial) still aborts setup.
$popupShieldWriteDeniedRows = @($chromePolicyPopupShield | Where-Object {
    [string]$_.warning_code -eq 'SYNAPSE_CHROME_POLICY_POPUP_SHIELD_WRITE_DENIED'
})
$unexpectedBlockingShieldRows = @($chromePolicyPopupShield | Where-Object {
    $_.blocking -eq $true -and [string]$_.warning_code -ne 'SYNAPSE_CHROME_POLICY_POPUP_SHIELD_WRITE_DENIED'
})
if ($unexpectedBlockingShieldRows.Count -gt 0) {
    $detail = ConvertTo-CompressedJson -Value $unexpectedBlockingShieldRows -Depth 10
    throw "SYNAPSE_CHROME_POLICY_POPUP_SHIELD_WRITE_FAILED detail=$detail remediation=the Synapse popup shield failed for an unexpected reason (not the admin-only HKCU managed-policy-root write denial); inspect detail.error/detail.acl and repair before rerunning setup"
}
if ($popupShieldWriteDeniedRows.Count -gt 0) {
    $deniedDetail = ConvertTo-CompressedJson -Value $popupShieldWriteDeniedRows -Depth 10
    Write-Warning "SYNAPSE_CHROME_POLICY_POPUP_SHIELD_WRITE_DENIED_NONBLOCKING the HKCU Chrome managed-policy key is admin-only on this host, so the non-elevated popup shield could not be written. This is not fatal: the self-shield only covers Synapse's own nativeMessaging permission, which the daemon gates separately at runtime; other extensions are never blocked or disabled. To also apply the self-shield, run setup once from an elevated PowerShell. detail=$deniedDetail"
}

if ($staleSynapseActivePermissions.Count -gt 0) {
    $detail = [ordered]@{
        extension_id = $ExtensionId
        stale_active_permissions = $staleSynapseActivePermissions
        chrome_policy_popup_shield = $chromePolicyPopupShield
    } | ConvertTo-Json -Depth 8 -Compress
    throw "SYNAPSE_CHROME_EXTENSION_STALE_ACTIVE_NATIVE_MESSAGING_PERMISSION extension_id=$ExtensionId detail=$detail remediation=Synapse attempted to apply/preserve the HKCU ExtensionSettings self-shield for nativeMessaging and included the physical policy write result in detail.chrome_policy_popup_shield; call browser_debugger.reload_bridge through the real Synapse MCP public facade when the live bridge advertises reloadSelf, otherwise keep normal browser commands failed closed until Chrome reloads the extension or restarts the already-open profile"
}

$bridgeOrigin = "$($script:BrowserRow.extension.origin_scheme)://$ExtensionId/"
# Identity is the running code (#237 stamp: running worker, then the worker on
# disk in the folder Chrome loaded), not the load folder. A stale worker or an
# older build fails closed on extension_code_stamp; the same code loaded from a
# non-standard folder passes with a warning naming that folder (#251 round 3).
$expectedLiveIdentity = New-SynapseBridgeExpectedIdentity -Row $script:BrowserRow -DetailedValues ([ordered]@{
    extension_code_stamp=$extensionDeploy.code_stamp; expected_code_stamp=$extensionDeploy.code_stamp
    extension_id=$ExtensionId; origin=$bridgeOrigin; transport='direct_http'
    extension_build_id=$bridgeBuildId; extension_version=[string]$extensionManifest.version; extension_protocol_version='1'
    extension_service_worker_sha256=$extensionDeploy.service_worker_sha256
    expected_extension_service_worker_sha256=$extensionDeploy.service_worker_sha256
    extension_service_worker_sha256_status='ok'; extension_service_worker_sha256_source="$($script:BrowserRow.extension.origin_scheme)://$ExtensionId/service_worker.js"
    active_profile=$activeChromeProfile; active_profile_installed='true'; tab_control_available='true'; extension_stale='false'
}) -DetailedAdvisory ([ordered]@{
    active_profile_extension_path=$extensionDir; expected_extension_service_worker_path=(Join-Path $extensionDir 'service_worker.js')
}) -RowValues ([ordered]@{
    extension_code_stamp=$extensionDeploy.code_stamp
    browser_kind=[string]$script:BrowserRow.id; extension_browser_kind=[string]$script:BrowserRow.id
    extension_id=$ExtensionId; origin=$bridgeOrigin; transport='direct_http'
    extension_build_id=$bridgeBuildId; tab_control_available='true'
})
# A UI reload registers asynchronously; its caller waits for registration and
# then verifies, so that path defers like -DeferLiveVerification (#251).
$runtimeOutcome = Get-SynapseChromeRuntimeOutcome -AutoInstall $chromeBridgeAutoInstall -Expected $expectedLiveIdentity `
    -DeferLive:($DeferLiveVerification -or $ReloadExistingExtensionViaUi)
$installOutcome = Get-SynapseChromeInstallOutcome -AutoInstall $runtimeOutcome
[pscustomobject]@{
    ok = $installOutcome.ok
    reason = $installOutcome.reason
    runtime_verification = $runtimeOutcome
    native_host = $hostName
    native_manifest = $null
    registry_key = $registryPath
    binary = $null
    extension_id = $ExtensionId
    extension_source_dir = $extensionSourceDir
    extension_dir = $extensionDir
    extension_deploy = $extensionDeploy
    daemon_bridge_transport = 'direct_localhost_websocket'
    daemon_bridge_origin = "$($script:BrowserRow.extension.origin_scheme)://$ExtensionId"
    browser_kind = [string]$script:BrowserRow.id
    bridge_config_path = $bridgeConfigPath
    bridge_self_reload_command = 'browser_debugger.reload_bridge'
    bridge_build_id_expected = $bridgeBuildId
    bridge_declared_build_sha256_expected = $bridgeDeclaredBuildSha256
    bridge_build_sha256_expected = $bridgeDeclaredBuildSha256
    bridge_service_worker_sha256_expected = $extensionDeploy.service_worker_sha256
    bridge_required_capabilities = @('alarmReconnect', 'activateTab', 'ariaSnapshot', 'assertPoll', 'cdpInput', 'evaluateScript', 'initScript', 'exposeBinding', 'handleDialog', 'fileUpload', 'viewportEmulation', 'deviceEmulation', 'geolocationEmulation', 'localeEmulation', 'mediaEmulation', 'networkConditions', 'closeTab', 'clock', 'coordinateClick', 'cookies', 'downloads', 'domAction', 'externalPopupRiskSuppression', 'frameLocators', 'frames', 'inspectElement', 'listTabs', 'locateElements', 'navigateTab', 'openTab', 'pageEvents', 'pageVitals', 'pageContent', 'pageScreenshot', 'pagePdf', 'scrollIntoView', 'setContent', 'storageState', 'waitForFunction', 'waitForLoadState', 'waitForUrl', 'waitForRequest', 'waitForResponse', 'waitForSelector', 'waitForText', 'reloadSelf', 'targetInfo', 'targetInfoPageText', 'typeActiveElement', 'setFieldValue')
    background_navigation_backend = 'chrome.tabs_plus_chrome.scripting_executeScript_plus_chrome.cookies_plus_chrome.downloads_plus_chrome.webNavigation_plus_chrome.webRequest_plus_chrome_tabs_captureVisibleTab_for_typed_dom_actions_storage_cookies_downloads_waits_page_screenshots_and_chrome_debugger_runtime_evaluate_init_scripts_handle_dialog_file_upload_cdp_input_hover_tap_drag_page_print_to_pdf_viewport_emulation_device_emulation_geolocation_emulation_locale_emulation_media_emulation_and_network_conditions_no_native_messaging_plus_chrome.management_external_popup_suppression'
    reconnect_driver = 'bounded_websocket_reconnect_with_chrome_alarms_mv3_wake'
    attach_popup_prevention = 'normal_bridge_debugger_permission_scoped_to_Runtime_evaluate_Page_addScriptToEvaluateOnNewDocument_Runtime_addBinding_Page_handleJavaScriptDialog_DOM_setFileInputFiles_Page_fileChooserOpened_cdpInput_hover_tap_active_drag_pagePdf_printToPDF_viewportEmulation_deviceEmulation_geolocationEmulation_localeEmulation_mediaEmulation_and_networkConditions_inactive_synthetic_drag_no_helper_windows_no_nativeMessaging_permission_plus_external_popup_risk_suppression'
    normal_bridge_attach_commands_available = $true
    normal_bridge_debugger_api_calls_present = $true
    expected_extension_id_guard_present = $true
    required_alarms_permission_present = ($requiredPermissions -contains 'alarms')
    recurring_wakeup_permission_present = ($requiredPermissions -contains 'alarms')
    required_cookies_permission_present = ($requiredPermissions -contains 'cookies')
    required_downloads_permission_present = ($requiredPermissions -contains 'downloads')
    required_debugger_permission_present = ($requiredPermissions -contains 'debugger')
    optional_debugger_permission_present = $false
    required_management_permission_present = ($requiredPermissions -contains 'management')
    required_web_navigation_permission_present = ($requiredPermissions -contains 'webNavigation')
    required_native_messaging_permission_present = $false
    optional_native_messaging_permission_present = $false
    localhost_host_permission_present = $true
    native_host_registry_keys = @((Get-SynapseNativeHostRegistryTargets -HostName $hostName) | ForEach-Object { $_.path })
    native_host_registry_cleanup = $nativeHostRegistryCleanup
    native_host_registry_present = ($nativeHostRegistryCleanup.after.Count -gt 0)
    native_host_manifest_present = (Test-Path -LiteralPath $hostManifestPath)
    silent_debugger_switch_required_for_attach_commands = $false
    silent_debugger_switch = $null
    current_chrome_processes = $chromeProcesses
    chrome_policy_cleanup = $chromePolicyCleanup
    chrome_policy_popup_shield = $chromePolicyPopupShield
    external_popup_risk_blocks_popup_free_commands = $false
    external_popup_risk_scope = 'diagnostic_only_other_extensions_preserved'
    external_popup_risk_block_reason = 'none'
    external_popup_risk_bridge_management_required = ($allExternalDebuggerOrNativeExtensions.Count -gt 0)
    external_popup_risk_bridge_management_permission_present = ($requiredPermissions -contains 'management')
    synapse_chrome_auto_install = $chromeBridgeAutoInstall
    synapse_chrome_profile_install_state = $synapseChromeProfileInstallState
    synapse_chrome_profile_readback = $synapseChromeProfileReadback
    synapse_stale_granted_permission_warning = [pscustomobject]@{
        warning = ($staleSynapseGrantedPermissions.Count -gt 0)
        scope = 'profile_granted_permissions_only_not_runtime_active'
        rows = $staleSynapseGrantedPermissions
    }
    external_hazard_extension_ids = $externalHazardExtensionIds
    external_debugger_or_native_extensions = $externalDebuggerOrNativeExtensions
    external_disabled_debugger_or_native_extensions = $externalDisabledDebuggerOrNativeExtensions
    external_debugger_extensions = $externalDebuggerExtensions
    external_native_messaging_processes = $externalNativeMessagingProcesses
    external_layout_infobar_processes = $externalLayoutInfobarProcesses
}
