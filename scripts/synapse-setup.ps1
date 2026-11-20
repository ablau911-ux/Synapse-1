<#
.SYNOPSIS
  Windows-side Synapse setup: build/install the daemon binary, deploy bundled
  profiles, generate the bearer token, register the auto-start HTTP daemon, and
  (optionally) wire the Windows-side MCP clients. Idempotent and fail-loud.

.DESCRIPTION
  Synapse has exactly ONE controlling body: the Windows-native synapse-mcp.exe
  HTTP daemon. It is the only process that can do real Win32 SendInput / UI
  Automation / WGC-DXGI capture, and it controls BOTH Windows programs (native
  windows) and WSL programs (WSLg GUI apps render as real Windows windows;
  act_run_shell / act_launch reach WSL CLIs via wsl.exe). Every MCP client — on
  Windows or in WSL — connects to this one daemon.

  This script makes that body exist and run, then points the Windows-side
  clients at it. The WSL-side entry (scripts/synapse-install.sh) calls this same
  script through interop and then wires the WSL-side clients.

  Robustness decisions baked in here (learned the hard way):
    * Build from the LOCAL source path (cd into -SourceDir). Building over a
      \\wsl.localhost / pushd-mapped drive bakes transient Z:\ paths into the
      binary (CARGO_MANIFEST_DIR) and intermittently fails cargo's dep-info
      step. -SourceDir must be a real local path.
    * Deploy the bundled profiles NEXT TO the installed exe so the daemon's
      executable-relative profile lookup always resolves, and ALSO pass
      --profile-dir explicitly. A compile-time CARGO_MANIFEST_DIR profile path
      never exists on an installed host.
    * Use a persistent CARGO_TARGET_DIR so re-installs are incremental, not a
      ~25-minute RocksDB rebuild every time.

  Nothing here silently falls back: every prerequisite is checked and throws a
  clear error naming exactly what failed and how to fix it.

.PARAMETER SourceDir
  Path to a LOCAL synapse source checkout to build from. Required unless
  -SkipBuild is set. Must be on a real local drive (not \\wsl.localhost or a
  pushd-mapped UNC drive).

.PARAMETER SkipBuild
  Do not build; validate and install CargoTarget\release\synapse-mcp.exe.
  Fails before daemon handoff when that release artifact is absent.

.PARAMETER BuildTimeoutMinutes
  Maximum time to allow the release build to run. The build process tree is
  launched inside a Windows Job Object with kill-on-close, so Cargo/rustc
  children cannot survive if this setup process exits or is killed.

.PARAMETER ForceRestart
  Permit setup/remove to stop the shared daemon even when active HTTP MCP
  sessions, live client TCP connections, or bridge children are present. Without
  this explicit maintenance flag, setup fails closed instead of interrupting
  another agent. The normal stop path is authenticated graceful shutdown; this
  flag also permits an exact-PID forced stop only after the graceful path fails
  for a verified legacy or unresponsive synapse-mcp.exe process.

.PARAMETER Bind
  Loopback address the daemon binds. Default 127.0.0.1:7700.

.PARAMETER ChromeNativeHostExePath
  Legacy diagnostic native-host path. The normal end-user Chrome bridge uses
  direct localhost HTTP registration plus WebSocket command delivery; setup
  does not install or launch native messaging because Chrome may create a
  visible cmd.exe wrapper for native hosts.

  Synapse applies a reversible HKCU Chrome ExtensionSettings popup shield for
  external debugger/nativeMessaging hazards by default, identified by a
  Synapse-authored blocked_install_message marker. It also preserves a
  self-shield for the stable Synapse extension ID so an older loaded bridge
  build cannot retain nativeMessaging capability. Background automation is
  still achieved on Synapse's own side: the bundled bridge uses direct
  localhost WebSocket, never nativeMessaging/helper Chrome windows, and exposes
  narrow chrome.debugger lanes for target-scoped hover/tap/active-tab drag,
  dialog handling, and viewport/device/geolocation/locale/media/network
  emulation plus inactive-tab synthetic mouse drag and HTML5 DataTransfer drag
  dispatch in the already-open authenticated Chrome profile.

  A correctly hardened host can make
  HKCU:\Software\Policies\Google\Chrome an admin-only managed-policy root even
  for users who are local administrators but currently run under a medium
  integrity UAC token. Non-elevated setup must not weaken that ACL. If the
  policy-level defense-in-depth is required, run this setup script from an
  elevated PowerShell and verify /health reports
  synapse_chrome_self_policy_shield_present=true. If elevation is not used,
  /health must still prove live chrome.management suppression is clear, and
  normal browser commands fail closed if suppression is not confirmed.

.PARAMETER MaintenanceLockPath
  File-lock Source of Truth that serializes setup/remove across multiple
  agents. The file contents name the owning PID and cleanup policy; the held
  FileStream is the actual lock and is released by Windows when setup exits.

.PARAMETER WireClients
  Wire the Windows-side MCP clients (Claude Code and Codex via HTTP, Claude
  Desktop via the connect bridge). Default $true.

.PARAMETER Remove
  Uninstall: stop + unregister the scheduled task. Leaves the DB, token, and
  binary in place unless -Purge is also given.

.PARAMETER Purge
  With -Remove, also delete the daemon DB, deployed profiles, and token.
  Only explicit Synapse-owned paths are deleted (the token FILE, DbPath and
  ProfilesDir when inside Synapse roots, and a named list of files in
  %APPDATA%\synapse); anything else is refused. The exact list is printed first.

.PARAMETER Force
  With -Remove -Purge, delete without the interactive confirmation prompt.

.PARAMETER ActiveIssue
  Optional current GitHub issue number/ref to preserve in Codex restart
  handoffs. Defaults to SYNAPSE_ACTIVE_ISSUE when set. Accepts 1441, #1441, or
  a ChrisRoyse/Synapse issue URL.

.PARAMETER RefreshToolSurfaceOnly
  Refresh the Codex tool-surface snapshot from the already-running daemon,
  without building, restarting, or changing client configuration.

.PARAMETER CodexStartSnapshotRetentionHours
  Age limit for legacy Codex start snapshots (names from before #259 that carry no
  owner PID) in %LOCALAPPDATA%\synapse\codex-start-snapshots. Setup persists it as the
  Windows User variable SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS, which the
  patched Codex launchers (and the WSL loader) read.

.PARAMETER CodexStartSnapshotGraceMinutes
  Minimum age before a launcher may prune another start's o<pid> snapshot whose
  owner process a successful lookup shows is gone (covers PID reuse and races).
  Persisted as the Windows User variable SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES.
  Each launcher deletes its own snapshot when Codex exits; a snapshot whose owner
  is alive, unknown, or not provably gone is never pruned.
#>
[CmdletBinding()]
param(
    [string]$SourceDir,
    [switch]$SkipBuild,
    [string]$Bind        = '127.0.0.1:7700',
    [string]$ExePath     = "$env:USERPROFILE\.cargo\bin\synapse-mcp.exe",
    [string]$ChromeNativeHostExePath = "$env:USERPROFILE\.cargo\bin\synapse-chrome-native-host.exe",
    [string]$CargoTarget = "$env:LOCALAPPDATA\synapse\build-target",
    [string]$DbPath      = "$env:LOCALAPPDATA\synapse\db-daemon",
    [string]$ProfilesDir = "$env:USERPROFILE\.cargo\bin\profiles",
    [string]$LogDir      = "$env:LOCALAPPDATA\synapse\logs",
    [string]$TokenPath   = "$env:APPDATA\synapse\token.txt",
    [string]$CodexToolSurfaceSnapshotPath = "$env:APPDATA\synapse\codex-tool-surface.json",
    [string]$ActiveIssue = $env:SYNAPSE_ACTIVE_ISSUE,
    [string]$TaskName    = 'SynapseMcpDaemon',
    [string]$MaintenanceLockPath = "$env:LOCALAPPDATA\synapse\setup-maintenance.lock.json",
    [ValidateRange(1, 1440)][int]$BuildTimeoutMinutes = 90,
    [ValidateRange(1, 87600)][int]$CodexStartSnapshotRetentionHours = 168,
    [ValidateRange(1, 1440)][int]$CodexStartSnapshotGraceMinutes = 10,
    [int]$PostExitParentPid = 0,
    [string]$PostExitContinuationReason = '',
    [string]$PostExitManifestPath = '',
    [switch]$PostExitUseInstalledBinary,
    [string]$PostExitInstalledSha256 = '',
    [switch]$ForceRestart,
    [switch]$SkipClientWiring,
    [switch]$RefreshToolSurfaceOnly,
    [switch]$Remove,
    [switch]$Purge,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
# Multi-browser bridge: browser rows (crates\synapse-browsers\browsers\*.json)
# are the single source for browser ids, wire ids and paths. The legacy row is
# the mandatory bridge; every other row is best-effort (not_installed = skip).
. (Join-Path $PSScriptRoot 'synapse-browser-rows.ps1')
# #251: live bridge identity readback shared with the bridge installer.
. (Join-Path $PSScriptRoot 'synapse-bridge-live-identity.ps1')
# Windowless scheduled-task actions (wscript.exe + run-hidden.vbs).
. (Join-Path $PSScriptRoot 'synapse-hidden-runner.ps1')
$SynapseLegacyBrowserId = 'chrome'
$SynapseChromeBridgeMaintenancePauseMs = 720000
# #140: /health has measured 0.3-0.9 s steady state and >4 s under deploy load.
$SynapseChromeBridgeMaintenancePauseHealthTimeoutSec = 20
# #140: a degraded /health (3 s daemon bound hit) is retried this many extra
# times, then the pause is skipped with a warning instead of aborting setup.
$SynapseChromeBridgeMaintenancePauseDegradedRetries = 5
$SynapseChromeBridgeMaintenancePauseDegradedRetryDelaySec = 3
$SynapseChromeBridgeMaintenanceCloseDrainMs = 7000
$SynapseChromeBridgeReconnectAlarmCushionMs = 45000
$SynapseChromeBridgeDefaultPostStartWaitMs = 30000
$SynapseChromeBridgeMaintenancePauseGuardMs = 60000
$SynapseChromeBridgeMaxPostStartWaitMs = $SynapseChromeBridgeMaintenancePauseMs + $SynapseChromeBridgeReconnectAlarmCushionMs + $SynapseChromeBridgeDefaultPostStartWaitMs
$SynapseChromeBridgeUiReloadDeferredReason = 'existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon'
$SynapseBindFinalDeadOwnerSettleSeconds = 15
$script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = $null
$script:SynapseBindPostExitContinuationRequired = $false
$script:SynapseBindPostExitContinuationDetail = $null
$script:SynapsePostExitStartOnly = ($PostExitParentPid -gt 0 -and $PostExitContinuationReason -eq 'dead_owner_bind_after_install')
$script:SynapseSetupRepairManifestPath = $env:SYNAPSE_SETUP_REPAIR_MANIFEST
function Write-SynapsePostExitManifestState {
    param(
        [Parameter(Mandatory=$true)][string]$State,
        [string]$Message = '',
        [int]$ExitCode = 0,
        [AllowNull()]$Readback
    )

    if (-not $script:SynapsePostExitStartOnly) {
        return
    }
    if ([string]::IsNullOrWhiteSpace($PostExitManifestPath)) {
        throw "SYNAPSE_POST_EXIT_MANIFEST_PATH_MISSING reason=$PostExitContinuationReason remediation=post-exit continuation must receive -PostExitManifestPath so completion/failure state is physically recorded"
    }
    if (-not (Test-Path -LiteralPath $PostExitManifestPath -PathType Leaf)) {
        throw "SYNAPSE_POST_EXIT_MANIFEST_MISSING path=$PostExitManifestPath remediation=inspect the continuation launcher arguments and rerun setup; completion/failure cannot be accepted without manifest readback"
    }

    $manifest = Get-Content -Raw -LiteralPath $PostExitManifestPath | ConvertFrom-Json
    $now = (Get-Date).ToUniversalTime().ToString('o')
    $manifest | Add-Member -NotePropertyName state -NotePropertyValue $State -Force
    $manifest | Add-Member -NotePropertyName exit_code -NotePropertyValue $ExitCode -Force
    $manifest | Add-Member -NotePropertyName updated_at_utc -NotePropertyValue $now -Force
    if ($State -eq 'completed') {
        $manifest | Add-Member -NotePropertyName completed_at_utc -NotePropertyValue $now -Force
        $manifest | Add-Member -NotePropertyName failure -NotePropertyValue $null -Force
    } elseif ($State -eq 'failed') {
        $manifest | Add-Member -NotePropertyName failed_at_utc -NotePropertyValue $now -Force
        $manifest | Add-Member -NotePropertyName failure -NotePropertyValue ([ordered]@{
            message = $Message
            remediation = 'inspect stdout/stderr/readback fields and rerun setup after fixing the named fatal condition'
        }) -Force
    }
    if (-not [string]::IsNullOrWhiteSpace($Message)) {
        $manifest | Add-Member -NotePropertyName message -NotePropertyValue $Message -Force
    }
    if ($null -ne $Readback) {
        $manifest | Add-Member -NotePropertyName readback -NotePropertyValue $Readback -Force
    }
    $json = ($manifest | ConvertTo-Json -Depth 40) + "`n"
    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($PostExitManifestPath, $json, $encoding)
}

function Write-SynapseSetupRepairManifestState {
    param(
        [Parameter(Mandatory=$true)][ValidateSet('completed','failed','handoff_started')][string]$State,
        [string]$Message = '',
        [int]$ExitCode = 0,
        [AllowNull()]$Readback,
        [string]$ContinuationManifestPath = ''
    )

    if ([string]::IsNullOrWhiteSpace($script:SynapseSetupRepairManifestPath)) {
        return
    }
    if (-not (Test-Path -LiteralPath $script:SynapseSetupRepairManifestPath -PathType Leaf)) {
        throw "SYNAPSE_SETUP_REPAIR_MANIFEST_MISSING path=$script:SynapseSetupRepairManifestPath remediation=external setup repair must stamp completion/failure in the parent manifest; inspect launch env SYNAPSE_SETUP_REPAIR_MANIFEST"
    }

    $manifest = Get-Content -Raw -LiteralPath $script:SynapseSetupRepairManifestPath | ConvertFrom-Json
    $now = (Get-Date).ToUniversalTime().ToString('o')
    $manifest | Add-Member -NotePropertyName state -NotePropertyValue $State -Force
    $manifest | Add-Member -NotePropertyName exit_code -NotePropertyValue $ExitCode -Force
    $manifest | Add-Member -NotePropertyName updated_at_utc -NotePropertyValue $now -Force
    if ($State -eq 'completed') {
        $manifest | Add-Member -NotePropertyName completed_at_utc -NotePropertyValue $now -Force
        $manifest | Add-Member -NotePropertyName failure -NotePropertyValue $null -Force
    } elseif ($State -eq 'failed') {
        $manifest | Add-Member -NotePropertyName failed_at_utc -NotePropertyValue $now -Force
        $manifest | Add-Member -NotePropertyName failure -NotePropertyValue ([ordered]@{
            message = $Message
            remediation = 'inspect stdout/stderr/readback fields and rerun setup repair after fixing the named fatal condition'
        }) -Force
    } elseif ($State -eq 'handoff_started') {
        $manifest | Add-Member -NotePropertyName handoff_started_at_utc -NotePropertyValue $now -Force
        $manifest | Add-Member -NotePropertyName failure -NotePropertyValue $null -Force
        if (-not [string]::IsNullOrWhiteSpace($ContinuationManifestPath)) {
            $manifest | Add-Member -NotePropertyName continuation_manifest_path -NotePropertyValue $ContinuationManifestPath -Force
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($Message)) {
        $manifest | Add-Member -NotePropertyName message -NotePropertyValue $Message -Force
    }
    if ($null -ne $Readback) {
        $manifest | Add-Member -NotePropertyName readback -NotePropertyValue $Readback -Force
    }
    $json = ($manifest | ConvertTo-Json -Depth 40) + "`n"
    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($script:SynapseSetupRepairManifestPath, $json, $encoding)
}

function Info($m)  { Write-Host "[synapse-setup] $m" }
function Step($m)  { Write-Host "`n=== $m ===" -ForegroundColor Cyan }
function Die($m)   {
    if (-not [string]::IsNullOrWhiteSpace($script:SynapseSetupRepairManifestPath)) {
        $state = 'failed'
        $continuationManifestPath = ''
        if ($m -match 'SYNAPSE_BIND_POST_EXIT_CONTINUATION_STARTED') {
            $state = 'handoff_started'
            if ($m -match 'manifest=([^ ]+)') {
                $continuationManifestPath = $Matches[1]
            }
        }
        try {
            Write-SynapseSetupRepairManifestState `
                -State $state `
                -Message $m `
                -ExitCode 1 `
                -Readback $null `
                -ContinuationManifestPath $continuationManifestPath
        } catch {
            throw "[synapse-setup] FATAL: $m ; SYNAPSE_SETUP_REPAIR_MANIFEST_FAILURE_WRITE_FAILED path=$script:SynapseSetupRepairManifestPath error=$($_.Exception.Message)"
        }
    }
    if ($script:SynapsePostExitStartOnly) {
        try {
            Write-SynapsePostExitManifestState -State 'failed' -Message $m -ExitCode 1 -Readback $null
        } catch {
            throw "[synapse-setup] FATAL: $m ; SYNAPSE_POST_EXIT_MANIFEST_FAILURE_WRITE_FAILED path=$PostExitManifestPath error=$($_.Exception.Message)"
        }
    }
    throw "[synapse-setup] FATAL: $m"
}

function Resolve-SynapseSetupFatalTrapPlan {
    # #377: what the top-level trap does with a FATAL. Pure: no I/O.
    param(
        [string]$RepairManifestState = '',
        [bool]$PostExitStartOnly = $false,
        [string]$PostExitManifestState = '',
        [int]$LastExitCode = 0
    )
    # A FATAL always ends the process with 1, whatever the last native command
    # returned (`break` under pwsh -File exited with that code, often 0). A repair
    # manifest already handed off stays handed off; a post-exit continuation records
    # 'failed' unless Die already did.
    return [pscustomobject]@{
        ExitCode = 1
        WriteRepairFailed = ($RepairManifestState -ne 'handoff_started')
        WritePostExitFailed = ($PostExitStartOnly -and $PostExitManifestState -ne 'failed')
        LockState = 'failed'
    }
}

function Get-SynapseUnixTimeMilliseconds {
    return [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
}

function Assert-SynapseChromeBridgeMaintenancePauseBudget {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Phase
    )

    if ($null -eq $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs) {
        return
    }

    $nowMs = Get-SynapseUnixTimeMilliseconds
    $remainingMs = [int64]$script:SynapseChromeBridgeMaintenancePauseUntilUnixMs - [int64]$nowMs
    if ($remainingMs -le [int64]$SynapseChromeBridgeMaintenancePauseGuardMs) {
        Die ("SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_EXPIRING reason={0} bind={1} phase={2} pause_until_unix_ms={3} remaining_ms={4} guard_ms={5} remediation=setup refuses to continue daemon bind drain after the Chrome bridge maintenance pause is close to expiry. The Chrome bridge can reconnect and recreate NetworkService peers after this point; rerun setup with a maintenance pause that covers the full drain budget or investigate why Windows still reports stale dead-owner TCP rows." -f `
            $Reason,
            $Bind,
            $Phase,
            $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs,
            $remainingMs,
            $SynapseChromeBridgeMaintenancePauseGuardMs)
    }
}

function Invoke-SynapseChromeBridgeVerifier {
    param(
        [Parameter(Mandatory = $true)]
        [string]$InstallerPath,
        [Parameter(Mandatory = $true)]
        [string]$NativeHostExePath
    )

    if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
        Die "SYNAPSE_CHROME_BRIDGE_INSTALLER_MISSING path=$InstallerPath remediation=setup requires the repo script that verifies the direct localhost Chrome bridge and removes stale nativeMessaging registration"
    }
    # Both callers run before the new daemon starts (preflight, and after the
    # old daemon was drained), so the live identity check is deferred to
    # Assert-SynapseBridgeLiveIdentityAfterSetup once the daemon is up (#251).
    $chromeBridgeArgs = @{
        SynapseNativeHostExe = $NativeHostExePath
        DeferLiveVerification = $true
    }
    $readback = & $InstallerPath @chromeBridgeArgs
    $runtimeVerification = $readback.runtime_verification
    if (-not $readback.ok) {
        if ($runtimeVerification.deferred -ne $true -or $null -eq $runtimeVerification.expected_identity) {
            Die "SYNAPSE_CHROME_BRIDGE_INSTALLER_FAILED path=$InstallerPath reason=$($readback.reason) remediation=installer did not return ok=true and did not defer the live bridge identity check to setup's post-start verification"
        }
        Info "Chrome bridge live identity check deferred until the daemon is running reason=$($readback.reason)"
    }
    $autoInstall = $readback.synapse_chrome_auto_install
    if (-not $autoInstall) {
        Die "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_READBACK_MISSING path=$InstallerPath remediation=setup requires the bridge installer to report synapse_chrome_auto_install so skipped or failed active-profile installation cannot pass silently"
    }
    if ([string]$autoInstall.reason -eq 'skip_auto_install_requested') {
        Die "SYNAPSE_CHROME_BRIDGE_AUTOINSTALL_SKIPPED path=$InstallerPath remediation=setup must auto-install the bundled Chrome bridge into the already-open active profile; remove -SkipAutoInstall and rerun from the interactive Windows desktop"
    }
    $profileInstallState = $readback.synapse_chrome_profile_install_state
    if (-not $profileInstallState) {
        Die "SYNAPSE_CHROME_BRIDGE_PROFILE_INSTALL_STATE_MISSING path=$InstallerPath remediation=setup requires active Chrome profile installation readback after bridge verification"
    }
    if ($profileInstallState.active_profile_installed -ne $true) {
        Die ("SYNAPSE_CHROME_BRIDGE_ACTIVE_PROFILE_NOT_INSTALLED active_profile={0} installed_profiles={1} auto_install_attempted={2} auto_install_reason={3} remediation=setup must auto-install the deployed stable Synapse Chrome bridge directory into the already-open active Chrome profile before daemon handoff can continue" -f `
            $profileInstallState.active_profile,
            (@($profileInstallState.installed_profiles) -join ','),
            $autoInstall.attempted,
            $autoInstall.reason)
    }
    return $readback
}

function Format-SynapseChromeBridgeProfileInstallState {
    param($Readback)

    $state = $Readback.synapse_chrome_profile_install_state
    if (-not $state) {
        return 'profile_install_state=missing'
    }
    $autoInstall = $Readback.synapse_chrome_auto_install
    $autoInstallAttempted = if ($autoInstall) { [string]$autoInstall.attempted } else { 'missing' }
    $autoInstallReason = if ($autoInstall) { [string]$autoInstall.reason } else { 'missing' }
    $extensionDir = if ($Readback.extension_dir) { [string]$Readback.extension_dir } else { 'missing' }
    return ("profile_install_state=installed:{0},profile_count:{1},installed_profile_count:{2},active_profile:{3},active_profile_installed:{4},reason:{5},auto_install_attempted:{6},auto_install_reason:{7},extension_dir:{8}" -f `
        $state.installed, `
        $state.profile_count, `
        $state.installed_profile_count, `
        $state.active_profile, `
        $state.active_profile_installed, `
        $state.reason, `
        $autoInstallAttempted, `
        $autoInstallReason, `
        $extensionDir)
}

function Invoke-SynapseChromeBridgeUiRepair {
    param(
        [Parameter(Mandatory = $true)]
        [string]$InstallerPath,
        [Parameter(Mandatory = $true)]
        [string]$NativeHostExePath
    )

    if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
        Die "SYNAPSE_CHROME_BRIDGE_INSTALLER_MISSING path=$InstallerPath remediation=setup requires the repo script that can repair the already-installed Chrome bridge through the already-open Chrome profile"
    }
    $readback = & $InstallerPath `
        -SynapseNativeHostExe $NativeHostExePath `
        -ReloadExistingExtensionViaUi `
        -AutoInstallTimeoutSeconds 45
    $autoInstall = $readback.synapse_chrome_auto_install
    if (-not $autoInstall) {
        Die "SYNAPSE_CHROME_BRIDGE_UI_REPAIR_READBACK_MISSING path=$InstallerPath remediation=installer did not return synapse_chrome_auto_install readback for the UI repair path"
    }
    # A UI repair cannot be live-verified yet: the reloaded extension registers
    # asynchronously, so the installer defers (ok=false, deferred=true) and the
    # caller waits for registration before the post-start identity check; the
    # #132 no-eligible-window reason is a deferral too, never a failure (#251).
    $repairDeferred = [string]$autoInstall.reason -eq $SynapseChromeBridgeUiReloadDeferredReason -or
        $readback.runtime_verification.deferred -eq $true
    if (-not $readback.ok -and -not $repairDeferred) {
        Die "SYNAPSE_CHROME_BRIDGE_UI_REPAIR_INSTALLER_FAILED path=$InstallerPath reason=$($readback.reason) remediation=installer neither returned ok=true nor deferred the live bridge identity check after the existing Chrome extension UI repair path"
    }
    $allowedReasons = @(
        'existing_ready_extension_ui_reload_invoked',
        'existing_ready_extension_nonstable_path_ui_reload_invoked',
        'installed_unpacked_extension_in_active_profile'
    )
    if ([string]$autoInstall.reason -eq $SynapseChromeBridgeUiReloadDeferredReason) {
        # No eligible interactive Chrome window exists, so the live extension UI
        # reload cannot run (#132): the installed extension row is unchanged and
        # the daemon reconverges it in place via browser_debugger.reload_bridge
        # once a bridge host registers. This is deferred, not failed.
        $rejectedWindows = if ($null -ne $autoInstall.rejected_windows) { [string]$autoInstall.rejected_windows } else { '<none>' }
        Info ("WARN: Chrome bridge UI repair deferred to daemon reason={0} bridge_self_reload_command={1} rejected_windows={2} remediation={3}" -f `
            [string]$autoInstall.reason,
            [string]$autoInstall.bridge_self_reload_command,
            $rejectedWindows,
            [string]$autoInstall.remediation)
    }
    elseif ($allowedReasons -notcontains [string]$autoInstall.reason) {
        Die "SYNAPSE_CHROME_BRIDGE_UI_REPAIR_NOT_PERFORMED reason=$($autoInstall.reason) attempted=$($autoInstall.attempted) changed=$($autoInstall.changed) remediation=post-start absent-host repair must either invoke the existing extension Reload control or install the bundled unpacked bridge into the already-open active profile"
    }
    $profileInstallState = $readback.synapse_chrome_profile_install_state
    if (-not $profileInstallState -or $profileInstallState.active_profile_installed -ne $true) {
        Die "SYNAPSE_CHROME_BRIDGE_UI_REPAIR_PROFILE_NOT_INSTALLED reason=$($autoInstall.reason) active_profile=$($profileInstallState.active_profile) remediation=post-start UI repair did not leave the active Chrome profile with the bundled Synapse bridge row installed"
    }
    if ([string]$autoInstall.reason -eq $SynapseChromeBridgeUiReloadDeferredReason) {
        return $readback
    }
    $uiBefore = if ($autoInstall.ui_before) { ($autoInstall.ui_before | ConvertTo-Json -Depth 8 -Compress) } else { '<none>' }
    $uiAfter = if ($autoInstall.ui_after) { ($autoInstall.ui_after | ConvertTo-Json -Depth 8 -Compress) } else { '<none>' }
    Info ("Chrome bridge UI repair completed reason={0} active_profile={1} chrome_window_pid={2} chrome_window_hwnd={3} ui_before={4} ui_after={5} {6}" -f `
        $autoInstall.reason,
        $autoInstall.active_profile,
        $autoInstall.chrome_window_pid,
        $autoInstall.chrome_window_hwnd,
        $uiBefore,
        $uiAfter,
        (Format-SynapseChromeBridgeProfileInstallState -Readback $readback))
    return $readback
}

$processTokenAtStart = $env:SYNAPSE_BEARER_TOKEN
$processToolSurfaceHashAtStart = $env:SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START
$processToolSurfaceSnapshotAtStart = $env:SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START
$script:SynapseMcpProtocolVersion = '2025-06-18'
$script:SynapseMcpSessionDeleteTimeoutSec = 20
$script:SynapseSetupMaintenanceLockStream = $null
$script:SynapseSetupMaintenanceLockPath = $null
$script:SynapseSetupMaintenanceLockReason = $null
$script:SynapseSetupOwnedDirectories = @()

function Get-ProcessLineage {
    param([int]$StartPid = $PID)
    $lineage = @()
    $seen = @{}
    $current = $StartPid
    $child = $null
    while ($current -and -not $seen.ContainsKey($current)) {
        $seen[$current] = $true
        $p = Get-CimInstance Win32_Process -Filter "ProcessId=$current" -ErrorAction SilentlyContinue
        if (-not $p) { break }
        # Guard against Windows PID reuse: ParentProcessId is a bare number, so
        # once a real parent exits its PID can be recycled by an unrelated, newer
        # process. A genuine parent always starts no later than its child; if the
        # candidate "parent" started after the child it is a recycled PID, not a
        # true ancestor, so stop the walk rather than climb into a phantom chain
        # (e.g. wininit.exe <- cmd.exe, which falsely tripped the cmd-ancestor guard).
        if ($child -and $p.CreationDate -and $child.CreationDate -and $p.CreationDate -gt $child.CreationDate) {
            break
        }
        $lineage += $p
        $child = $p
        $current = [int]$p.ParentProcessId
    }
    return $lineage
}

function Get-SynapseCurrentCodexAncestor {
    $lineage = Get-ProcessLineage
    return ($lineage | Where-Object {
        $_.Name -ieq 'codex.exe' -or $_.CommandLine -match '@openai[\\/]+codex|codex\.js|codex-win32'
    } | Select-Object -First 1)
}

function Test-SynapseCodexProcess {
    param([AllowNull()]$Process)

    if ($null -eq $Process) {
        return $false
    }
    $name = [string]$Process.Name
    $commandLine = [string]$Process.CommandLine
    return (
        $name -ieq 'codex.exe' -or
        $commandLine -match '@openai[\\/]+codex|codex\.js|codex-win32|openai\.chatgpt'
    )
}

function Read-SynapseSetupMaintenanceLockOwner {
    param([Parameter(Mandatory=$true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return '<missing>'
    }
    try {
        $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        try {
            $reader = New-Object System.IO.StreamReader($stream, [System.Text.Encoding]::UTF8, $true, 4096, $true)
            try {
                $text = $reader.ReadToEnd().Trim()
                if ([string]::IsNullOrWhiteSpace($text)) { return '<empty>' }
                return ($text -replace '\s+', ' ')
            } finally {
                $reader.Dispose()
            }
        } finally {
            $stream.Dispose()
        }
    } catch {
        return "<unreadable error=$($_.Exception.Message)>"
    }
}

# Cross-process guard shared with the stdio bridge (crates/synapse-mcp/src/connect.rs
# SpawnGuard, same name). Setup holds it while acquiring the maintenance lock; the
# bridge holds it across its lock check + daemon spawn, so they cannot interleave (#11).
$script:SynapseSetupSpawnGuardName = 'Global\SynapseSetupMaintenanceSpawnGuard'

function New-SynapseSetupSpawnGuardMutex {
    # Let any authenticated user open it even when setup runs elevated.
    try {
        $sid = New-Object System.Security.Principal.SecurityIdentifier([System.Security.Principal.WellKnownSidType]::AuthenticatedUserSid, $null)
        $rule = New-Object System.Security.AccessControl.MutexAccessRule($sid, 'Synchronize, Modify', 'Allow')
        $security = New-Object System.Security.AccessControl.MutexSecurity
        $security.AddAccessRule($rule)
        $created = $false
        if ($PSVersionTable.PSVersion.Major -ge 6) {
            return [System.Threading.MutexAcl]::Create($false, $script:SynapseSetupSpawnGuardName, [ref]$created, $security)
        }
        return New-Object System.Threading.Mutex($false, $script:SynapseSetupSpawnGuardName, [ref]$created, $security)
    } catch {
        return New-Object System.Threading.Mutex($false, $script:SynapseSetupSpawnGuardName)
    }
}

function Enter-SynapseSetupSpawnGuard {
    param([int]$TimeoutMs = 30000)
    $mutex = New-SynapseSetupSpawnGuardMutex
    $got = $false
    try {
        $got = $mutex.WaitOne($TimeoutMs)
    } catch [System.Threading.AbandonedMutexException] {
        $got = $true
    }
    if (-not $got) {
        $mutex.Dispose()
        Die "SYNAPSE_SETUP_SPAWN_GUARD_TIMEOUT name=$script:SynapseSetupSpawnGuardName timeout_ms=$TimeoutMs remediation=a synapse-mcp bridge is stuck inside its daemon-spawn critical section; inspect synapse-mcp --mode connect processes"
    }
    return ,$mutex
}

function Exit-SynapseSetupSpawnGuard {
    param($Mutex)
    if ($null -eq $Mutex) { return }
    try { $Mutex.ReleaseMutex() } catch { }
    $Mutex.Dispose()
}

function Get-SynapseProcessStartUnix {
    param([Parameter(Mandatory=$true)][int]$ProcessId)
    try {
        return [DateTimeOffset]::new((Get-Process -Id $ProcessId -ErrorAction Stop).StartTime).ToUnixTimeSeconds()
    } catch {
        return $null
    }
}

function Acquire-SynapseSetupMaintenanceLock {
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [Parameter(Mandatory=$true)][string]$Reason
    )

    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    # Hold the spawn guard from before the lock file is opened until its "held"
    # JSON is on disk, so a bridge's check+spawn can never straddle our acquire (#11).
    $spawnGuard = Enter-SynapseSetupSpawnGuard
    try {
    try {
        $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read)
    } catch [System.IO.IOException] {
        $owner = Read-SynapseSetupMaintenanceLockOwner -Path $Path
        Die "SYNAPSE_SETUP_MAINTENANCE_LOCK_HELD reason=$Reason path=$Path owner=$owner remediation=another setup/remove process owns the maintenance lock; wait for that setup process to exit or inspect the named PID. Do not close terminal windows or broad shell processes to clear this condition."
    } catch {
        Die "SYNAPSE_SETUP_MAINTENANCE_LOCK_OPEN_FAILED reason=$Reason path=$Path error=$($_.Exception.Message) remediation=repair permissions on the synapse local appdata directory"
    }

    $script:SynapseSetupMaintenanceLockStream = $stream
    $script:SynapseSetupMaintenanceLockPath = $Path
    $script:SynapseSetupMaintenanceLockReason = $Reason
    $self = Get-CimInstance Win32_Process -Filter "ProcessId=$PID" -ErrorAction SilentlyContinue
    $lineageText = (Get-ProcessLineage | ForEach-Object { "{0}:{1}" -f $_.ProcessId, $_.Name }) -join ' <- '
    $owner = [ordered]@{
        schema = 'synapse_setup_maintenance_lock/v1'
        state = 'held'
        reason = $Reason
        pid = $PID
        pid_start_unix = (Get-SynapseProcessStartUnix -ProcessId $PID)
        parent_pid = $self.ParentProcessId
        process_name = $self.Name
        command_line = $self.CommandLine
        source_dir = $SourceDir
        bind = $Bind
        started_at_utc = (Get-Date).ToUniversalTime().ToString('o')
        lineage = $lineageText
        cleanup_policy = 'never close terminal windows globally; only exact process IDs spawned by this setup operation or verified synapse-mcp targets may be stopped'
    }
    $json = $owner | ConvertTo-Json -Depth 6
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json + "`n")
    $stream.SetLength(0)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush($true)
    } finally {
        Exit-SynapseSetupSpawnGuard -Mutex $spawnGuard
    }
    Info "Maintenance lock acquired reason=$Reason path=$Path pid=$PID"
}

function Release-SynapseSetupMaintenanceLock {
    param(
        [Parameter(Mandatory=$true)][ValidateSet('released','failed')][string]$State,
        [string]$ErrorMessage
    )

    if ($null -eq $script:SynapseSetupMaintenanceLockStream) {
        return
    }

    $stream = $script:SynapseSetupMaintenanceLockStream
    try {
        $self = Get-CimInstance Win32_Process -Filter "ProcessId=$PID" -ErrorAction SilentlyContinue
        $owner = [ordered]@{
            schema = 'synapse_setup_maintenance_lock/v1'
            state = $State
            reason = $script:SynapseSetupMaintenanceLockReason
            pid = $PID
            parent_pid = if ($self) { $self.ParentProcessId } else { $null }
            process_name = if ($self) { $self.Name } else { $null }
            command_line = if ($self) { $self.CommandLine } else { $null }
            source_dir = $SourceDir
            bind = $Bind
            released_at_utc = (Get-Date).ToUniversalTime().ToString('o')
            cleanup_policy = 'never close terminal windows globally; only exact process IDs spawned by this setup operation or verified synapse-mcp targets may be stopped'
        }
        if (-not [string]::IsNullOrWhiteSpace($ErrorMessage)) {
            $owner.error = ($ErrorMessage -replace '\s+', ' ').Trim()
        }
        $json = $owner | ConvertTo-Json -Depth 6
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($json + "`n")
        $stream.SetLength(0)
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
        Info "Maintenance lock $State path=$script:SynapseSetupMaintenanceLockPath pid=$PID"
    } catch {
        Info "WARN: could not write maintenance lock release state path=$script:SynapseSetupMaintenanceLockPath error=$($_.Exception.Message)"
    } finally {
        $stream.Dispose()
        $script:SynapseSetupMaintenanceLockStream = $null
        $script:SynapseSetupMaintenanceLockPath = $null
        $script:SynapseSetupMaintenanceLockReason = $null
    }
}

function Wait-SynapsePostExitParent {
    param(
        [int]$ParentPid,
        [string]$Reason
    )

    if ($ParentPid -le 0) {
        return
    }

    $parent = Get-Process -Id $ParentPid -ErrorAction SilentlyContinue
    if ($null -eq $parent) {
        Info "SYNAPSE_POST_EXIT_PARENT_ALREADY_GONE parent_pid=$ParentPid reason=$Reason"
        return
    }

    Info "SYNAPSE_POST_EXIT_PARENT_WAIT parent_pid=$ParentPid reason=$Reason"
    try {
        Wait-Process -Id $ParentPid -Timeout 180 -ErrorAction Stop
    } catch {
        $stillAlive = [bool](Get-Process -Id $ParentPid -ErrorAction SilentlyContinue)
        if ($stillAlive) {
            Die "SYNAPSE_POST_EXIT_PARENT_STILL_RUNNING parent_pid=$ParentPid reason=$Reason remediation=the setup continuation waits for the parent runner to exit before reacquiring the maintenance lock; inspect the parent process and setup logs, never kill terminal/IDE/WSL hosts globally"
        }
    }
    Start-Sleep -Seconds 1
    Info "SYNAPSE_POST_EXIT_PARENT_GONE parent_pid=$ParentPid reason=$Reason"
}

trap {
    # #377: a FATAL must end the process non-zero and record the failure. `break`
    # under pwsh -File exited with the last native exit code (often 0).
    $errorText = $_ | Out-String
    $oneLineError = ($errorText -replace '\s+', ' ').Trim()
    $repairState = ''
    try {
        if (-not [string]::IsNullOrWhiteSpace($script:SynapseSetupRepairManifestPath) -and (Test-Path -LiteralPath $script:SynapseSetupRepairManifestPath -PathType Leaf)) {
            $repairState = [string](Get-Content -Raw -LiteralPath $script:SynapseSetupRepairManifestPath | ConvertFrom-Json).state
        }
    } catch {
        Info "WARN: could not read setup repair manifest state path=$script:SynapseSetupRepairManifestPath error=$($_.Exception.Message)"
    }
    $postExitState = ''
    if ($script:SynapsePostExitStartOnly) {
        try {
            if (-not [string]::IsNullOrWhiteSpace($PostExitManifestPath) -and (Test-Path -LiteralPath $PostExitManifestPath -PathType Leaf)) {
                $postExitState = [string](Get-Content -Raw -LiteralPath $PostExitManifestPath | ConvertFrom-Json).state
            }
        } catch {
            Info "WARN: could not read post-exit manifest state path=$PostExitManifestPath error=$($_.Exception.Message)"
        }
    }
    $fatalPlan = Resolve-SynapseSetupFatalTrapPlan `
        -RepairManifestState $repairState `
        -PostExitStartOnly ([bool]$script:SynapsePostExitStartOnly) `
        -PostExitManifestState $postExitState `
        -LastExitCode ([int]$global:LASTEXITCODE)
    if ($fatalPlan.WriteRepairFailed) {
        try {
            Write-SynapseSetupRepairManifestState -State 'failed' -Message $oneLineError -ExitCode $fatalPlan.ExitCode -Readback $null
        } catch {
            Info "WARN: could not write setup repair manifest failure state path=$script:SynapseSetupRepairManifestPath error=$($_.Exception.Message)"
        }
    }
    if ($fatalPlan.WritePostExitFailed) {
        try {
            Write-SynapsePostExitManifestState -State 'failed' -Message $oneLineError -ExitCode $fatalPlan.ExitCode -Readback $null
        } catch {
            Info "WARN: could not write post-exit manifest failure state path=$PostExitManifestPath error=$($_.Exception.Message)"
        }
    }
    try {
        if (Get-Command Remove-SynapseSetupOwnedDirectories -ErrorAction SilentlyContinue) { Remove-SynapseSetupOwnedDirectories }
    } catch {
        Info "WARN: could not clean setup-owned run directories error=$($_.Exception.Message)"
    }
    Release-SynapseSetupMaintenanceLock -State $fatalPlan.LockState -ErrorMessage $errorText
    [Console]::Error.WriteLine($errorText)
    exit $fatalPlan.ExitCode
}

# #377: these checks call Die, so they run only once Die, the trap and the lock
# helpers exist (earlier, a misuse failed with "The term 'Die' is not recognized").
if ($PostExitUseInstalledBinary -and -not ($script:SynapsePostExitStartOnly -and $SkipBuild)) {
    Die "SYNAPSE_POST_EXIT_USE_INSTALLED_BINARY_MISUSED remediation=-PostExitUseInstalledBinary is reserved for the dead_owner_bind_after_install continuation, which is launched with -PostExitParentPid and -SkipBuild and must validate the already-installed verified daemon bytes; remove the flag for a plain -SkipBuild run, which installs the CargoTarget release artifact"
}
if ($PostExitUseInstalledBinary -and [string]::IsNullOrWhiteSpace($PostExitInstalledSha256)) {
    Die "SYNAPSE_POST_EXIT_INSTALLED_SHA_MISSING remediation=the continuation launcher must pass -PostExitInstalledSha256 with the sha256 the parent computed after installing, so the continuation proves the installed bytes were not replaced between parent verification and continuation preflight"
}
if ($script:SynapsePostExitStartOnly -and -not $PostExitUseInstalledBinary) {
    Die "SYNAPSE_POST_EXIT_INSTALLED_BINARY_MODE_REQUIRED remediation=the dead_owner_bind_after_install continuation must be launched by Start-SynapsePostExitSetupContinuation with -PostExitUseInstalledBinary and -PostExitInstalledSha256, so it validates the bytes the parent installed and verified; a hand-written continuation must not silently validate an arbitrary CargoTarget artifact instead"
}

function Quote-WindowsCommandArgument {
    param([Parameter(Mandatory=$true)][string]$Value)
    if ($Value.Length -eq 0) { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }
    $escaped = $Value -replace '(\\*)"', '$1$1\"'
    $escaped = $escaped -replace '(\\+)$', '$1$1'
    return '"' + $escaped + '"'
}

function Quote-VbsString {
    param([Parameter(Mandatory=$true)][string]$Value)
    return '"' + ($Value -replace '"', '""') + '"'
}

function Quote-PowerShellSingleQuotedString {
    param([AllowNull()][string]$Value)
    if ($null -eq $Value) { $Value = '' }
    return "'" + ($Value -replace "'", "''") + "'"
}

function Vbs-Literal {
    param([Parameter(Mandatory=$true)][string]$Value)
    return '"' + ($Value -replace '"', '""') + '"'
}

function New-HiddenDaemonLauncher {
    param(
        [Parameter(Mandatory=$true)][string]$OutputPath,
        [Parameter(Mandatory=$true)][string]$ExePath,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath,
        [Parameter(Mandatory=$true)][string]$ProfilesDir,
        [Parameter(Mandatory=$true)][string]$LogDir,
        [Parameter(Mandatory=$true)][string]$TokenPath
    )

    $daemonLogDir = $LogDir
    $launcherLog = Join-Path $LogDir 'daemon-launcher.log'
    $daemonCommand = @(
        (Quote-WindowsCommandArgument $ExePath),
        '--mode', 'http',
        '--bind', (Quote-WindowsCommandArgument $Bind),
        '--db', (Quote-WindowsCommandArgument $DbPath),
        '--profile-dir', (Quote-WindowsCommandArgument $ProfilesDir),
        '--log-level', 'info'
    ) -join ' '

    @"
Option Explicit
Dim shell, fso, env, tokenPath, launcherLog, daemonLogDir, daemonCommand
Dim tokenFile, token, exitCode

Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
tokenPath = $(Vbs-Literal $TokenPath)
launcherLog = $(Vbs-Literal $launcherLog)
daemonLogDir = $(Vbs-Literal $daemonLogDir)
daemonCommand = $(Vbs-Literal $daemonCommand)

Sub LogLine(message)
  Dim logFile
  Set logFile = fso.OpenTextFile(launcherLog, 8, True)
  logFile.WriteLine Now & " " & message
  logFile.Close
End Sub

On Error Resume Next
If Not fso.FolderExists(daemonLogDir) Then
  fso.CreateFolder daemonLogDir
End If
If Err.Number <> 0 Then
  WScript.Quit 1
End If
On Error GoTo 0

If Not fso.FileExists(tokenPath) Then
  LogLine "SYNAPSE_DAEMON_TOKEN_MISSING path=" & tokenPath
  WScript.Quit 1
End If

On Error Resume Next
Set tokenFile = fso.OpenTextFile(tokenPath, 1, False)
If Err.Number <> 0 Then
  LogLine "SYNAPSE_DAEMON_TOKEN_READ_FAILED path=" & tokenPath & " err_number=" & Err.Number & " err_description=" & Err.Description
  WScript.Quit 1
End If
If tokenFile.AtEndOfStream Then
  token = ""
Else
  token = Trim(tokenFile.ReadAll)
End If
If Err.Number <> 0 Then
  LogLine "SYNAPSE_DAEMON_TOKEN_READ_FAILED path=" & tokenPath & " err_number=" & Err.Number & " err_description=" & Err.Description
  tokenFile.Close
  WScript.Quit 1
End If
tokenFile.Close
On Error GoTo 0

If Len(token) < 16 Then
  LogLine "SYNAPSE_DAEMON_TOKEN_INVALID path=" & tokenPath & " length=" & Len(token)
  WScript.Quit 1
End If

Set env = shell.Environment("PROCESS")
env("SYNAPSE_BEARER_TOKEN") = token
env("SYNAPSE_LOG_DIR") = daemonLogDir

LogLine "SYNAPSE_DAEMON_LAUNCH_START command=" & daemonCommand
exitCode = shell.Run(daemonCommand, 0, True)
LogLine "SYNAPSE_DAEMON_EXIT exit_code=" & exitCode
WScript.Quit exitCode
"@ | Set-Content -Path $OutputPath -Encoding ascii
}

function Ensure-SynapseSetupProcessJobType {
    if ('SynapseSetup.ProcessJob' -as [type]) { return }

    Add-Type -Language CSharp -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

namespace SynapseSetup
{
    public static class ProcessJob
    {
        private const uint CREATE_SUSPENDED = 0x00000004;
        private const uint CREATE_NO_WINDOW = 0x08000000;
        private const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
        private const int JobObjectExtendedLimitInformation = 9;
        private const uint WAIT_OBJECT_0 = 0x00000000;
        private const uint WAIT_TIMEOUT = 0x00000102;
        private const uint WAIT_FAILED = 0xffffffff;
        private const uint WAIT_NOT_CALLED = 0xfffffffe;
        private const uint EXIT_TIMEOUT = 124;
        private const uint EXIT_ASSIGN_FAILED = 125;
        private const uint EXIT_RESUME_FAILED = 126;
        private const uint INFINITE = 0xffffffff;
        private const uint EXIT_JOB_CLEANUP = 130;
        private const int JobObjectBasicProcessIdList = 3;
        private const int JOB_PROCESS_ID_CAPACITY = 1024;
        private const int JOB_DRAIN_TIMEOUT_MS = 15000;
        private const int JOB_DRAIN_POLL_MS = 100;

        [StructLayout(LayoutKind.Sequential)]
        private struct IO_COUNTERS
        {
            public ulong ReadOperationCount;
            public ulong WriteOperationCount;
            public ulong OtherOperationCount;
            public ulong ReadTransferCount;
            public ulong WriteTransferCount;
            public ulong OtherTransferCount;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_BASIC_LIMIT_INFORMATION
        {
            public long PerProcessUserTimeLimit;
            public long PerJobUserTimeLimit;
            public uint LimitFlags;
            public UIntPtr MinimumWorkingSetSize;
            public UIntPtr MaximumWorkingSetSize;
            public uint ActiveProcessLimit;
            public UIntPtr Affinity;
            public uint PriorityClass;
            public uint SchedulingClass;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION
        {
            public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation;
            public IO_COUNTERS IoInfo;
            public UIntPtr ProcessMemoryLimit;
            public UIntPtr JobMemoryLimit;
            public UIntPtr PeakProcessMemoryUsed;
            public UIntPtr PeakJobMemoryUsed;
        }

        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
        private struct STARTUPINFO
        {
            public uint cb;
            public string lpReserved;
            public string lpDesktop;
            public string lpTitle;
            public uint dwX;
            public uint dwY;
            public uint dwXSize;
            public uint dwYSize;
            public uint dwXCountChars;
            public uint dwYCountChars;
            public uint dwFillAttribute;
            public uint dwFlags;
            public ushort wShowWindow;
            public ushort cbReserved2;
            public IntPtr lpReserved2;
            public IntPtr hStdInput;
            public IntPtr hStdOutput;
            public IntPtr hStdError;
        }

        [StructLayout(LayoutKind.Sequential)]
        private struct PROCESS_INFORMATION
        {
            public IntPtr hProcess;
            public IntPtr hThread;
            public uint dwProcessId;
            public uint dwThreadId;
        }

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        private static extern IntPtr CreateJobObject(IntPtr lpJobAttributes, string lpName);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool SetInformationJobObject(
            IntPtr hJob,
            int jobObjectInfoClass,
            IntPtr lpJobObjectInfo,
            uint cbJobObjectInfoLength);

        [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
        private static extern bool CreateProcess(
            string lpApplicationName,
            StringBuilder lpCommandLine,
            IntPtr lpProcessAttributes,
            IntPtr lpThreadAttributes,
            bool bInheritHandles,
            uint dwCreationFlags,
            IntPtr lpEnvironment,
            string lpCurrentDirectory,
            ref STARTUPINFO lpStartupInfo,
            out PROCESS_INFORMATION lpProcessInformation);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint ResumeThread(IntPtr hThread);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern uint WaitForSingleObject(IntPtr hHandle, uint dwMilliseconds);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool GetExitCodeProcess(IntPtr hProcess, out uint lpExitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool TerminateJobObject(IntPtr hJob, uint uExitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool QueryInformationJobObject(
            IntPtr hJob,
            int jobObjectInfoClass,
            IntPtr lpJobObjectInfo,
            uint cbJobObjectInfoLength,
            out uint lpReturnLength);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool TerminateProcess(IntPtr hProcess, uint uExitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool CloseHandle(IntPtr hObject);

        public static int Run(
            string applicationName,
            string commandLine,
            string workingDirectory,
            uint timeoutMilliseconds,
            out string failure,
            out string diagnosticsJson)
        {
            failure = "";
            diagnosticsJson = "";
            string completionKind = "not_started";
            string waitKind = "not_waited";
            uint wait = WAIT_NOT_CALLED;
            uint exitCode = 0xffffffff;
            uint cleanupWait = WAIT_NOT_CALLED;
            uint childPid = 0;
            bool jobCreated = false;
            bool processCreated = false;
            bool assignedToJob = false;
            bool resumed = false;
            bool timedOut = false;
            bool terminateJobCalled = false;
            bool terminateJobOk = false;
            string terminateJobError = "";
            bool jobQueryOk = false;
            uint[] jobActiveProcessIdsAfter = new uint[0];
            int jobDrainWaitMs = 0;
            IntPtr job = IntPtr.Zero;
            IntPtr limitPointer = IntPtr.Zero;
            PROCESS_INFORMATION processInfo = new PROCESS_INFORMATION();
            try
            {
                job = CreateJobObject(IntPtr.Zero, null);
                if (job == IntPtr.Zero)
                {
                    failure = "PROCESS_JOB_CREATE_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "job_create_failed";
                    return 127;
                }
                jobCreated = true;

                JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits = new JOBOBJECT_EXTENDED_LIMIT_INFORMATION();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                int limitSize = Marshal.SizeOf(typeof(JOBOBJECT_EXTENDED_LIMIT_INFORMATION));
                limitPointer = Marshal.AllocHGlobal(limitSize);
                Marshal.StructureToPtr(limits, limitPointer, false);
                if (!SetInformationJobObject(job, JobObjectExtendedLimitInformation, limitPointer, (uint)limitSize))
                {
                    failure = "PROCESS_JOB_LIMIT_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "job_limit_failed";
                    return 127;
                }

                STARTUPINFO startupInfo = new STARTUPINFO();
                startupInfo.cb = (uint)Marshal.SizeOf(typeof(STARTUPINFO));
                StringBuilder mutableCommandLine = new StringBuilder(commandLine);
                bool created = CreateProcess(
                    applicationName,
                    mutableCommandLine,
                    IntPtr.Zero,
                    IntPtr.Zero,
                    false,
                    CREATE_SUSPENDED | CREATE_NO_WINDOW,
                    IntPtr.Zero,
                    workingDirectory,
                    ref startupInfo,
                    out processInfo);
                if (!created)
                {
                    failure = "PROCESS_JOB_CREATE_PROCESS_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "process_create_failed";
                    return 127;
                }
                processCreated = true;
                childPid = processInfo.dwProcessId;

                if (!AssignProcessToJobObject(job, processInfo.hProcess))
                {
                    failure = "PROCESS_JOB_ASSIGN_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "job_assign_failed";
                    bool terminated = TerminateProcess(processInfo.hProcess, EXIT_ASSIGN_FAILED);
                    terminateJobCalled = false;
                    terminateJobOk = terminated;
                    if (!terminated)
                    {
                        terminateJobError = new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    }
                    return (int)EXIT_ASSIGN_FAILED;
                }
                assignedToJob = true;

                if (ResumeThread(processInfo.hThread) == 0xffffffff)
                {
                    failure = "PROCESS_JOB_RESUME_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "resume_failed";
                    terminateJobCalled = true;
                    terminateJobOk = TerminateJobObject(job, EXIT_RESUME_FAILED);
                    if (!terminateJobOk)
                    {
                        terminateJobError = new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    }
                    return (int)EXIT_RESUME_FAILED;
                }
                resumed = true;

                wait = WaitForSingleObject(
                    processInfo.hProcess,
                    timeoutMilliseconds == 0 ? INFINITE : timeoutMilliseconds);
                waitKind = WaitKind(wait);
                if (wait == WAIT_TIMEOUT)
                {
                    timedOut = true;
                    completionKind = "timeout";
                    terminateJobCalled = true;
                    terminateJobOk = TerminateJobObject(job, EXIT_TIMEOUT);
                    if (!terminateJobOk)
                    {
                        terminateJobError = new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    }
                    cleanupWait = WaitForSingleObject(processInfo.hProcess, 15000);
                    failure = "PROCESS_JOB_TIMEOUT: child process tree exceeded timeout_ms=" + timeoutMilliseconds;
                    return (int)EXIT_TIMEOUT;
                }
                if (wait == WAIT_FAILED)
                {
                    failure = "PROCESS_JOB_WAIT_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "wait_failed";
                    terminateJobCalled = true;
                    terminateJobOk = TerminateJobObject(job, 127);
                    if (!terminateJobOk)
                    {
                        terminateJobError = new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    }
                    return 127;
                }
                if (wait != WAIT_OBJECT_0)
                {
                    failure = "PROCESS_JOB_WAIT_UNEXPECTED: wait_result=" + wait;
                    completionKind = "wait_unexpected";
                    terminateJobCalled = true;
                    terminateJobOk = TerminateJobObject(job, 127);
                    if (!terminateJobOk)
                    {
                        terminateJobError = new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    }
                    return 127;
                }

                if (!GetExitCodeProcess(processInfo.hProcess, out exitCode))
                {
                    failure = "PROCESS_JOB_EXIT_CODE_FAILED: " + new Win32Exception(Marshal.GetLastWin32Error()).Message;
                    completionKind = "exit_code_failed";
                    return 127;
                }
                completionKind = "child_exit";
                return unchecked((int)exitCode);
            }
            finally
            {
                // #379: every exit path terminates the job and reads its own PID list
                // back until it is empty (or the drain budget ends), so cleanup is
                // proven from the job, not assumed from closing the handle.
                if (job != IntPtr.Zero && assignedToJob)
                {
                    if (!terminateJobCalled)
                    {
                        terminateJobCalled = true;
                        terminateJobOk = TerminateJobObject(job, EXIT_JOB_CLEANUP);
                        if (!terminateJobOk)
                        {
                            terminateJobError = new Win32Exception(Marshal.GetLastWin32Error()).Message;
                        }
                    }
                    jobQueryOk = DrainJob(job, out jobActiveProcessIdsAfter, out jobDrainWaitMs);
                }
                diagnosticsJson = BuildDiagnosticsJson(
                    applicationName,
                    commandLine,
                    workingDirectory,
                    timeoutMilliseconds,
                    jobCreated,
                    processCreated,
                    childPid,
                    assignedToJob,
                    resumed,
                    wait,
                    waitKind,
                    timedOut,
                    exitCode,
                    completionKind,
                    terminateJobCalled,
                    terminateJobOk,
                    terminateJobError,
                    cleanupWait,
                    failure,
                    jobQueryOk,
                    jobActiveProcessIdsAfter,
                    jobDrainWaitMs);
                if (limitPointer != IntPtr.Zero)
                {
                    Marshal.FreeHGlobal(limitPointer);
                }
                if (processInfo.hThread != IntPtr.Zero)
                {
                    CloseHandle(processInfo.hThread);
                }
                if (processInfo.hProcess != IntPtr.Zero)
                {
                    CloseHandle(processInfo.hProcess);
                }
                if (job != IntPtr.Zero)
                {
                    CloseHandle(job);
                }
            }
        }

        private static string WaitKind(uint wait)
        {
            if (wait == WAIT_NOT_CALLED) return "not_called";
            if (wait == WAIT_OBJECT_0) return "WAIT_OBJECT_0";
            if (wait == WAIT_TIMEOUT) return "WAIT_TIMEOUT";
            if (wait == WAIT_FAILED) return "WAIT_FAILED";
            return "unexpected_" + wait.ToString();
        }

        private static string JsonEscape(string value)
        {
            if (value == null) return "";
            StringBuilder escaped = new StringBuilder();
            foreach (char c in value)
            {
                switch (c)
                {
                    case '\\': escaped.Append("\\\\"); break;
                    case '"': escaped.Append("\\\""); break;
                    case '\b': escaped.Append("\\b"); break;
                    case '\f': escaped.Append("\\f"); break;
                    case '\n': escaped.Append("\\n"); break;
                    case '\r': escaped.Append("\\r"); break;
                    case '\t': escaped.Append("\\t"); break;
                    default:
                        if (c < 0x20)
                        {
                            escaped.Append("\\u");
                            escaped.Append(((int)c).ToString("x4"));
                        }
                        else
                        {
                            escaped.Append(c);
                        }
                        break;
                }
            }
            return escaped.ToString();
        }

        private static string BuildDiagnosticsJson(
            string applicationName,
            string commandLine,
            string workingDirectory,
            uint timeoutMilliseconds,
            bool jobCreated,
            bool processCreated,
            uint childPid,
            bool assignedToJob,
            bool resumed,
            uint waitResult,
            string waitKind,
            bool timedOut,
            uint exitCode,
            string completionKind,
            bool terminateJobCalled,
            bool terminateJobOk,
            string terminateJobError,
            uint cleanupWait,
            string failure,
            bool jobQueryOk,
            uint[] jobActiveProcessIdsAfter,
            int jobDrainWaitMs)
        {
            int signedExitCode = unchecked((int)exitCode);
            StringBuilder json = new StringBuilder();
            json.Append("{");
            json.Append("\"schema\":\"synapse_process_job_result/v1\"");
            json.Append(",\"application_name\":\"").Append(JsonEscape(applicationName)).Append("\"");
            json.Append(",\"command_line\":\"").Append(JsonEscape(commandLine)).Append("\"");
            json.Append(",\"working_directory\":\"").Append(JsonEscape(workingDirectory)).Append("\"");
            json.Append(",\"timeout_ms\":").Append(timeoutMilliseconds);
            json.Append(",\"job_created\":").Append(jobCreated ? "true" : "false");
            json.Append(",\"process_created\":").Append(processCreated ? "true" : "false");
            json.Append(",\"child_pid\":").Append(childPid == 0 ? "null" : childPid.ToString());
            json.Append(",\"assigned_to_job\":").Append(assignedToJob ? "true" : "false");
            json.Append(",\"resumed\":").Append(resumed ? "true" : "false");
            json.Append(",\"wait_result\":").Append(waitResult);
            json.Append(",\"wait_kind\":\"").Append(JsonEscape(waitKind)).Append("\"");
            json.Append(",\"timed_out\":").Append(timedOut ? "true" : "false");
            json.Append(",\"exit_code_unsigned\":").Append(exitCode);
            json.Append(",\"exit_code_signed\":").Append(signedExitCode);
            json.Append(",\"exit_code_hex\":\"0x").Append(exitCode.ToString("X8")).Append("\"");
            json.Append(",\"completion_kind\":\"").Append(JsonEscape(completionKind)).Append("\"");
            json.Append(",\"terminate_job_called\":").Append(terminateJobCalled ? "true" : "false");
            json.Append(",\"terminate_job_ok\":").Append(terminateJobOk ? "true" : "false");
            json.Append(",\"terminate_job_error\":\"").Append(JsonEscape(terminateJobError)).Append("\"");
            json.Append(",\"cleanup_wait_result\":").Append(cleanupWait);
            json.Append(",\"cleanup_wait_kind\":\"").Append(JsonEscape(WaitKind(cleanupWait))).Append("\"");
            json.Append(",\"failure\":\"").Append(JsonEscape(failure)).Append("\"");
            json.Append(",\"job_query_ok\":").Append(jobQueryOk ? "true" : "false");
            json.Append(",\"job_active_process_ids_after\":[");
            for (int i = 0; i < jobActiveProcessIdsAfter.Length; i++)
            {
                if (i > 0) json.Append(",");
                json.Append(jobActiveProcessIdsAfter[i]);
            }
            json.Append("]");
            json.Append(",\"job_drain_wait_ms\":").Append(jobDrainWaitMs);
            json.Append("}");
            return json.ToString();
        }

        // JOBOBJECT_BASIC_PROCESS_ID_LIST: two DWORD counts, then ULONG_PTR ids at offset 8.
        private static bool TryReadJobProcessIds(IntPtr job, out uint[] processIds)
        {
            processIds = new uint[0];
            int size = 8 + JOB_PROCESS_ID_CAPACITY * IntPtr.Size;
            IntPtr buffer = Marshal.AllocHGlobal(size);
            try
            {
                uint returned;
                if (!QueryInformationJobObject(job, JobObjectBasicProcessIdList, buffer, (uint)size, out returned))
                {
                    return false;
                }
                int count = Marshal.ReadInt32(buffer, 4);
                if (count < 0 || count > JOB_PROCESS_ID_CAPACITY)
                {
                    return false;
                }
                processIds = new uint[count];
                for (int i = 0; i < count; i++)
                {
                    processIds[i] = unchecked((uint)Marshal.ReadIntPtr(buffer, 8 + i * IntPtr.Size).ToInt64());
                }
                return true;
            }
            finally
            {
                Marshal.FreeHGlobal(buffer);
            }
        }

        // Polls the job's own PID list until it is empty or the drain budget ends.
        // Returns false only when the job cannot be read back.
        private static bool DrainJob(IntPtr job, out uint[] remaining, out int waitedMs)
        {
            waitedMs = 0;
            while (true)
            {
                if (!TryReadJobProcessIds(job, out remaining))
                {
                    return false;
                }
                if (remaining.Length == 0 || waitedMs >= JOB_DRAIN_TIMEOUT_MS)
                {
                    return true;
                }
                System.Threading.Thread.Sleep(JOB_DRAIN_POLL_MS);
                waitedMs += JOB_DRAIN_POLL_MS;
            }
        }
    }
}
'@ | Out-Null
}

function Invoke-SynapseProcessInKillOnCloseJob {
    param(
        [Parameter(Mandatory=$true)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [Parameter(Mandatory=$true)][string]$WorkingDirectory,
        [Parameter(Mandatory=$true)][int]$TimeoutMinutes,
        [string]$LogPath,
        [System.Management.Automation.PSReference]$Diagnostics
    )

    Ensure-SynapseSetupProcessJobType
    if (-not (Test-Path $FilePath)) { Die "Process job target missing: $FilePath" }
    if (-not (Test-Path $WorkingDirectory)) { Die "Process job working directory missing: $WorkingDirectory" }

    $argumentText = (($ArgumentList | ForEach-Object { Quote-WindowsCommandArgument $_ }) -join ' ').Trim()
    $targetCommand = (Quote-WindowsCommandArgument $FilePath)
    if (-not [string]::IsNullOrWhiteSpace($argumentText)) {
        $targetCommand = "$targetCommand $argumentText"
    }

    $applicationPath = $FilePath
    $commandLine = $targetCommand
    if (-not [string]::IsNullOrWhiteSpace($LogPath)) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogPath) | Out-Null
        if (Test-Path $LogPath) { Remove-Item $LogPath -Force }
        $cmdPath = Join-Path $env:SystemRoot 'System32\cmd.exe'
        $redirectCommand = "$targetCommand > $(Quote-WindowsCommandArgument $LogPath) 2>&1"
        $applicationPath = $cmdPath
        $commandLine = "$(Quote-WindowsCommandArgument $cmdPath) /d /s /c `"$redirectCommand`""
    }

    $timeoutMilliseconds = [uint32]([math]::Min([int64]$TimeoutMinutes * 60 * 1000, [uint32]::MaxValue))
    $failure = ''
    $processJobDiagnosticsJson = ''
    $startedAt = (Get-Date).ToUniversalTime().ToString('o')
    $compilerProcessesBefore = @(Get-SynapseBuildToolProcessSnapshot)
    $exitCode = [SynapseSetup.ProcessJob]::Run(
        $applicationPath,
        $commandLine,
        $WorkingDirectory,
        $timeoutMilliseconds,
        [ref]$failure,
        [ref]$processJobDiagnosticsJson)
    $completedAt = (Get-Date).ToUniversalTime().ToString('o')
    $compilerProcessesAfter = @(Get-SynapseBuildToolProcessSnapshot)
    $processJobDiagnostics = $null
    if (-not [string]::IsNullOrWhiteSpace($processJobDiagnosticsJson)) {
        try {
            $processJobDiagnostics = $processJobDiagnosticsJson | ConvertFrom-Json -ErrorAction Stop
        } catch {
            $processJobDiagnostics = [pscustomobject]@{
                schema = 'synapse_process_job_result_parse_failed/v1'
                raw = $processJobDiagnosticsJson
                parse_error = $_.Exception.Message
            }
        }
    }
    $childProcessAfter = $null
    if ($processJobDiagnostics -and $processJobDiagnostics.child_pid) {
        $childPid = [int]$processJobDiagnostics.child_pid
        $childProcessAfter = Get-CimInstance Win32_Process -Filter "ProcessId=$childPid" -ErrorAction SilentlyContinue |
            Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine
    }
    $diagnosticObject = [ordered]@{
        schema = 'synapse_setup_process_job_invocation/v1'
        command = $targetCommand
        application_path = $applicationPath
        working_directory = $WorkingDirectory
        timeout_minutes = $TimeoutMinutes
        timeout_ms = $timeoutMilliseconds
        log_path = $LogPath
        started_at_utc = $startedAt
        completed_at_utc = $completedAt
        exit_code = $exitCode
        failure = $failure
        process_job = $processJobDiagnostics
        child_process_after = $childProcessAfter
        build_tool_processes_before = $compilerProcessesBefore
        build_tool_processes_after = $compilerProcessesAfter
        cleanup_result = [ordered]@{
            process_table_after_read = $true
            child_process_alive_after = ($null -ne $childProcessAfter)
            live_build_tool_process_count_after = @($compilerProcessesAfter).Count
        }
    }
    $cleanupVerdict = $null
    if ($processJobDiagnostics -and $processJobDiagnostics.assigned_to_job -eq $true) {
        # #379: zero job-owned processes must be proven from the job's own PID list;
        # other agents' cargo/rustc are reported as unrelated and never touched.
        $cleanupVerdict = Resolve-SynapseProcessJobCleanupVerdict `
            -JobQueryOk ($processJobDiagnostics.job_query_ok -eq $true) `
            -JobActivePidsAfter @($processJobDiagnostics.job_active_process_ids_after | ForEach-Object { [int64]$_ }) `
            -BuildToolPidsAfter @($compilerProcessesAfter | ForEach-Object { [int64]$_.ProcessId })
        $diagnosticObject.cleanup_result.job_query_ok = ($processJobDiagnostics.job_query_ok -eq $true)
        $diagnosticObject.cleanup_result.job_clean = $cleanupVerdict.JobClean
        $diagnosticObject.cleanup_result.job_leaked_process_ids_after = @($cleanupVerdict.JobLeakedPids)
        $diagnosticObject.cleanup_result.unrelated_build_tool_process_ids_after = @($cleanupVerdict.UnrelatedBuildToolPids)
    }
    if ($PSBoundParameters.ContainsKey('Diagnostics')) {
        $Diagnostics.Value = [pscustomobject]$diagnosticObject
    }
    if ($cleanupVerdict -and $cleanupVerdict.FailClosed) {
        Die "$($cleanupVerdict.Code) command=$targetCommand job_query_ok=$($diagnosticObject.cleanup_result.job_query_ok) job_leaked_process_ids_after=$(@($cleanupVerdict.JobLeakedPids) -join ',') unrelated_build_tool_process_ids_after=$(@($cleanupVerdict.UnrelatedBuildToolPids) -join ',') remediation=setup's kill-on-close job still held processes (or could not be read back) after TerminateJobObject and a 15 s drain; inspect those exact PIDs, never kill other agents' cargo/rustc"
    }
    return $exitCode
}

function Get-SynapseBuildToolProcessSnapshot {
    @(Get-CimInstance Win32_Process -Filter "Name='cargo.exe' OR Name='rustc.exe'" -ErrorAction SilentlyContinue |
        Sort-Object ProcessId |
        Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine)
}

function Resolve-SynapseProcessJobCleanupVerdict {
    # #379: did the setup build job end with zero job-owned processes? Pure: takes the
    # job's own PID readback and the machine-wide cargo/rustc PIDs.
    param(
        [bool]$JobQueryOk = $false,
        [AllowEmptyCollection()][int64[]]$JobActivePidsAfter = @(),
        [AllowEmptyCollection()][int64[]]$BuildToolPidsAfter = @()
    )
    # Only the job's own PID list decides "leaked"; another agent's cargo/rustc on the
    # same machine is reported as unrelated and never counted (or touched) as setup's.
    if (-not $JobQueryOk) {
        return [pscustomobject]@{
            JobClean = $false
            JobLeakedPids = @()
            UnrelatedBuildToolPids = @($BuildToolPidsAfter | Sort-Object -Unique)
            FailClosed = $true
            Code = 'SYNAPSE_SETUP_PROCESS_JOB_READBACK_FAILED'
        }
    }
    $leaked = @($JobActivePidsAfter | Sort-Object -Unique)
    $unrelated = @($BuildToolPidsAfter | Where-Object { $leaked -notcontains $_ } | Sort-Object -Unique)
    $clean = ($leaked.Count -eq 0)
    return [pscustomobject]@{
        JobClean = $clean
        JobLeakedPids = $leaked
        UnrelatedBuildToolPids = $unrelated
        FailClosed = (-not $clean)
        Code = $(if ($clean) { '' } else { 'SYNAPSE_SETUP_PROCESS_JOB_LEAKED' })
    }
}

function Get-SynapseBuildLogSignal {
    param([string]$Path)

    $signal = [ordered]@{
        path = $Path
        exists = $false
        has_compiler_error = $false
        compiler_error_matches = @()
        tail_80 = ''
    }
    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]$signal
    }
    $signal.exists = $true
    $signal.tail_80 = (Get-Content -LiteralPath $Path -Tail 80 -ErrorAction SilentlyContinue) -join "`n"
    $matches = @(Select-String -LiteralPath $Path -Pattern '(?i)(^error(\[.*\])?:|^error:|fatal error|could not compile|failed to run custom build command|panicked at)' -ErrorAction SilentlyContinue |
        Select-Object -First 20 LineNumber, Line)
    $signal.has_compiler_error = ($matches.Count -gt 0)
    $signal.compiler_error_matches = @($matches)
    return [pscustomobject]$signal
}

function Get-SynapseArtifactReadback {
    param([Parameter(Mandatory=$true)][string]$Path)

    $readback = [ordered]@{
        path = $Path
        exists = $false
        length_bytes = $null
        sha256 = $null
        exclusive_open = 'not_checked'
        exclusive_open_error = $null
    }
    if (-not (Test-Path -LiteralPath $Path)) {
        $readback.exclusive_open = 'missing'
        return [pscustomobject]$readback
    }
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    $readback.exists = $true
    $readback.length_bytes = $item.Length
    try {
        $readback.sha256 = Get-SynapseFileSha256 -Path $Path
    } catch {
        $readback.sha256 = $null
        $readback.exclusive_open_error = "hash_failed: $($_.Exception.Message)"
    }
    try {
        $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::None)
        try {
            $readback.exclusive_open = 'ok'
        } finally {
            $stream.Dispose()
        }
    } catch {
        $readback.exclusive_open = 'locked_or_unreadable'
        $readback.exclusive_open_error = $_.Exception.Message
    }
    return [pscustomobject]$readback
}

function Get-SynapseReleaseBuildFailureKind {
    param(
        [Parameter(Mandatory=$true)]$Diagnostics,
        [Parameter(Mandatory=$true)]$LogSignal,
        [Parameter(Mandatory=$true)]$ArtifactReadback
    )

    $job = $Diagnostics.process_job
    if ($job -and $job.completion_kind -eq 'timeout') {
        return [pscustomobject]@{
            code = 'SYNAPSE_RELEASE_BUILD_TIMEOUT'
            remediation = 'increase BuildTimeoutMinutes only after verifying rustc/cargo are still making progress, or inspect build_tool_processes_after and setup-build.log for a stuck compiler/linker'
        }
    }
    if ($job -and -not [string]::IsNullOrWhiteSpace([string]$job.failure) -and $job.completion_kind -ne 'child_exit') {
        return [pscustomobject]@{
            code = 'SYNAPSE_RELEASE_BUILD_PROCESS_JOB_FAILED'
            remediation = 'inspect process_job.failure, wait_kind, terminate_job_ok, and cleanup_wait_kind; repair the Windows process/job-object failure before rerunning setup'
        }
    }
    if ($ArtifactReadback.exclusive_open -eq 'locked_or_unreadable') {
        return [pscustomobject]@{
            code = 'SYNAPSE_RELEASE_BUILD_ARTIFACT_LOCKED'
            remediation = 'inspect the process table for a build or scanner process holding the release artifact; do not close protected terminal/IDE/WSL host processes'
        }
    }
    if ($LogSignal.has_compiler_error) {
        return [pscustomobject]@{
            code = 'SYNAPSE_RELEASE_BUILD_COMPILER_FAILED'
            remediation = 'repair the compiler error lines recorded in setup-build.log before rerunning setup'
        }
    }
    if ($job -and [int]$job.exit_code_signed -eq -1) {
        return [pscustomobject]@{
            code = 'SYNAPSE_RELEASE_BUILD_CHILD_EXIT_NO_COMPILER_ERROR'
            remediation = 'child process exited -1 without compiler diagnostics; inspect process_job child_pid, wait_kind, build_tool_processes_before/after, artifact_readback, and Windows host logs for external termination or toolchain process death'
        }
    }
    return [pscustomobject]@{
        code = 'SYNAPSE_RELEASE_BUILD_CHILD_EXIT'
        remediation = 'child process exited nonzero; inspect setup-build.log, process_job, build_tool_processes_before/after, and artifact_readback for the root cause'
    }
}

function Get-SynapseCargoVersionFailureKind {
    param([Parameter(Mandatory=$true)]$Diagnostics)

    $job = $Diagnostics.process_job
    if ($job -and $job.completion_kind -eq 'timeout') {
        return [pscustomobject]@{
            code = 'SYNAPSE_CARGO_VERSION_TIMEOUT'
            remediation = 'cargo --version did not return inside the setup preflight timeout; inspect child_pid, wait_kind, terminate_job_ok, and process table before rerunning setup'
        }
    }
    if ($job -and -not [string]::IsNullOrWhiteSpace([string]$job.failure) -and $job.completion_kind -ne 'child_exit') {
        return [pscustomobject]@{
            code = 'SYNAPSE_CARGO_VERSION_PROCESS_JOB_FAILED'
            remediation = 'repair the Windows process/job-object failure recorded in setup-cargo-version-diagnostics.json before rerunning setup'
        }
    }
    return [pscustomobject]@{
        code = 'SYNAPSE_CARGO_VERSION_FAILED'
        remediation = 'cargo --version exited nonzero; inspect setup-cargo-version.log and setup-cargo-version-diagnostics.json, then repair the Rust toolchain before rerunning setup'
    }
}

function Resolve-CodexLauncherPlan {
    # Which codex.js the patched launchers call, and which launcher directories get
    # patched. Pure: existence is passed in as the list of paths that exist.
    param(
        [string]$LauncherDir,
        [string]$NpmGlobalDir,
        [AllowEmptyCollection()][string[]]$ExistingPaths = @()
    )
    # The PATH-winning launcher is not always npm's own shim dir (a copied shim in
    # %USERPROFILE%\.codex\bin has no node_modules), so the launcher-local package
    # wins when present, else npm's global one; every dir holding a codex shim is
    # patched once (Windows paths compare case-insensitively).
    $package = 'node_modules\@openai\codex\bin\codex.js'
    $codexJs = $null
    $targets = @()
    foreach ($dir in @($LauncherDir, $NpmGlobalDir)) {
        if ([string]::IsNullOrWhiteSpace($dir)) { continue }
        $prefix = $dir.TrimEnd('\') + '\'
        if ($null -eq $codexJs -and $ExistingPaths -contains ($prefix + $package)) {
            $codexJs = $prefix + $package
        }
        $hasShim = @('codex.ps1', 'codex.cmd', 'codex') | Where-Object { $ExistingPaths -contains ($prefix + $_) }
        if ($hasShim -and $targets -notcontains $dir) {
            $targets += $dir
        }
    }
    return [pscustomobject]@{ CodexJs = $codexJs; TargetDirs = $targets }
}

function Set-CodexLauncherEntrypoint {
    # Launcher text with the real codex.js baked in by absolute path. Pure.
    param(
        [AllowEmptyString()][string]$Text = '',
        [ValidateSet('ps1', 'cmd', 'sh')][string]$Kind,
        [AllowEmptyString()][string]$CodexJsPath = ''
    )
    # The path goes inside double quotes in PowerShell, cmd and sh launchers, so
    # characters those expand (", %, $, `) are refused rather than escaped.
    if ([string]::IsNullOrWhiteSpace($CodexJsPath) -or $CodexJsPath -match '["%$`]') {
        throw "SYNAPSE_CODEX_PACKAGE_PATH_UNSAFE path=$CodexJsPath remediation=install Codex under a path without double quotes, %, `$ or backticks"
    }
    $relative = if ($Kind -eq 'cmd') { '"%dp0%\node_modules\@openai\codex\bin\codex.js"' } else { '"$basedir/node_modules/@openai/codex/bin/codex.js"' }
    $absolute = if ($Kind -eq 'sh') { '"' + ($CodexJsPath -replace '\\', '/') + '"' } else { '"' + $CodexJsPath + '"' }
    if (-not $Text.Contains($relative)) {
        throw "SYNAPSE_CODEX_LAUNCHER_TEMPLATE_DRIFT kind=$Kind remediation=the $Kind launcher template no longer calls $relative; update Set-CodexLauncherEntrypoint with the template"
    }
    return $Text.Replace($relative, $absolute)
}

function Install-CodexSynapseTokenLoader {
    param(
        [Parameter(Mandatory=$true)][string]$CodexCommandPath,
        [Parameter(Mandatory=$true)][string]$TokenPath
    )

    $launcherDir = Split-Path -Parent $CodexCommandPath
    if (-not $launcherDir -or -not (Test-Path $launcherDir)) {
        Die "Cannot resolve Codex launcher directory from '$CodexCommandPath'."
    }
    $npmGlobalDir = Join-Path $env:APPDATA 'npm'
    $existingPaths = @(foreach ($dir in @($launcherDir, $npmGlobalDir)) {
        foreach ($leaf in @('codex.ps1', 'codex.cmd', 'codex', 'node_modules\@openai\codex\bin\codex.js')) {
            $candidate = $dir.TrimEnd('\') + '\' + $leaf
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { $candidate }
        }
    })
    $plan = Resolve-CodexLauncherPlan -LauncherDir $launcherDir -NpmGlobalDir $npmGlobalDir -ExistingPaths $existingPaths
    if (-not $plan.CodexJs) {
        Die "SYNAPSE_CODEX_PACKAGE_MISSING launcher=$CodexCommandPath remediation=install Codex with 'npm install -g @openai/codex@latest', then rerun setup."
    }
    foreach ($dir in $plan.TargetDirs) {
        Install-CodexSynapseTokenLoaderInDir -LauncherDir $dir -CodexJsPath $plan.CodexJs -TokenPath $TokenPath
    }

    # Structural readback: the PATH-winning launcher must actually start Codex.
    $versionOut = & $CodexCommandPath --version 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or $versionOut -notmatch 'codex') {
        Die "SYNAPSE_CODEX_LAUNCHER_BROKEN launcher=$CodexCommandPath codex_js=$($plan.CodexJs) exit=$LASTEXITCODE output=$($versionOut.Trim()) remediation=inspect the launcher; restore $CodexCommandPath.synapse-bak if needed"
    }
    Info "Codex launcher verified: $CodexCommandPath -> $($plan.CodexJs) ($($versionOut.Trim()))"
}

function Install-CodexSynapseTokenLoaderInDir {
    param(
        [Parameter(Mandatory=$true)][string]$LauncherDir,
        [Parameter(Mandatory=$true)][string]$CodexJsPath,
        [Parameter(Mandatory=$true)][string]$TokenPath
    )

    $npmDir = $LauncherDir

    $ps1Path = Join-Path $npmDir 'codex.ps1'
    $cmdPath = Join-Path $npmDir 'codex.cmd'
    $shPath = Join-Path $npmDir 'codex'

    if (Test-Path $ps1Path) {
        $ps1 = @'
#!/usr/bin/env pwsh
$basedir=Split-Path $MyInvocation.MyCommand.Definition -Parent

# Synapse MCP token loader: begin
$synapseConfigPath = Join-Path $env:USERPROFILE '.codex\config.toml'
$synapseTokenPath = Join-Path $env:APPDATA 'synapse\token.txt'
$synapseHasConfig = $false
$synapseOwnSnapshot = $null
if (Test-Path $synapseConfigPath) {
  try {
    $synapseHasConfig = ((Get-Content -Raw $synapseConfigPath) -match '(?m)^\[mcp_servers\.synapse\]')
  } catch {
    Write-Error "SYNAPSE_CODEX_CONFIG_UNREADABLE path=$synapseConfigPath remediation=repair Codex config permissions or rerun scripts\synapse-setup.ps1"
    exit 1
  }
}
if ($synapseHasConfig) {
  if (-not (Test-Path $synapseTokenPath)) {
    Write-Error "SYNAPSE_CODEX_TOKEN_MISSING path=$synapseTokenPath remediation=run scripts\synapse-setup.ps1 to generate the bearer token"
    exit 1
  }
  $synapseTokenRaw = Get-Content -Raw $synapseTokenPath
  $synapseToken = if ($null -eq $synapseTokenRaw) { '' } else { $synapseTokenRaw.Trim() }
  if ([string]::IsNullOrWhiteSpace($synapseToken)) {
    Write-Error "SYNAPSE_CODEX_TOKEN_EMPTY path=$synapseTokenPath remediation=delete the empty token and rerun scripts\synapse-setup.ps1"
    exit 1
  }
  if ($env:SYNAPSE_BEARER_TOKEN -ne $synapseToken) {
    $env:SYNAPSE_BEARER_TOKEN = $synapseToken
  }
  $synapseToolSurfacePath = Join-Path $env:APPDATA 'synapse\codex-tool-surface.json'
  if (-not (Test-Path $synapseToolSurfacePath)) {
    Write-Error "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_MISSING path=$synapseToolSurfacePath remediation=run scripts\synapse-setup.ps1 to write the current daemon tools/list fingerprint before starting Codex"
    exit 1
  }
  try {
    $synapseToolSurface = Get-Content -Raw $synapseToolSurfacePath | ConvertFrom-Json
  } catch {
    Write-Error "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_UNREADABLE path=$synapseToolSurfacePath error=$($_.Exception.Message) remediation=repair the snapshot file or rerun scripts\synapse-setup.ps1"
    exit 1
  }
  $synapseToolSurfaceHash = [string]$synapseToolSurface.tool_surface_sha256
  if ([string]::IsNullOrWhiteSpace($synapseToolSurfaceHash)) {
    Write-Error "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_INVALID path=$synapseToolSurfacePath remediation=delete the invalid snapshot and rerun scripts\synapse-setup.ps1"
    exit 1
  }
  $synapseStartSnapshotDir = Join-Path $env:LOCALAPPDATA 'synapse\codex-start-snapshots'
  # #259: o<owner PID>-<GUID>. The owner is this PowerShell process; it deletes the file when Codex exits.
  $synapseStartSnapshotPath = Join-Path $synapseStartSnapshotDir ("codex-tool-surface-o{0}-{1}.json" -f $PID, [Guid]::NewGuid().ToString('N'))
  try {
    New-Item -ItemType Directory -Force -Path $synapseStartSnapshotDir | Out-Null
    # #259: CreateNew never overwrites another start's snapshot.
    $synapseStartSnapshotBytes = [System.IO.File]::ReadAllBytes($synapseToolSurfacePath)
    $synapseStartSnapshotStream = [System.IO.File]::Open($synapseStartSnapshotPath, 'CreateNew', 'Write', 'Read')
    try { $synapseStartSnapshotStream.Write($synapseStartSnapshotBytes, 0, $synapseStartSnapshotBytes.Length) } finally { $synapseStartSnapshotStream.Dispose() }
  } catch {
    Write-Error "SYNAPSE_CODEX_TOOL_SURFACE_START_SNAPSHOT_FAILED path=$synapseStartSnapshotPath error=$($_.Exception.Message) remediation=repair permissions on %LOCALAPPDATA%\synapse\codex-start-snapshots before starting Codex"
    exit 1
  }
  $synapseOwnSnapshot = $synapseStartSnapshotPath
  # #259: best-effort prune of snapshots other starts left behind (crash, killed window).
  # Candidates are enumerated BEFORE liveness is read, so a start that appears meanwhile is
  # never judged against an older process table. Same rule in the cmd and sh launchers:
  #   o<pid>-<id>.json: delete only if the process lookup succeeded, <pid> is gone, and the
  #     file is older than SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES.
  #   legacy <n>-... names (no owner): delete only if older than SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS.
  #   anything else, a failed lookup, or an unset limit: keep.
  try {
    $synapseKeepPath = [System.IO.Path]::GetFullPath($synapseStartSnapshotPath)
    $synapseCandidates = @(Get-ChildItem -LiteralPath $synapseStartSnapshotDir -Filter 'codex-tool-surface-*.json' -File -ErrorAction Stop | Where-Object { $_.FullName -ne $synapseKeepPath })
    $synapseLive = $null
    try {
      $synapseLive = @{}
      foreach ($synapseProc in @(Get-CimInstance Win32_Process -Property ProcessId -ErrorAction Stop)) { $synapseLive[[long]$synapseProc.ProcessId] = $true }
      if ($synapseLive.Count -eq 0) { $synapseLive = $null }
    } catch { $synapseLive = $null }
    $synapseGraceMinutes = 0
    [void][int]::TryParse([string]$env:SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES, [ref]$synapseGraceMinutes)
    $synapseRetentionHours = 0
    [void][int]::TryParse([string]$env:SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS, [ref]$synapseRetentionHours)
    $synapseNow = [DateTime]::UtcNow
    foreach ($synapseOld in $synapseCandidates) {
      $synapseAge = $synapseNow - $synapseOld.LastWriteTimeUtc
      $synapseTail = $synapseOld.Name.Substring(19)
      $synapseDrop = $false
      if ($synapseTail -match '\Ao(\d{1,10})-[0-9a-f]+\.json\z') {
        $synapseDrop = $null -ne $synapseLive -and $synapseGraceMinutes -gt 0 -and $synapseAge.TotalMinutes -gt $synapseGraceMinutes -and -not $synapseLive.ContainsKey([long]$Matches[1])
      } elseif ($synapseTail -match '\A\d') {
        $synapseDrop = $synapseRetentionHours -gt 0 -and $synapseAge.TotalHours -gt $synapseRetentionHours
      }
      if ($synapseDrop) { try { $synapseOld.Delete() } catch { } }
    }
  } catch { }
  $env:SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START = $synapseToolSurfaceHash
  $env:SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START = [string]$synapseToolSurface.tool_count
  $env:SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START = $synapseStartSnapshotPath
}
Remove-Variable synapseConfigPath,synapseTokenPath,synapseHasConfig -ErrorAction SilentlyContinue
Remove-Variable synapseTokenRaw,synapseToken -ErrorAction SilentlyContinue
Remove-Variable synapseToolSurfacePath,synapseToolSurface,synapseToolSurfaceHash -ErrorAction SilentlyContinue
Remove-Variable synapseStartSnapshotDir,synapseStartSnapshotPath,synapseStartSnapshotBytes,synapseStartSnapshotStream -ErrorAction SilentlyContinue
Remove-Variable synapseKeepPath,synapseCandidates,synapseLive,synapseProc,synapseGraceMinutes,synapseRetentionHours -ErrorAction SilentlyContinue
Remove-Variable synapseNow,synapseOld,synapseAge,synapseTail,synapseDrop -ErrorAction SilentlyContinue
# Synapse MCP token loader: end

$exe=""
if ($PSVersionTable.PSVersion -lt "6.0" -or $IsWindows) {
  # Fix case when both the Windows and Linux builds of Node
  # are installed in the same directory
  $exe=".exe"
}
$ret=0
try {
  if (Test-Path "$basedir/node$exe") {
    # Support pipeline input
    if ($MyInvocation.ExpectingInput) {
      $input | & "$basedir/node$exe"  "$basedir/node_modules/@openai/codex/bin/codex.js" $args
    } else {
      & "$basedir/node$exe"  "$basedir/node_modules/@openai/codex/bin/codex.js" $args
    }
    $ret=$LASTEXITCODE
  } else {
    # Support pipeline input
    if ($MyInvocation.ExpectingInput) {
      $input | & "node$exe"  "$basedir/node_modules/@openai/codex/bin/codex.js" $args
    } else {
      & "node$exe"  "$basedir/node_modules/@openai/codex/bin/codex.js" $args
    }
    $ret=$LASTEXITCODE
  }
} finally {
  # #259: this start owns its snapshot; remove it on every exit path (normal, error, Ctrl+C).
  if ($synapseOwnSnapshot) { Remove-Item -LiteralPath $synapseOwnSnapshot -Force -ErrorAction SilentlyContinue }
}
exit $ret
'@
        $ps1 = Set-CodexLauncherEntrypoint -Text $ps1 -Kind 'ps1' -CodexJsPath $CodexJsPath
        Copy-Item $ps1Path "$ps1Path.synapse-bak" -Force
        Set-Content -Path $ps1Path -Value $ps1 -Encoding utf8
        Info "Installed Synapse token loader in Codex PowerShell launcher: $ps1Path"
    } else {
        Info "WARN: Codex PowerShell launcher not found at $ps1Path; cannot install ps1 token loader."
    }

    if (Test-Path $cmdPath) {
        $cmd = @'
@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL EnableExtensions EnableDelayedExpansion
CALL :find_dp0

REM Synapse MCP token loader: begin
SET "_synapse_cfg=%USERPROFILE%\.codex\config.toml"
SET "_synapse_tok=%APPDATA%\synapse\token.txt"
SET "_synapse_surface=%APPDATA%\synapse\codex-tool-surface.json"
SET "_synapse_has_cfg="
SET "_synapse_own_snapshot="
IF EXIST "%_synapse_cfg%" (
  %SystemRoot%\System32\findstr.exe /R /C:"^\[mcp_servers\.synapse\]" "%_synapse_cfg%" >NUL 2>NUL
  IF NOT ERRORLEVEL 1 SET "_synapse_has_cfg=1"
)
IF DEFINED _synapse_has_cfg (
  IF NOT EXIST "%_synapse_tok%" (
    ECHO SYNAPSE_CODEX_TOKEN_MISSING path=%_synapse_tok% remediation=run scripts\synapse-setup.ps1 to generate the bearer token 1>&2
    EXIT /B 1
  )
  SET /P _synapse_file_token=<"%_synapse_tok%"
  IF NOT DEFINED _synapse_file_token (
    ECHO SYNAPSE_CODEX_TOKEN_EMPTY path=%_synapse_tok% remediation=delete the empty token and rerun scripts\synapse-setup.ps1 1>&2
    EXIT /B 1
  )
  IF NOT "%SYNAPSE_BEARER_TOKEN%"=="!_synapse_file_token!" SET "SYNAPSE_BEARER_TOKEN=!_synapse_file_token!"
  IF NOT EXIST "%_synapse_surface%" (
    ECHO SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_MISSING path=%_synapse_surface% remediation=run scripts\synapse-setup.ps1 to write the current daemon tools/list fingerprint before starting Codex 1>&2
    EXIT /B 1
  )
  SET "_synapse_surface_hash="
  SET "_synapse_surface_count="
  FOR /F "tokens=2 delims=:" %%A IN ('%SystemRoot%\System32\findstr.exe /C:tool_surface_sha256 "%_synapse_surface%"') DO SET "_synapse_surface_hash=%%~A"
  FOR /F "tokens=2 delims=:" %%A IN ('%SystemRoot%\System32\findstr.exe /C:tool_count "%_synapse_surface%"') DO SET "_synapse_surface_count=%%~A"
  SET "_synapse_surface_hash=!_synapse_surface_hash:"=!"
  SET "_synapse_surface_hash=!_synapse_surface_hash:,=!"
  SET "_synapse_surface_hash=!_synapse_surface_hash: =!"
  SET "_synapse_surface_count=!_synapse_surface_count:"=!"
  SET "_synapse_surface_count=!_synapse_surface_count:,=!"
  SET "_synapse_surface_count=!_synapse_surface_count: =!"
  IF NOT DEFINED _synapse_surface_hash (
    ECHO SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_INVALID path=%_synapse_surface% remediation=delete the invalid snapshot and rerun scripts\synapse-setup.ps1 1>&2
    EXIT /B 1
  )
  SET "_synapse_start_dir=%LOCALAPPDATA%\synapse\codex-start-snapshots"
  SET "_synapse_start_surface="
  IF NOT EXIST "!_synapse_start_dir!" MD "!_synapse_start_dir!" >NUL 2>NUL
  IF NOT EXIST "!_synapse_start_dir!" (
    ECHO SYNAPSE_CODEX_TOOL_SURFACE_START_SNAPSHOT_FAILED path=!_synapse_start_dir! remediation=repair permissions on %LOCALAPPDATA%\synapse\codex-start-snapshots before starting Codex 1>&2
    EXIT /B 1
  )
  REM #259: name = o + wrapper cmd PID + GUID, written with CreateNew by a PS 5.1 helper that prints the path.
  REM The wrapper deletes its own snapshot after Codex exits. The helper prunes only proven orphans:
  REM files enumerated first, then o-PID owners proven gone by a successful lookup and older than
  REM SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES; legacy names only past SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS.
  SET "_synapse_start_ps=$ErrorActionPreference='Stop'; $dir=$env:_synapse_start_dir; $tag='u'"
  SET "_synapse_start_ps=!_synapse_start_ps!; try { $self=Get-CimInstance Win32_Process -Filter ('ProcessId=' + $PID); $for=Get-CimInstance Win32_Process -Filter ('ProcessId=' + $self.ParentProcessId); if ($for.ParentProcessId) { $tag='o' + $for.ParentProcessId } } catch { $tag='u' }"
  SET "_synapse_start_ps=!_synapse_start_ps!; $path=Join-Path $dir ('codex-tool-surface-{0}-{1}.json' -f $tag,[guid]::NewGuid().ToString('N')); $bytes=[IO.File]::ReadAllBytes($env:_synapse_surface)"
  SET "_synapse_start_ps=!_synapse_start_ps!; $out=[IO.File]::Open($path,'CreateNew','Write','Read'); try { $out.Write($bytes,0,$bytes.Length) } finally { $out.Dispose() }"
  SET "_synapse_start_ps=!_synapse_start_ps!; try { $keep=[IO.Path]::GetFullPath($path); $files=@(Get-ChildItem -LiteralPath $dir -Filter 'codex-tool-surface-*.json' -File | Where-Object { $_.FullName -ne $keep })"
  SET "_synapse_start_ps=!_synapse_start_ps!; $live=$null; try { $live=@{}; foreach ($p in @(Get-CimInstance Win32_Process -Property ProcessId)) { $live[[long]$p.ProcessId]=$true }; if ($live.Count -eq 0) { $live=$null } } catch { $live=$null }"
  SET "_synapse_start_ps=!_synapse_start_ps!; $grace=0; [void][int]::TryParse([string]$env:SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES,[ref]$grace); $hours=0; [void][int]::TryParse([string]$env:SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS,[ref]$hours); $now=[DateTime]::UtcNow"
  SET "_synapse_start_ps=!_synapse_start_ps!; foreach ($f in $files) { $age=$now - $f.LastWriteTimeUtc; $tail=$f.Name.Substring(19); $drop=$false"
  SET "_synapse_start_ps=!_synapse_start_ps!; if ($tail -match '\Ao(\d{1,10})-[0-9a-f]+\.json\z') { $drop=$null -ne $live -and $grace -gt 0 -and $age.TotalMinutes -gt $grace -and -not $live.ContainsKey([long]$Matches[1]) } elseif ($tail -match '\A\d') { $drop=$hours -gt 0 -and $age.TotalHours -gt $hours }"
  SET "_synapse_start_ps=!_synapse_start_ps!; if ($drop) { try { $f.Delete() } catch { } } } } catch { }; $path"
  FOR /F "usebackq delims=" %%P IN (`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "& ([ScriptBlock]::Create($env:_synapse_start_ps))"`) DO SET "_synapse_start_surface=%%P"
  SET "_synapse_start_ps="
  IF NOT DEFINED _synapse_start_surface (
    ECHO SYNAPSE_CODEX_TOOL_SURFACE_START_SNAPSHOT_FAILED path=!_synapse_start_dir! remediation=repair permissions on %LOCALAPPDATA%\synapse\codex-start-snapshots before starting Codex 1>&2
    EXIT /B 1
  )
  IF NOT EXIST "!_synapse_start_surface!" (
    ECHO SYNAPSE_CODEX_TOOL_SURFACE_START_SNAPSHOT_FAILED path=!_synapse_start_surface! remediation=repair permissions on %LOCALAPPDATA%\synapse\codex-start-snapshots before starting Codex 1>&2
    EXIT /B 1
  )
  SET "SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START=!_synapse_surface_hash!"
  SET "SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START=!_synapse_surface_count!"
  SET "SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START=!_synapse_start_surface!"
  SET "_synapse_own_snapshot=!_synapse_start_surface!"
)
SET "_synapse_cfg="
SET "_synapse_tok="
SET "_synapse_surface="
SET "_synapse_has_cfg="
SET "_synapse_file_token="
SET "_synapse_surface_hash="
SET "_synapse_surface_count="
SET "_synapse_start_dir="
SET "_synapse_start_surface="
SET "_synapse_start_ps="
REM Synapse MCP token loader: end
REM #259: the CALLed child deletes this start's own snapshot after Codex exits and re-exits with Codex's code.

IF EXIST "%dp0%\node.exe" (
  SET "_prog=%dp0%\node.exe"
) ELSE (
  SET "_prog=node"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)

endLocal & SET "SYNAPSE_BEARER_TOKEN=%SYNAPSE_BEARER_TOKEN%" & SET "SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START=%SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START%" & SET "SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START=%SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START%" & SET "SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START=%SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START%" & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %* & CALL %COMSPEC% /d /c "(IF NOT "%_synapse_own_snapshot%"=="" DEL /F /Q "%_synapse_own_snapshot%" >NUL 2>NUL) & EXIT %%ERRORLEVEL%%"
'@
        $cmd = Set-CodexLauncherEntrypoint -Text $cmd -Kind 'cmd' -CodexJsPath $CodexJsPath
        Copy-Item $cmdPath "$cmdPath.synapse-bak" -Force
        Set-Content -Path $cmdPath -Value $cmd -Encoding ascii
        Info "Installed Synapse token loader in Codex CMD launcher: $cmdPath"
    } else {
        Info "WARN: Codex CMD launcher not found at $cmdPath; cannot install cmd token loader."
    }

    if (Test-Path $shPath) {
        $sh = @'
#!/bin/sh
basedir=$(dirname "$(echo "$0" | sed -e 's,\\,/,g')")

# Synapse MCP token loader: begin
synapse_cfg="$USERPROFILE/.codex/config.toml"
synapse_tok="$APPDATA/synapse/token.txt"
case `uname` in
    *CYGWIN*|*MINGW*|*MSYS*)
        if command -v cygpath > /dev/null 2>&1; then
            synapse_cfg=$(cygpath -u "$synapse_cfg")
            synapse_tok=$(cygpath -u "$synapse_tok")
        fi
    ;;
esac
if [ -f "$synapse_cfg" ] && grep -Eq '^\[mcp_servers\.synapse\]' "$synapse_cfg"; then
    if [ ! -r "$synapse_tok" ]; then
        printf '%s\n' "SYNAPSE_CODEX_TOKEN_MISSING path=$synapse_tok remediation=run scripts/synapse-setup.ps1 to generate the bearer token" >&2
        exit 1
    fi
    synapse_file_token=$(tr -d '\r\n' < "$synapse_tok")
    if [ -z "$synapse_file_token" ]; then
        printf '%s\n' "SYNAPSE_CODEX_TOKEN_EMPTY path=$synapse_tok remediation=delete the empty token and rerun scripts/synapse-setup.ps1" >&2
        exit 1
    fi
    if [ "${SYNAPSE_BEARER_TOKEN:-}" != "$synapse_file_token" ]; then
        SYNAPSE_BEARER_TOKEN="$synapse_file_token"
        export SYNAPSE_BEARER_TOKEN
    fi
    synapse_surface="$APPDATA/synapse/codex-tool-surface.json"
    if [ ! -r "$synapse_surface" ]; then
        printf '%s\n' "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_MISSING path=$synapse_surface remediation=run scripts/synapse-setup.ps1 to write the current daemon tools/list fingerprint before starting Codex" >&2
        exit 1
    fi
    synapse_surface_hash=$(sed -n 's/.*"tool_surface_sha256"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F][0-9a-fA-F]*\)".*/\1/p' "$synapse_surface" | head -n 1)
    synapse_surface_count=$(sed -n 's/.*"tool_count"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$synapse_surface" | head -n 1)
    if [ -z "$synapse_surface_hash" ]; then
        printf '%s\n' "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_INVALID path=$synapse_surface remediation=delete the invalid snapshot and rerun scripts/synapse-setup.ps1" >&2
        exit 1
    fi
    synapse_start_dir="$LOCALAPPDATA/synapse/codex-start-snapshots"
    # #259: o<this shell's Windows PID>-<random>. The shell stays alive while node runs (no exec)
    # and its EXIT trap deletes the snapshot. No Windows PID -> "u" (owner unknown, never pruned).
    synapse_owner_pid=$(cat "/proc/$$/winpid" 2>/dev/null)
    case "$synapse_owner_pid" in
        ''|*[!0-9]*) synapse_owner_tag="u" ;;
        *) synapse_owner_tag="o$synapse_owner_pid" ;;
    esac
    synapse_start_nonce=$(od -An -N16 -tx1 /dev/urandom 2>/dev/null | tr -d ' \n')
    [ -n "$synapse_start_nonce" ] || synapse_start_nonce="$(date +%s)$$"
    synapse_start_surface="$synapse_start_dir/codex-tool-surface-$synapse_owner_tag-$synapse_start_nonce.json"
    if ! mkdir -p "$synapse_start_dir" || ! (set -C; cat "$synapse_surface" > "$synapse_start_surface") 2>/dev/null; then
        printf '%s\n' "SYNAPSE_CODEX_TOOL_SURFACE_START_SNAPSHOT_FAILED path=$synapse_start_surface remediation=repair permissions on $synapse_start_dir before starting Codex" >&2
        exit 1
    fi
    synapse_own_snapshot="$synapse_start_surface"
    trap 'rm -f -- "$synapse_own_snapshot"' EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    # #259: best-effort prune, same rule as the cmd/ps1 launchers: enumerate first, then read
    # liveness; o<pid> only when the lookup succeeded, <pid> is gone and the file is older than
    # the grace period; legacy names only past the retention; everything else is kept.
    if command -v powershell.exe > /dev/null 2>&1; then
        _synapse_start_dir="$synapse_start_dir" _synapse_start_keep="$synapse_start_surface" powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command '
$keep = [IO.Path]::GetFullPath($env:_synapse_start_keep)
$files = @(Get-ChildItem -LiteralPath $env:_synapse_start_dir -Filter "codex-tool-surface-*.json" -File -ErrorAction Stop | Where-Object { $_.FullName -ne $keep })
$live = $null
try { $live = @{}; foreach ($p in @(Get-CimInstance Win32_Process -Property ProcessId -ErrorAction Stop)) { $live[[long]$p.ProcessId] = $true }; if ($live.Count -eq 0) { $live = $null } } catch { $live = $null }
$grace = 0; [void][int]::TryParse([string]$env:SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES, [ref]$grace)
$hours = 0; [void][int]::TryParse([string]$env:SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS, [ref]$hours)
$now = [DateTime]::UtcNow
foreach ($f in $files) {
  $age = $now - $f.LastWriteTimeUtc; $tail = $f.Name.Substring(19); $drop = $false
  if ($tail -match "\Ao(\d{1,10})-[0-9a-f]+\.json\z") {
    $drop = $null -ne $live -and $grace -gt 0 -and $age.TotalMinutes -gt $grace -and -not $live.ContainsKey([long]$Matches[1])
  } elseif ($tail -match "\A\d") {
    $drop = $hours -gt 0 -and $age.TotalHours -gt $hours
  }
  if ($drop) { try { $f.Delete() } catch { } }
}' > /dev/null 2>&1 || true
    fi
    SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START="$synapse_surface_hash"
    SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START="$synapse_surface_count"
    SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START="$synapse_start_surface"
    export SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START
fi
unset synapse_cfg synapse_tok synapse_file_token synapse_surface synapse_surface_hash synapse_surface_count synapse_start_dir synapse_start_surface synapse_owner_pid synapse_owner_tag synapse_start_nonce
# Synapse MCP token loader: end

case `uname` in
    *CYGWIN*|*MINGW*|*MSYS*)
        if command -v cygpath > /dev/null 2>&1; then
            basedir=`cygpath -w "$basedir"`
        fi
    ;;
esac

# #259: run node as a child (not exec) so the EXIT trap can delete this start's snapshot.
if [ -x "$basedir/node" ]; then
  "$basedir/node"  "$basedir/node_modules/@openai/codex/bin/codex.js" "$@"
else
  node  "$basedir/node_modules/@openai/codex/bin/codex.js" "$@"
fi
exit $?
'@
        $sh = Set-CodexLauncherEntrypoint -Text $sh -Kind 'sh' -CodexJsPath $CodexJsPath
        Copy-Item $shPath "$shPath.synapse-bak" -Force
        $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::WriteAllText($shPath, ($sh -replace "`r?`n", "`n"), $utf8NoBom)
        Info "Installed Synapse token loader in Codex shell launcher: $shPath"
    } else {
        Info "WARN: Codex shell launcher not found at $shPath; cannot install shell token loader."
    }

    $loaderTokenRaw = if (Test-Path $TokenPath) { Get-Content -Raw $TokenPath } else { $null }
    $loaderToken = if ($null -eq $loaderTokenRaw) { '' } else { $loaderTokenRaw.Trim() }
    if ((Test-Path $TokenPath) -and [string]::IsNullOrWhiteSpace($loaderToken)) {
        Die "Installed Codex token loaders, but token at $TokenPath is empty."
    }
}

function Test-CodexSynapseHttpConfig {
    param(
        [Parameter(Mandatory=$true)][string]$ConfigPath,
        [Parameter(Mandatory=$true)][string]$Bind
    )

    $body = Get-CodexSynapseConfigBody -ConfigPath $ConfigPath
    if ($null -eq $body) {
        return $false
    }
    return (Test-CodexSynapseHttpConfigBody -Body $body -Bind $Bind)
}

function Test-CodexSynapseHttpConfigBody {
    # Does a [mcp_servers.synapse] section body carry the required client policy? Pure.
    param(
        [AllowEmptyString()][string]$Body = '',
        [Parameter(Mandatory=$true)][string]$Bind
    )
    $bindUrlRegex = [regex]::Escape("http://$Bind/mcp")
    return ($Body -match "url\s*=\s*`"$bindUrlRegex`"" -and
        $Body -match 'bearer_token_env_var\s*=\s*"SYNAPSE_BEARER_TOKEN"' -and
        $Body -match '(?m)^\s*required\s*=\s*false\s*$' -and
        $Body -match '(?m)^\s*default_tools_approval_mode\s*=\s*"approve"\s*$')
}

function Test-CodexSynapseHttpTransportConfig {
    param(
        [Parameter(Mandatory=$true)][string]$ConfigPath,
        [Parameter(Mandatory=$true)][string]$Bind
    )

    $body = Get-CodexSynapseConfigBody -ConfigPath $ConfigPath
    if ($null -eq $body) {
        return $false
    }
    $bindUrlRegex = [regex]::Escape("http://$Bind/mcp")
    return ($body -match "url\s*=\s*`"$bindUrlRegex`"" -and
        $body -match 'bearer_token_env_var\s*=\s*"SYNAPSE_BEARER_TOKEN"')
}

function Get-CodexSynapseConfigBody {
    param(
        [Parameter(Mandatory=$true)][string]$ConfigPath
    )

    if (-not (Test-Path $ConfigPath)) {
        return $null
    }
    try {
        $content = Get-Content -Raw $ConfigPath
    } catch {
        return $null
    }
    $section = [regex]::Match(
        $content,
        '(?ms)^\[mcp_servers\.synapse\]\s*(?<body>.*?)(?=^\[|\z)'
    )
    if (-not $section.Success) {
        return $null
    }
    return [string]$section.Groups['body'].Value
}

function Set-CodexSynapseClientPolicy {
    param(
        [Parameter(Mandatory=$true)][string]$ConfigPath,
        [Parameter(Mandatory=$true)][string]$Bind
    )

    $configDir = Split-Path -Parent $ConfigPath
    if (-not (Test-Path $configDir)) {
        [System.IO.Directory]::CreateDirectory($configDir) | Out-Null
    }

    $content = ''
    if (Test-Path $ConfigPath) {
        $content = Get-Content -Raw $ConfigPath
    }

    $content = Update-CodexSynapseSectionText -Content $content -Bind $Bind

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($ConfigPath, $content, $utf8NoBom)
}

function Update-CodexSynapseSectionText {
    # The [mcp_servers.synapse] section policy applied to config.toml text. Pure;
    # every other section stays byte-identical.
    param(
        [AllowEmptyString()][string]$Content = '',
        [Parameter(Mandatory=$true)][string]$Bind
    )
    # ($content below is this parameter: PowerShell names are case-insensitive.)
    $desiredLines = @(
        ('url = "http://{0}/mcp"' -f $Bind),
        'bearer_token_env_var = "SYNAPSE_BEARER_TOKEN"',
        # #2269: a daemon outage must stay an MCP error, never stop Codex from opening.
        'required = false',
        'startup_timeout_sec = 20',
        'default_tools_approval_mode = "approve"'
    )
    $sectionRegex = '(?ms)^\[mcp_servers\.synapse\]\s*(?<body>.*?)(?=^\[|\z)'
    $section = [regex]::Match($content, $sectionRegex)

    if ($section.Success) {
        $body = [string]$section.Groups['body'].Value
        $preserved = @()
        foreach ($line in ($body -split "`r?`n")) {
            if ($line -match '^\s*(url|bearer_token_env_var|required|startup_timeout_sec|startup_timeout_ms|default_tools_approval_mode)\s*=') {
                continue
            }
            if ([string]::IsNullOrWhiteSpace($line) -and $preserved.Count -eq 0) {
                continue
            }
            $preserved += $line
        }
        while ($preserved.Count -gt 0 -and [string]::IsNullOrWhiteSpace($preserved[$preserved.Count - 1])) {
            if ($preserved.Count -eq 1) {
                $preserved = @()
            } else {
                $preserved = @($preserved[0..($preserved.Count - 2)])
            }
        }
        $newSectionLines = @('[mcp_servers.synapse]') + $desiredLines
        if ($preserved.Count -gt 0) {
            $newSectionLines += $preserved
        }
        $newSection = ($newSectionLines -join "`r`n") + "`r`n"
        $content = $content.Substring(0, $section.Index) + $newSection + $content.Substring($section.Index + $section.Length)
    } else {
        if (-not [string]::IsNullOrEmpty($content) -and -not $content.EndsWith("`n")) {
            $content += "`r`n"
        }
        if (-not [string]::IsNullOrWhiteSpace($content)) {
            $content += "`r`n"
        }
        $content += ((@('[mcp_servers.synapse]') + $desiredLines) -join "`r`n") + "`r`n"
    }
    return $content
}

function Get-SynapseMcpProcessSnapshot {
    @(Get-CimInstance Win32_Process -Filter "Name='synapse-mcp.exe'" -ErrorAction SilentlyContinue |
        Sort-Object ProcessId |
        Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine)
}

function Format-SynapseMcpProcessSnapshot {
    param([object[]]$Snapshot)
    if (-not $Snapshot -or $Snapshot.Count -eq 0) {
        return '<none>'
    }
    return (($Snapshot | ForEach-Object {
        $matchRules = if ($_.PSObject.Properties.Name -contains 'DeployTargetRules') { $_.DeployTargetRules } else { '<unclassified>' }
        $bindArg = if ($_.PSObject.Properties.Name -contains 'DeployTargetBindArg') { $_.DeployTargetBindArg } else { '<unclassified>' }
        $dbArg = if ($_.PSObject.Properties.Name -contains 'DeployTargetDbArg') { $_.DeployTargetDbArg } else { '<unclassified>' }
        "pid=$($_.ProcessId) ppid=$($_.ParentProcessId) path=$($_.ExecutablePath) target_match=$matchRules bind_arg=$bindArg db_arg=$dbArg cmd=$($_.CommandLine)"
    }) -join "`n")
}

function Normalize-SynapseSetupPathForCompare {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return '' }
    try {
        $full = [System.IO.Path]::GetFullPath($Path)
    } catch {
        $full = $Path.Trim()
    }
    return $full.TrimEnd([char[]]@([char]92, [char]47))
}

function Get-SynapseCommandLineArgumentValue {
    param(
        [string]$CommandLine,
        [Parameter(Mandatory=$true)][string]$Name
    )
    if ([string]::IsNullOrWhiteSpace($CommandLine)) { return $null }
    $escapedName = [regex]::Escape($Name)
    $pattern = "(?i)(?:^|\s)$escapedName(?:\s+|=)(?:""(?<quoted>[^""]*)""|(?<bare>\S+))"
    $match = [regex]::Match($CommandLine, $pattern)
    if (-not $match.Success) { return $null }
    if ($match.Groups['quoted'].Success) { return $match.Groups['quoted'].Value }
    return $match.Groups['bare'].Value
}

function Get-SynapseMcpDeployTargetMatch {
    param(
        [Parameter(Mandatory=$true)]$Process,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath
    )

    $rules = @()
    $bindArg = Get-SynapseCommandLineArgumentValue -CommandLine $Process.CommandLine -Name '--bind'
    $dbArgForBridge = Get-SynapseCommandLineArgumentValue -CommandLine $Process.CommandLine -Name '--db'
    $modeArg = Get-SynapseCommandLineArgumentValue -CommandLine $Process.CommandLine -Name '--mode'
    if (-not [string]::IsNullOrWhiteSpace($modeArg) -and $modeArg.Trim() -ieq 'connect') {
        # Issue #11: a `--mode connect` stdio bridge belongs to a live MCP client
        # (Codex/Claude) that cannot respawn it. It is never a deploy stop target;
        # it rides its HTTP reconnect loop across the daemon replacement.
        return [pscustomobject]@{
            IsMatch = $false
            Rules = @('connect_bridge_preserved')
            BindArg = if ($null -eq $bindArg) { '<missing>' } else { $bindArg }
            DbArg = if ($null -eq $dbArgForBridge) { '<missing>' } else { $dbArgForBridge }
            ExpectedDb = Normalize-SynapseSetupPathForCompare -Path $DbPath
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($bindArg) -and $bindArg.Trim() -ieq $Bind) {
        $rules += "bind=$Bind"
    }

    $expectedDb = Normalize-SynapseSetupPathForCompare -Path $DbPath
    $dbArg = Get-SynapseCommandLineArgumentValue -CommandLine $Process.CommandLine -Name '--db'
    $actualDb = Normalize-SynapseSetupPathForCompare -Path $dbArg
    if (-not [string]::IsNullOrWhiteSpace($actualDb) -and $actualDb -ieq $expectedDb) {
        $rules += "db=$expectedDb"
    }

    [pscustomobject]@{
        IsMatch = ($rules.Count -gt 0)
        Rules = $rules
        BindArg = if ($null -eq $bindArg) { '<missing>' } else { $bindArg }
        DbArg = if ($null -eq $dbArg) { '<missing>' } else { $dbArg }
        ExpectedDb = $expectedDb
    }
}

function Add-SynapseMcpDeployTargetMetadata {
    param(
        [Parameter(Mandatory=$true)]$Process,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath
    )

    $match = Get-SynapseMcpDeployTargetMatch -Process $Process -Bind $Bind -DbPath $DbPath
    $rules = if ($match.Rules.Count -gt 0) { $match.Rules -join ',' } else { '<none>' }
    $Process | Add-Member -NotePropertyName DeployTargetMatched -NotePropertyValue $match.IsMatch -Force
    $Process | Add-Member -NotePropertyName DeployTargetRules -NotePropertyValue $rules -Force
    $Process | Add-Member -NotePropertyName DeployTargetBindArg -NotePropertyValue $match.BindArg -Force
    $Process | Add-Member -NotePropertyName DeployTargetDbArg -NotePropertyValue $match.DbArg -Force
    return $Process
}

function Select-SynapseMcpDeployTargetProcesses {
    param(
        [object[]]$Snapshot,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath,
        [switch]$Invert
    )

    @($Snapshot | ForEach-Object {
        $process = Add-SynapseMcpDeployTargetMetadata -Process $_ -Bind $Bind -DbPath $DbPath
        if ($Invert) {
            if (-not $process.DeployTargetMatched) { $process }
        } else {
            if ($process.DeployTargetMatched) { $process }
        }
    })
}

function Get-SynapseBindEndpoint {
    param([Parameter(Mandatory=$true)][string]$Bind)

    $lastColon = $Bind.LastIndexOf(':')
    if ($lastColon -lt 1 -or $lastColon -eq ($Bind.Length - 1)) {
        Die "SYNAPSE_BIND_PARSE_FAILED bind=$Bind remediation=use host:port, for example 127.0.0.1:7700"
    }

    $address = $Bind.Substring(0, $lastColon)
    $portText = $Bind.Substring($lastColon + 1)
    $port = 0
    if (-not [int]::TryParse($portText, [ref]$port) -or $port -lt 1 -or $port -gt 65535) {
        Die "SYNAPSE_BIND_PARSE_FAILED bind=$Bind port=$portText remediation=use a TCP port from 1 through 65535"
    }

    [pscustomobject]@{ Address = $address; Port = $port }
}

function Test-SynapseBindAvailable {
    param(
        [Parameter(Mandatory=$true)][string]$Bind
    )

    $endpoint = Get-SynapseBindEndpoint -Bind $Bind
    $listener = $null
    try {
        $ipAddress = [System.Net.IPAddress]::Parse($endpoint.Address)
        $listener = [System.Net.Sockets.TcpListener]::new($ipAddress, [int]$endpoint.Port)
        $listener.Start()
        return [pscustomobject]@{ Ok = $true; Error = $null }
    } catch {
        return [pscustomobject]@{ Ok = $false; Error = $_.Exception.Message }
    } finally {
        if ($null -ne $listener) {
            try { $listener.Stop() } catch { }
        }
    }
}

function Get-SynapseTcpClientSnapshot {
    param([Parameter(Mandatory=$true)][string]$Bind)

    $endpoint = Get-SynapseBindEndpoint -Bind $Bind
    $allTcp = @(Get-NetTCPConnection -ErrorAction SilentlyContinue)
    $serverConnections = @($allTcp |
        Where-Object {
            $_.LocalAddress -eq $endpoint.Address -and
            $_.LocalPort -eq $endpoint.Port -and
            "$($_.State)" -ne 'Listen'
        } |
        Sort-Object LocalPort, RemotePort, OwningProcess)

    foreach ($connection in $serverConnections) {
        $peer = @($allTcp | Where-Object {
            $_.LocalAddress -eq $connection.RemoteAddress -and
            $_.LocalPort -eq $connection.RemotePort -and
            $_.RemoteAddress -eq $connection.LocalAddress -and
            $_.RemotePort -eq $connection.LocalPort
        } | Select-Object -First 1)
        $peerOwnerPid = if ($peer.Count -gt 0) { [int]$peer[0].OwningProcess } else { 0 }
        $peerOwner = if ($peerOwnerPid -gt 0) {
            Get-CimInstance Win32_Process -Filter "ProcessId=$peerOwnerPid" -ErrorAction SilentlyContinue
        } else {
            $null
        }
        $peerOwnerExists = ($null -ne $peerOwner)
        [pscustomobject]@{
            State = $connection.State
            LocalAddress = $connection.LocalAddress
            LocalPort = $connection.LocalPort
            RemoteAddress = $connection.RemoteAddress
            RemotePort = $connection.RemotePort
            OwningProcess = $connection.OwningProcess
            OwnerName = (Get-Process -Id $connection.OwningProcess -ErrorAction SilentlyContinue).ProcessName
            OwnerCommandLine = (Get-CimInstance Win32_Process -Filter "ProcessId=$($connection.OwningProcess)" -ErrorAction SilentlyContinue).CommandLine
            PeerOwningProcess = $peerOwnerPid
            PeerOwnerExists = $peerOwnerExists
            PeerOwnerName = $peerOwner.Name
            PeerOwnerCommandLine = $peerOwner.CommandLine
            HasLivePeer = $peerOwnerExists
        }
    }
}

function Get-SynapseTcpBindListenerSnapshot {
    param([Parameter(Mandatory=$true)][string]$Bind)

    $endpoint = Get-SynapseBindEndpoint -Bind $Bind
    $listeners = @(Get-NetTCPConnection -LocalAddress $endpoint.Address -LocalPort $endpoint.Port -State Listen -ErrorAction SilentlyContinue |
        Sort-Object LocalAddress, LocalPort, OwningProcess)

    foreach ($listener in $listeners) {
        $owner = if ($listener.OwningProcess -gt 0) {
            Get-CimInstance Win32_Process -Filter "ProcessId=$($listener.OwningProcess)" -ErrorAction SilentlyContinue
        } else {
            $null
        }
        [pscustomobject]@{
            LocalAddress = $listener.LocalAddress
            LocalPort = $listener.LocalPort
            State = $listener.State
            OwningProcess = $listener.OwningProcess
            CreationTime = $listener.CreationTime
            OwnerExists = ($null -ne $owner)
            OwnerName = $owner.Name
            OwnerCommandLine = $owner.CommandLine
        }
    }
}

function Format-SynapseTcpClientSnapshot {
    param([object[]]$Snapshot)
    if (-not $Snapshot -or $Snapshot.Count -eq 0) {
        return '<none>'
    }
    return (($Snapshot | ForEach-Object {
        "state=$($_.State) local=$($_.LocalAddress):$($_.LocalPort) remote=$($_.RemoteAddress):$($_.RemotePort) owner_pid=$($_.OwningProcess) owner=$($_.OwnerName) peer_pid=$($_.PeerOwningProcess) peer_exists=$($_.PeerOwnerExists) peer=$($_.PeerOwnerName) has_live_peer=$($_.HasLivePeer) peer_cmd=$($_.PeerOwnerCommandLine)"
    }) -join "`n")
}

function Format-SynapseTcpBindListenerSnapshot {
    param([object[]]$Snapshot)
    if (-not $Snapshot -or $Snapshot.Count -eq 0) {
        return '<none>'
    }
    return (($Snapshot | ForEach-Object {
        "state=$($_.State) local=$($_.LocalAddress):$($_.LocalPort) owner_pid=$($_.OwningProcess) owner_exists=$($_.OwnerExists) owner=$($_.OwnerName) created=$($_.CreationTime) owner_cmd=$($_.OwnerCommandLine)"
    }) -join "`n")
}

function Get-SynapseProtectedProcessNames {
    return @(
        'cmd.exe',
        'powershell.exe',
        'pwsh.exe',
        'WindowsTerminal.exe',
        'OpenConsole.exe',
        'conhost.exe',
        'wsl.exe',
        'wslhost.exe',
        'Code.exe',
        'codex.exe',
        'claude.exe',
        'node.exe'
    )
}

function Get-SynapseTcpClientPeerCloseDecision {
    param(
        [Parameter(Mandatory=$true)]$TcpClient
    )

    if (-not $TcpClient.HasLivePeer -or [int]$TcpClient.PeerOwningProcess -le 0) {
        return [pscustomobject]@{
            CanClose = $false
            Reason = 'no_live_peer'
            PeerProcess = $null
            Kind = $null
        }
    }

    $peerPid = [int]$TcpClient.PeerOwningProcess
    $peerProcess = Get-CimInstance Win32_Process -Filter "ProcessId=$peerPid" -ErrorAction SilentlyContinue
    if (-not $peerProcess) {
        return [pscustomobject]@{
            CanClose = $false
            Reason = 'peer_exited'
            PeerProcess = $null
            Kind = $null
        }
    }

    $protectedNames = Get-SynapseProtectedProcessNames
    if ($protectedNames -contains $peerProcess.Name) {
        return [pscustomobject]@{
            CanClose = $false
            Reason = "protected_process:$($peerProcess.Name)"
            PeerProcess = $peerProcess
            Kind = $null
        }
    }

    $commandLine = [string]$peerProcess.CommandLine
    $isChromeNetworkService = (
        $peerProcess.Name -ieq 'chrome.exe' -and
        $commandLine -match '(?i)--type=utility' -and
        $commandLine -match '(?i)--utility-sub-type=network\.mojom\.NetworkService'
    )
    if ($isChromeNetworkService) {
        return [pscustomobject]@{
            CanClose = $true
            Reason = 'exact_chrome_network_service_peer'
            PeerProcess = $peerProcess
            Kind = 'chrome_network_service'
        }
    }

    return [pscustomobject]@{
        CanClose = $false
        Reason = "unowned_peer:$($peerProcess.Name)"
        PeerProcess = $peerProcess
        Kind = $null
    }
}

function Stop-SynapseStaleBindClientPeersForMaintenance {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][object[]]$TcpClients
    )

    $liveClients = @($TcpClients | Where-Object { $_.HasLivePeer -and [int]$_.PeerOwningProcess -gt 0 })
    if ($liveClients.Count -eq 0) {
        return [pscustomobject]@{
            ClosedCount = 0
            ClosedPeerPids = @()
            RefusedPeerPids = @()
        }
    }

    $decisions = @($liveClients | ForEach-Object {
        $decision = Get-SynapseTcpClientPeerCloseDecision -TcpClient $_
        [pscustomobject]@{
            TcpClient = $_
            CanClose = $decision.CanClose
            Reason = $decision.Reason
            PeerProcess = $decision.PeerProcess
            Kind = $decision.Kind
        }
    })
    $closable = @($decisions | Where-Object { $_.CanClose })
    $refused = @($decisions | Where-Object { -not $_.CanClose })
    $refusedPeerPids = @($refused | ForEach-Object { [int]$_.TcpClient.PeerOwningProcess } | Where-Object { $_ -gt 0 } | Sort-Object -Unique)

    if ($refused.Count -gt 0) {
        $refusedDetail = (($refused | ForEach-Object {
            $peer = $_.PeerProcess
            $peerPid = if ($peer) { [int]$peer.ProcessId } else { [int]$_.TcpClient.PeerOwningProcess }
            $peerName = if ($peer) { $peer.Name } else { '<missing>' }
            $peerCommandLine = if ($peer) { $peer.CommandLine } else { '<missing>' }
            "peer_pid=$peerPid peer=$peerName reason=$($_.Reason) tcp=local:$($_.TcpClient.LocalAddress):$($_.TcpClient.LocalPort)->remote:$($_.TcpClient.RemoteAddress):$($_.TcpClient.RemotePort) peer_cmd=$peerCommandLine"
        }) -join "`n")
        Info ("FORCE_RESTART: SYNAPSE_FORCE_RESTART_TCP_PEER_CLOSE_REFUSED reason={0} bind={1} refused_count={2}`nrefused:`n{3}`nremediation=setup only closes exact known non-terminal client peers that are safe to restart, such as Chrome NetworkService. Protected terminal/IDE/WSL/Codex/Claude/Node peers are left running and the bind must release naturally or setup fails closed." -f `
            $Reason,
            $Bind,
            $refused.Count,
            $refusedDetail)
    }

    if ($closable.Count -eq 0) {
        return [pscustomobject]@{
            ClosedCount = 0
            ClosedPeerPids = @()
            RefusedPeerPids = $refusedPeerPids
        }
    }

    $closedPeerPids = @()
    foreach ($peerGroup in ($closable | Group-Object { [int]$_.PeerProcess.ProcessId })) {
        $first = $peerGroup.Group[0]
        $peer = $first.PeerProcess
        $peerPid = [int]$peer.ProcessId
        $peerCommandLine = [string]$peer.CommandLine
        $tcpDetail = (($peerGroup.Group | ForEach-Object {
            "local:$($_.TcpClient.LocalAddress):$($_.TcpClient.LocalPort)->remote:$($_.TcpClient.RemoteAddress):$($_.TcpClient.RemotePort)"
        }) -join ',')
        Info ("FORCE_RESTART: SYNAPSE_FORCE_RESTART_CLOSE_TCP_PEER reason={0} bind={1} peer_pid={2} peer={3} kind={4} tcp={5} peer_cmd={6}`nremediation=Windows kept a dead-owner Synapse listener row because this exact live client peer still owned a socket to the stopped daemon. Closing this exact Chrome NetworkService process lets Chrome restart networking without closing the browser profile, then setup separately re-probes the bind before starting a daemon." -f `
            $Reason,
            $Bind,
            $peerPid,
            $peer.Name,
            $first.Kind,
            $tcpDetail,
            $peerCommandLine)
        try {
            Stop-Process -Id $peerPid -Force -ErrorAction Stop
            $closedPeerPids += $peerPid
        } catch {
            Die ("SYNAPSE_FORCE_RESTART_TCP_PEER_CLOSE_FAILED reason={0} bind={1} peer_pid={2} peer={3} error={4} remediation=setup could not close the exact live client peer that is holding the dead-owner daemon socket; inspect the peer process and rerun setup after it exits" -f `
                $Reason,
                $Bind,
                $peerPid,
                $peer.Name,
                $_.Exception.Message)
        }
    }

    Start-Sleep -Seconds 2
    foreach ($closedPid in $closedPeerPids) {
        $after = Get-CimInstance Win32_Process -Filter "ProcessId=$closedPid" -ErrorAction SilentlyContinue
        if ($after) {
            Die ("SYNAPSE_FORCE_RESTART_TCP_PEER_STILL_RUNNING reason={0} bind={1} peer_pid={2} peer={3} command_line={4} remediation=exact client peer did not exit after Stop-Process; inspect it before retrying setup" -f `
                $Reason,
                $Bind,
                $closedPid,
                $after.Name,
                $after.CommandLine)
        }
    }

    Info ("FORCE_RESTART: SYNAPSE_FORCE_RESTART_TCP_PEER_CLOSE_VERIFIED reason={0} bind={1} closed_peer_pids={2}" -f `
        $Reason,
        $Bind,
        ($closedPeerPids -join ','))
    return [pscustomobject]@{
        ClosedCount = $closedPeerPids.Count
        ClosedPeerPids = @($closedPeerPids)
        RefusedPeerPids = $refusedPeerPids
    }
}

function Wait-SynapseBindReleased {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [int]$TimeoutSeconds = 15,
        [switch]$ForceRestart
    )

    if ($script:SynapseBindPostExitContinuationRequired) {
        $detail = $script:SynapseBindPostExitContinuationDetail
        $detailReason = if ($detail -and $detail.reason) { [string]$detail.reason } else { '<unknown>' }
        Info ("SYNAPSE_BIND_POST_EXIT_CONTINUATION_ALREADY_REQUIRED reason={0} bind={1} original_reason={2} remediation=the verified daemon bytes will be installed and a post-exit continuation will reacquire the maintenance lock after this setup process exits; skipping duplicate bind-drain wait inside the same process." -f `
            $Reason,
            $Bind,
            $detailReason)
        return
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $lastDeadOwnerLog = [DateTime]::MinValue
    $lastSafePeerCloseDeferredLog = [DateTime]::MinValue
    $closedForceRestartPeerPids = @{}
    $refusedForceRestartPeerPids = @{}
    $deferredForceRestartPeerPids = @{}
    $maxForceRestartPeerClosePids = 5
    do {
        Assert-SynapseChromeBridgeMaintenancePauseBudget -Reason $Reason -Bind $Bind -Phase 'initial_bind_wait'
        $listeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
        $probe = Test-SynapseBindAvailable -Bind $Bind
        if ($listeners.Count -eq 0 -and $probe.Ok) {
            Info "Synapse bind release verified reason=$Reason bind=$Bind listener_count=0 bind_probe=ok"
            return
        }
        $liveListeners = @($listeners | Where-Object { $_.OwnerExists })
        $staleListeners = @($listeners | Where-Object { -not $_.OwnerExists })
        if ($liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and $probe.Ok) {
            Info ("Synapse bind release accepted stale dead-owner listener rows reason={0} bind={1} timeout_s={2} stale_listener_count={3} bind_probe=ok`nstale_listeners:`n{4}`nremediation=Windows can report LISTEN rows briefly after the owning process exits; setup verified a new listener can bind before continuing." -f `
                $Reason,
                $Bind,
                $TimeoutSeconds,
                $staleListeners.Count,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners))
            return
        }
        if ($liveListeners.Count -eq 0 -and -not $probe.Ok -and (((Get-Date) - $lastDeadOwnerLog).TotalSeconds -ge 10)) {
            $lastDeadOwnerLog = Get-Date
            Info ("Synapse bind release waiting on Windows dead-owner TCP drain reason={0} bind={1} listener_count={2} stale_listener_count={3} bind_probe_error={4}`nstale_listeners:`n{5}`nremediation=setup will not reuse or steal the port; it waits until a normal bind probe proves the address is actually reusable." -f `
                $Reason,
                $Bind,
                $listeners.Count,
                $staleListeners.Count,
                $probe.Error,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners))
        }
        if ($ForceRestart -and $liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and -not $probe.Ok) {
            $tcpClients = @(Get-SynapseTcpClientSnapshot -Bind $Bind)
            $liveTcpClients = @($tcpClients | Where-Object { $_.HasLivePeer -and [int]$_.PeerOwningProcess -gt 0 })
            $unclosableLiveTcpClients = @($liveTcpClients | Where-Object {
                $decision = Get-SynapseTcpClientPeerCloseDecision -TcpClient $_
                -not $decision.CanClose
            })
            if ($unclosableLiveTcpClients.Count -gt 0) {
                if (((Get-Date) - $lastSafePeerCloseDeferredLog).TotalSeconds -ge 10) {
                    $lastSafePeerCloseDeferredLog = Get-Date
                    Info ("Synapse bind release safe-peer close deferred reason={0} bind={1} unclosable_live_peer_count={2} live_peer_count={3}`ntcp_clients:`n{4}`nremediation=setup will not churn restartable peers while protected/unowned clients still hold the dead-owner daemon socket; it waits for those clients to release naturally, then may close exact safe peers if needed." -f `
                        $Reason,
                        $Bind,
                        $unclosableLiveTcpClients.Count,
                        $liveTcpClients.Count,
                        (Format-SynapseTcpClientSnapshot -Snapshot $liveTcpClients))
                }
            } else {
            $newLiveTcpClients = @($liveTcpClients | Where-Object {
                $peerPidKey = [string][int]$_.PeerOwningProcess
                -not $closedForceRestartPeerPids.ContainsKey($peerPidKey) -and -not $refusedForceRestartPeerPids.ContainsKey($peerPidKey) -and -not $deferredForceRestartPeerPids.ContainsKey($peerPidKey)
            })
            if ($newLiveTcpClients.Count -gt 0) {
                $newPeerPids = @($newLiveTcpClients | ForEach-Object { [int]$_.PeerOwningProcess } | Sort-Object -Unique)
                $newClosablePeerPids = @($newLiveTcpClients | Where-Object {
                    $decision = Get-SynapseTcpClientPeerCloseDecision -TcpClient $_
                    $decision.CanClose
                } | ForEach-Object { [int]$_.PeerOwningProcess } | Sort-Object -Unique)
                if (($closedForceRestartPeerPids.Count + $newClosablePeerPids.Count) -gt $maxForceRestartPeerClosePids) {
                    foreach ($peerPid in @($newClosablePeerPids)) {
                        $deferredForceRestartPeerPids[[string]$peerPid] = $true
                    }
                    Info ("SYNAPSE_FORCE_RESTART_TCP_PEER_CLOSE_LIMIT_REACHED_WAITING reason={0} bind={1} max_peer_pids={2} already_closed_peer_pids={3} deferred_peer_pids={4}`ntcp_clients:`n{5}`nremediation=Chrome NetworkService can restart faster than Windows releases the stopped daemon socket. Setup stops closing additional safe peers at the hard cap and continues the bounded dead-owner bind drain; final success still requires a normal bind probe, and final failure reports the remaining physical TCP/process SoT." -f `
                        $Reason,
                        $Bind,
                        $maxForceRestartPeerClosePids,
                        (($closedForceRestartPeerPids.Keys | Sort-Object) -join ','),
                        ($newClosablePeerPids -join ','),
                        (Format-SynapseTcpClientSnapshot -Snapshot $liveTcpClients))
                    continue
                }
                $peerClose = Stop-SynapseStaleBindClientPeersForMaintenance -Reason $Reason -Bind $Bind -TcpClients $newLiveTcpClients
                foreach ($peerPid in @($peerClose.ClosedPeerPids)) {
                    $closedForceRestartPeerPids[[string]$peerPid] = $true
                }
                foreach ($peerPid in @($peerClose.RefusedPeerPids)) {
                    $refusedForceRestartPeerPids[[string]$peerPid] = $true
                }
                if ([int]$peerClose.ClosedCount -gt 0) {
                    continue
                }
            }
            }
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)

    $extendedDeadline = (Get-Date).AddSeconds([Math]::Max(300, $TimeoutSeconds))
    $enteredExtendedDrain = $false
    while ((Get-Date) -lt $extendedDeadline) {
        Assert-SynapseChromeBridgeMaintenancePauseBudget -Reason $Reason -Bind $Bind -Phase 'extended_dead_owner_drain'
        $listeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
        $probe = Test-SynapseBindAvailable -Bind $Bind
        $tcpClients = @(Get-SynapseTcpClientSnapshot -Bind $Bind)
        $processes = @(Get-SynapseMcpProcessSnapshot)
        $liveListeners = @($listeners | Where-Object { $_.OwnerExists })
        $staleListeners = @($listeners | Where-Object { -not $_.OwnerExists })
        if ($listeners.Count -eq 0 -and $probe.Ok) {
            Info ("Synapse bind release verified after dead-owner drain reason={0} bind={1} initial_timeout_s={2} listener_count=0 bind_probe=ok tcp_client_count={3} process_count={4}" -f `
                $Reason,
                $Bind,
                $TimeoutSeconds,
                $tcpClients.Count,
                $processes.Count)
            return
        }
        if ($liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and $probe.Ok) {
            Info ("Synapse bind release accepted stale dead-owner listener rows after drain reason={0} bind={1} initial_timeout_s={2} stale_listener_count={3} tcp_client_count={4} process_count={5} bind_probe=ok`nstale_listeners:`n{6}`ntcp_clients:`n{7}`nremediation=Windows still reports stale LISTEN rows, but setup separately proved a normal listener can bind." -f `
                $Reason,
                $Bind,
                $TimeoutSeconds,
                $staleListeners.Count,
                $tcpClients.Count,
                $processes.Count,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners),
                (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients))
            return
        }
        if ($liveListeners.Count -gt 0 -or $processes.Count -gt 0) {
            break
        }
        if ($ForceRestart -and $staleListeners.Count -gt 0 -and -not $probe.Ok -and $tcpClients.Count -gt 0) {
            $liveTcpClients = @($tcpClients | Where-Object { $_.HasLivePeer -and [int]$_.PeerOwningProcess -gt 0 })
            if ($liveTcpClients.Count -eq 0) {
                if (-not $enteredExtendedDrain -or (((Get-Date) - $lastDeadOwnerLog).TotalSeconds -ge 10)) {
                    Info ("Synapse bind release live-peer close not attempted yet reason={0} bind={1} tcp_client_count={2} live_peer_count=0 remediation=setup will keep waiting; TIME_WAIT or ownerless rows do not consume the exact live-peer close attempt." -f `
                        $Reason,
                        $Bind,
                        $tcpClients.Count)
                }
            } else {
                $unclosableLiveTcpClients = @($liveTcpClients | Where-Object {
                    $decision = Get-SynapseTcpClientPeerCloseDecision -TcpClient $_
                    -not $decision.CanClose
                })
                if ($unclosableLiveTcpClients.Count -gt 0) {
                    $codexPinnedTcpClients = @(Get-SynapseCodexPeerRows -TcpClients $unclosableLiveTcpClients)
                    if ($codexPinnedTcpClients.Count -gt 0 -and $processes.Count -eq 0 -and $staleListeners.Count -gt 0 -and -not $probe.Ok) {
                        $codexPeerPids = @($codexPinnedTcpClients |
                            ForEach-Object { [int]$_.PeerOwningProcess } |
                            Sort-Object -Unique)
                        $script:SynapseBindPostExitContinuationRequired = $true
                        $script:SynapseBindPostExitContinuationDetail = [ordered]@{
                            schema = 'synapse_bind_post_exit_continuation_required/v1'
                            reason = $Reason
                            bind = $Bind
                            timeout_s = $TimeoutSeconds
                            phase = 'protected_codex_dead_owner_bind'
                            stale_listener_count = $staleListeners.Count
                            codex_peer_pids = @($codexPeerPids)
                            bind_probe_ok = $probe.Ok
                            bind_probe_error = $probe.Error
                            observed_at_utc = (Get-Date).ToUniversalTime().ToString('o')
                            stale_listeners = @($staleListeners)
                            tcp_clients = @($tcpClients)
                            protected_codex_tcp_clients = @($codexPinnedTcpClients)
                            processes = @($processes)
                            remediation = 'the stopped daemon has no live owner, but Windows still exposes a dead-owner listener pinned by protected Codex MCP peers inside this setup process; install the verified bytes, then start a post-exit continuation after this runner exits instead of killing Codex or terminal/IDE/WSL hosts'
                        }
                        Info ("SYNAPSE_BIND_POST_EXIT_CONTINUATION_REQUIRED reason={0} bind={1} phase=protected_codex_dead_owner_bind codex_peer_pids={2} stale_listener_count={3} bind_probe_error={4}`nstale_listeners:`n{5}`ntcp_clients:`n{6}`nremediation=setup will install the verified daemon bytes and then start a hidden post-exit continuation after the current runner exits; it will not kill protected Codex, terminal, IDE, or WSL host processes." -f `
                            $Reason,
                            $Bind,
                            ($codexPeerPids -join ','),
                            $staleListeners.Count,
                            $probe.Error,
                            (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners),
                            (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients))
                        return
                    }
                    if (-not $enteredExtendedDrain -or (((Get-Date) - $lastSafePeerCloseDeferredLog).TotalSeconds -ge 10)) {
                        $lastSafePeerCloseDeferredLog = Get-Date
                        Info ("Synapse bind release safe-peer close deferred reason={0} bind={1} unclosable_live_peer_count={2} live_peer_count={3}`ntcp_clients:`n{4}`nremediation=setup will not churn restartable peers while protected/unowned clients still hold the dead-owner daemon socket; it waits for those clients to release naturally, then may close exact safe peers if needed." -f `
                            $Reason,
                            $Bind,
                            $unclosableLiveTcpClients.Count,
                            $liveTcpClients.Count,
                            (Format-SynapseTcpClientSnapshot -Snapshot $liveTcpClients))
                    }
                } else {
                $newLiveTcpClients = @($liveTcpClients | Where-Object {
                    $peerPidKey = [string][int]$_.PeerOwningProcess
                    -not $closedForceRestartPeerPids.ContainsKey($peerPidKey) -and -not $refusedForceRestartPeerPids.ContainsKey($peerPidKey) -and -not $deferredForceRestartPeerPids.ContainsKey($peerPidKey)
                })
                if ($newLiveTcpClients.Count -eq 0) {
                    if (-not $enteredExtendedDrain -or (((Get-Date) - $lastDeadOwnerLog).TotalSeconds -ge 10)) {
                        Info ("Synapse bind release live-peer close already classified reason={0} bind={1} live_peer_count={2} closed_peer_pids={3} refused_peer_pids={4} remediation=setup will keep waiting for Windows to release the socket after exact safe-peer close attempts and protected-peer refusals already completed." -f `
                            $Reason,
                            $Bind,
                            $liveTcpClients.Count,
                            (($closedForceRestartPeerPids.Keys | Sort-Object) -join ','),
                            (($refusedForceRestartPeerPids.Keys | Sort-Object) -join ',') + "$(if ($deferredForceRestartPeerPids.Count -gt 0) { '; deferred_peer_pids=' + (($deferredForceRestartPeerPids.Keys | Sort-Object) -join ',') } else { '' })")
                    }
                } else {
                    $newPeerPids = @($newLiveTcpClients | ForEach-Object { [int]$_.PeerOwningProcess } | Sort-Object -Unique)
                    $newClosablePeerPids = @($newLiveTcpClients | Where-Object {
                        $decision = Get-SynapseTcpClientPeerCloseDecision -TcpClient $_
                        $decision.CanClose
                    } | ForEach-Object { [int]$_.PeerOwningProcess } | Sort-Object -Unique)
                    if (($closedForceRestartPeerPids.Count + $newClosablePeerPids.Count) -gt $maxForceRestartPeerClosePids) {
                        foreach ($peerPid in @($newClosablePeerPids)) {
                            $deferredForceRestartPeerPids[[string]$peerPid] = $true
                        }
                        Info ("SYNAPSE_FORCE_RESTART_TCP_PEER_CLOSE_LIMIT_REACHED_WAITING reason={0} bind={1} max_peer_pids={2} already_closed_peer_pids={3} deferred_peer_pids={4}`ntcp_clients:`n{5}`nremediation=Chrome NetworkService can restart faster than Windows releases the stopped daemon socket. Setup stops closing additional safe peers at the hard cap and continues the bounded dead-owner bind drain; final success still requires a normal bind probe, and final failure reports the remaining physical TCP/process SoT." -f `
                            $Reason,
                            $Bind,
                            $maxForceRestartPeerClosePids,
                            (($closedForceRestartPeerPids.Keys | Sort-Object) -join ','),
                            ($newClosablePeerPids -join ','),
                            (Format-SynapseTcpClientSnapshot -Snapshot $liveTcpClients))
                        continue
                    }
                    $peerClose = Stop-SynapseStaleBindClientPeersForMaintenance -Reason $Reason -Bind $Bind -TcpClients $newLiveTcpClients
                    foreach ($peerPid in @($peerClose.ClosedPeerPids)) {
                        $closedForceRestartPeerPids[[string]$peerPid] = $true
                    }
                    foreach ($peerPid in @($peerClose.RefusedPeerPids)) {
                        $refusedForceRestartPeerPids[[string]$peerPid] = $true
                    }
                    if ([int]$peerClose.ClosedCount -gt 0) {
                        continue
                    }
                }
                }
            }
        }
        if (-not $enteredExtendedDrain -or (((Get-Date) - $lastDeadOwnerLog).TotalSeconds -ge 10)) {
            $enteredExtendedDrain = $true
            $lastDeadOwnerLog = Get-Date
            Info ("Synapse bind release extended dead-owner drain reason={0} bind={1} initial_timeout_s={2} listener_count={3} stale_listener_count={4} tcp_client_count={5} process_count={6} bind_probe_ok={7} bind_probe_error={8}`nstale_listeners:`n{9}`ntcp_clients:`n{10}`nremediation=setup is waiting for Windows to release dead-owner TCP rows; it will continue only after a normal bind probe succeeds." -f `
                $Reason,
                $Bind,
                $TimeoutSeconds,
                $listeners.Count,
                $staleListeners.Count,
                $tcpClients.Count,
                $processes.Count,
                $probe.Ok,
                $probe.Error,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners),
                (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients))
        }
        Start-Sleep -Seconds 1
    }

    $listeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
    $probe = Test-SynapseBindAvailable -Bind $Bind
    $tcpClients = @(Get-SynapseTcpClientSnapshot -Bind $Bind)
    $processes = @(Get-SynapseMcpProcessSnapshot)
    $liveListeners = @($listeners | Where-Object { $_.OwnerExists })
    $staleListeners = @($listeners | Where-Object { -not $_.OwnerExists })
    if ($liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and $probe.Ok) {
        Info ("Synapse bind release accepted stale dead-owner listener rows reason={0} bind={1} timeout_s={2} stale_listener_count={3} tcp_client_count={4} process_count={5} bind_probe=ok`nstale_listeners:`n{6}`ntcp_clients:`n{7}`nremediation=Windows can report LISTEN rows briefly after the owning process exits; setup verified a new listener can bind before continuing." -f `
            $Reason,
            $Bind,
            $TimeoutSeconds,
            $staleListeners.Count,
            $tcpClients.Count,
            $processes.Count,
            (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners),
            (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients))
        return
    }
    if ($ForceRestart -and $liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and $processes.Count -eq 0 -and $tcpClients.Count -eq 0 -and -not $probe.Ok) {
        Info ("SYNAPSE_BIND_FINAL_DEAD_OWNER_SETTLE reason={0} bind={1} settle_s={2} stale_listener_count={3} bind_probe_error={4}`nstale_listeners:`n{5}`nremediation=no live daemon process and no TCP peer client remain; setup performs one bounded final kernel-state settle/readback before fatal so a disappearing Windows dead-owner row is not mistaken for a live owner." -f `
            $Reason,
            $Bind,
            $SynapseBindFinalDeadOwnerSettleSeconds,
            $staleListeners.Count,
            $probe.Error,
            (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners))
        $settleDeadline = (Get-Date).AddSeconds($SynapseBindFinalDeadOwnerSettleSeconds)
        while ((Get-Date) -lt $settleDeadline) {
            Assert-SynapseChromeBridgeMaintenancePauseBudget -Reason $Reason -Bind $Bind -Phase 'final_dead_owner_settle'
            Start-Sleep -Milliseconds 250
            $settleListeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
            $settleProbe = Test-SynapseBindAvailable -Bind $Bind
            $settleTcpClients = @(Get-SynapseTcpClientSnapshot -Bind $Bind)
            $settleProcesses = @(Get-SynapseMcpProcessSnapshot)
            $settleLiveListeners = @($settleListeners | Where-Object { $_.OwnerExists })
            $settleStaleListeners = @($settleListeners | Where-Object { -not $_.OwnerExists })
            if ($settleListeners.Count -eq 0 -and $settleProbe.Ok) {
                Info ("Synapse bind release verified after final dead-owner settle reason={0} bind={1} listener_count=0 bind_probe=ok tcp_client_count={2} process_count={3}" -f `
                    $Reason,
                    $Bind,
                    $settleTcpClients.Count,
                    $settleProcesses.Count)
                return
            }
            if ($settleLiveListeners.Count -eq 0 -and $settleStaleListeners.Count -gt 0 -and $settleProbe.Ok) {
                Info ("Synapse bind release accepted stale dead-owner listener rows after final settle reason={0} bind={1} stale_listener_count={2} tcp_client_count={3} process_count={4} bind_probe=ok`nstale_listeners:`n{5}`ntcp_clients:`n{6}`nremediation=Windows still reports stale LISTEN rows, but setup separately proved a normal listener can bind." -f `
                    $Reason,
                    $Bind,
                    $settleStaleListeners.Count,
                    $settleTcpClients.Count,
                    $settleProcesses.Count,
                    (Format-SynapseTcpBindListenerSnapshot -Snapshot $settleStaleListeners),
                    (Format-SynapseTcpClientSnapshot -Snapshot $settleTcpClients))
                return
            }
            if ($settleLiveListeners.Count -gt 0 -or $settleProcesses.Count -gt 0 -or $settleTcpClients.Count -gt 0) {
                Info ("SYNAPSE_BIND_FINAL_DEAD_OWNER_SETTLE_ABORTED reason={0} bind={1} live_listener_count={2} tcp_client_count={3} process_count={4} bind_probe_ok={5}`nlive_listeners:`n{6}`ntcp_clients:`n{7}`nprocesses:`n{8}`nremediation=a live owner/client appeared during final settle; setup will fail closed with the current physical SoT instead of assuming the stale-row case." -f `
                    $Reason,
                    $Bind,
                    $settleLiveListeners.Count,
                    $settleTcpClients.Count,
                    $settleProcesses.Count,
                    $settleProbe.Ok,
                    (Format-SynapseTcpBindListenerSnapshot -Snapshot $settleLiveListeners),
                    (Format-SynapseTcpClientSnapshot -Snapshot $settleTcpClients),
                    (Format-SynapseMcpProcessSnapshot -Snapshot $settleProcesses))
                break
            }
        }
        $listeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
        $probe = Test-SynapseBindAvailable -Bind $Bind
        $tcpClients = @(Get-SynapseTcpClientSnapshot -Bind $Bind)
        $processes = @(Get-SynapseMcpProcessSnapshot)
        $liveListeners = @($listeners | Where-Object { $_.OwnerExists })
        $staleListeners = @($listeners | Where-Object { -not $_.OwnerExists })
        if ($liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and $processes.Count -eq 0 -and $tcpClients.Count -eq 0 -and -not $probe.Ok) {
            $script:SynapseBindPostExitContinuationRequired = $true
            $script:SynapseBindPostExitContinuationDetail = [ordered]@{
                schema = 'synapse_bind_post_exit_continuation_required/v1'
                reason = $Reason
                bind = $Bind
                timeout_s = $TimeoutSeconds
                stale_listener_count = $staleListeners.Count
                bind_probe_ok = $probe.Ok
                bind_probe_error = $probe.Error
                observed_at_utc = (Get-Date).ToUniversalTime().ToString('o')
                stale_listeners = @($staleListeners)
                tcp_clients = @($tcpClients)
                processes = @($processes)
                remediation = 'the stopped daemon has no live owner and no TCP peers, but Windows keeps the dead-owner listener unavailable inside this setup process; install the verified bytes, then start a post-exit continuation that waits for this runner to release process-scoped/kernel state before daemon start'
            }
            Info ("SYNAPSE_BIND_POST_EXIT_CONTINUATION_REQUIRED reason={0} bind={1} stale_listener_count={2} bind_probe_error={3}`nstale_listeners:`n{4}`nremediation=setup will install the verified daemon bytes and then start a hidden post-exit continuation instead of starting a daemon while the bind probe still fails." -f `
                $Reason,
                $Bind,
                $staleListeners.Count,
                $probe.Error,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners))
            return
        }
    }
    if ($ForceRestart -and $liveListeners.Count -eq 0 -and $staleListeners.Count -gt 0 -and $processes.Count -eq 0 -and -not $probe.Ok) {
        $liveTcpClients = @($tcpClients | Where-Object { $_.HasLivePeer -and [int]$_.PeerOwningProcess -gt 0 })
        $codexPinnedTcpClients = @(Get-SynapseCodexPeerRows -TcpClients $liveTcpClients)
        if ($codexPinnedTcpClients.Count -gt 0) {
            $codexPeerPids = @($codexPinnedTcpClients |
                ForEach-Object { [int]$_.PeerOwningProcess } |
                Sort-Object -Unique)
            $script:SynapseBindPostExitContinuationRequired = $true
            $script:SynapseBindPostExitContinuationDetail = [ordered]@{
                schema = 'synapse_bind_post_exit_continuation_required/v1'
                reason = $Reason
                bind = $Bind
                timeout_s = $TimeoutSeconds
                phase = 'protected_codex_dead_owner_bind_final'
                stale_listener_count = $staleListeners.Count
                codex_peer_pids = @($codexPeerPids)
                bind_probe_ok = $probe.Ok
                bind_probe_error = $probe.Error
                observed_at_utc = (Get-Date).ToUniversalTime().ToString('o')
                stale_listeners = @($staleListeners)
                tcp_clients = @($tcpClients)
                protected_codex_tcp_clients = @($codexPinnedTcpClients)
                processes = @($processes)
                remediation = 'the stopped daemon has no live owner, but a protected Codex MCP peer appeared only at the final dead-owner readback; install the verified bytes, then start a post-exit continuation after this runner exits instead of killing Codex or terminal/IDE/WSL hosts'
            }
            Info ("SYNAPSE_BIND_POST_EXIT_CONTINUATION_REQUIRED reason={0} bind={1} phase=protected_codex_dead_owner_bind_final codex_peer_pids={2} stale_listener_count={3} bind_probe_error={4}`nstale_listeners:`n{5}`ntcp_clients:`n{6}`nremediation=setup will install the verified daemon bytes and then start a hidden post-exit continuation after the current runner exits; it will not kill protected Codex, terminal, IDE, or WSL host processes." -f `
                $Reason,
                $Bind,
                ($codexPeerPids -join ','),
                $staleListeners.Count,
                $probe.Error,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners),
                (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients))
            return
        }
    }
    Die ("SYNAPSE_BIND_STILL_LISTENING reason={0} bind={1} timeout_s={2} listener_count={3} live_listener_count={4} stale_listener_count={5} process_count={6} bind_probe_ok={7} bind_probe_error={8}`nlive_listeners:`n{9}`nstale_listeners:`n{10}`ntcp_clients:`n{11}`nprocesses:`n{12}`nremediation=the configured HTTP bind is still occupied after daemon shutdown or Windows has not released dead-owner TCP rows after the extended drain. Do not start another daemon or switch ports. Close/restart the exact live MCP client peer listed here if it owns the remaining connection, or restart the current Codex process when it is the peer; never close terminal/IDE/WSL processes globally." -f `
        $Reason,
        $Bind,
        $TimeoutSeconds,
        $listeners.Count,
        $liveListeners.Count,
        $staleListeners.Count,
        $processes.Count,
        $probe.Ok,
        $probe.Error,
        (Format-SynapseTcpBindListenerSnapshot -Snapshot $liveListeners),
        (Format-SynapseTcpBindListenerSnapshot -Snapshot $staleListeners),
        (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients),
        (Format-SynapseMcpProcessSnapshot -Snapshot $processes))
}

function Read-SynapseSetupTokenForRestartGuard {
    param([Parameter(Mandatory=$true)][string]$TokenPath)

    if (-not (Test-Path -LiteralPath $TokenPath)) {
        return [pscustomobject]@{ Ok = $false; Code = 'SYNAPSE_RESTART_GUARD_TOKEN_MISSING'; Token = $null; Detail = "path=$TokenPath" }
    }

    try {
        $raw = Get-Content -Raw -LiteralPath $TokenPath
        $token = if ($null -eq $raw) { '' } else { $raw.Trim() }
    } catch {
        return [pscustomobject]@{ Ok = $false; Code = 'SYNAPSE_RESTART_GUARD_TOKEN_READ_FAILED'; Token = $null; Detail = "path=$TokenPath error=$($_.Exception.Message)" }
    }

    if ($token.Length -lt 16) {
        return [pscustomobject]@{ Ok = $false; Code = 'SYNAPSE_RESTART_GUARD_TOKEN_INVALID'; Token = $null; Detail = "path=$TokenPath length=$($token.Length)" }
    }

    [pscustomobject]@{ Ok = $true; Code = 'OK'; Token = $token; Detail = "path=$TokenPath length=$($token.Length)" }
}

function Read-SynapseHealthForRestartGuard {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token
    )

    try {
        $health = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $Token" } -TimeoutSec 4
        [pscustomobject]@{ Ok = $true; Health = $health; Error = $null }
    } catch {
        [pscustomobject]@{ Ok = $false; Health = $null; Error = $_.Exception.Message }
    }
}

function ConvertTo-SynapseCanonicalValue {
    param([AllowNull()][object]$Value)

    if ($null -eq $Value) {
        return $null
    }

    if ($Value -is [System.Collections.IDictionary]) {
        $ordered = [ordered]@{}
        foreach ($key in @($Value.Keys | Sort-Object { [string]$_ })) {
            $ordered[[string]$key] = ConvertTo-SynapseCanonicalValue -Value $Value[$key]
        }
        return $ordered
    }

    if ($Value -is [System.Management.Automation.PSCustomObject]) {
        $ordered = [ordered]@{}
        foreach ($prop in @($Value.PSObject.Properties | Sort-Object Name)) {
            $ordered[$prop.Name] = ConvertTo-SynapseCanonicalValue -Value $prop.Value
        }
        return $ordered
    }

    if ($Value -is [System.Collections.IEnumerable] -and $Value -isnot [string]) {
        $items = New-Object System.Collections.ArrayList
        foreach ($item in $Value) {
            [void]$items.Add((ConvertTo-SynapseCanonicalValue -Value $item))
        }
        return ,($items.ToArray())
    }

    return $Value
}

function Get-SynapseCanonicalJson {
    param([AllowNull()][object]$Value)

    $canonical = ConvertTo-SynapseCanonicalValue -Value $Value
    return ($canonical | ConvertTo-Json -Depth 100 -Compress)
}

function Get-SynapseSha256Hex {
    param([Parameter(Mandatory=$true)][string]$Text)

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
        return (($sha.ComputeHash($bytes) | ForEach-Object { $_.ToString('x2') }) -join '')
    } finally {
        $sha.Dispose()
    }
}

function Get-SynapseObjectPropertyValue {
    param(
        [AllowNull()]$Object,
        [Parameter(Mandatory=$true)][string[]]$Names
    )

    if ($null -eq $Object) {
        return $null
    }
    foreach ($name in $Names) {
        $property = $Object.PSObject.Properties[$name]
        if ($property) {
            return $property.Value
        }
    }
    return $null
}

function Read-SynapseMcpSseJsonResponse {
    param(
        [Parameter(Mandatory=$true)][string]$Content,
        [Parameter(Mandatory=$true)][string]$Operation,
        [int]$ExpectedId = 0
    )

    $trimmed = $Content.Trim()
    if ($trimmed.StartsWith('{')) {
        $message = $trimmed | ConvertFrom-Json
    } else {
        $normalized = ($Content -replace "`r`n", "`n") -replace "`r", "`n"
        $message = $null
        foreach ($frame in @($normalized -split "`n`n")) {
            $dataLines = @()
            foreach ($line in @($frame -split "`n")) {
                if ($line.StartsWith('data:')) {
                    $dataLines += $line.Substring(5).TrimStart()
                }
            }
            $data = ($dataLines -join "`n").Trim()
            if ($data.StartsWith('{')) {
                $message = $data | ConvertFrom-Json
                break
            }
        }
        if ($null -eq $message) {
            $prefix = if ($Content.Length -gt 240) { $Content.Substring(0, 240) } else { $Content }
            Die "SYNAPSE_MCP_SSE_PARSE_FAILED operation=$Operation content_prefix=$prefix remediation=streamable HTTP returned no JSON data frame; inspect daemon logs and MCP transport compatibility"
        }
    }

    if ($ExpectedId -ne 0 -and [int]$message.id -ne $ExpectedId) {
        Die "SYNAPSE_MCP_JSONRPC_ID_MISMATCH operation=$Operation expected_id=$ExpectedId actual_id=$($message.id) remediation=the daemon returned an unexpected JSON-RPC response; inspect streamable HTTP session handling"
    }
    if ($null -ne $message.error) {
        $errorJson = $message.error | ConvertTo-Json -Compress -Depth 8
        Die "SYNAPSE_MCP_JSONRPC_ERROR operation=$Operation error=$errorJson remediation=repair the daemon MCP endpoint before accepting setup"
    }

    return $message
}

function Get-SynapseWebResponseUtf8Content {
    param([Parameter(Mandatory=$true)]$Response)

    $streamProperty = $Response.PSObject.Properties['RawContentStream']
    if ($streamProperty -and $null -ne $streamProperty.Value) {
        $stream = $streamProperty.Value
        if ($stream.CanSeek) {
            $stream.Position = 0
        }
        $encoding = [System.Text.UTF8Encoding]::new($false, $true)
        $reader = [System.IO.StreamReader]::new($stream, $encoding, $true, 4096, $true)
        try {
            return $reader.ReadToEnd()
        } finally {
            $reader.Dispose()
            if ($stream.CanSeek) {
                $stream.Position = 0
            }
        }
    }

    $content = $Response.Content
    if ($content -is [byte[]]) {
        $encoding = [System.Text.UTF8Encoding]::new($false, $true)
        return $encoding.GetString($content)
    }
    return [string]$content
}

function Invoke-SynapseMcpHttpPost {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)][string]$Method,
        [Parameter(Mandatory=$true)]$Params,
        [int]$Id = 0,
        [string]$SessionId,
        [int]$TimeoutSec = 8
    )

    $headers = @{
        Authorization = "Bearer $Token"
        Accept = 'application/json, text/event-stream'
    }
    if (-not [string]::IsNullOrWhiteSpace($SessionId)) {
        $headers['Mcp-Session-Id'] = $SessionId
        $headers['MCP-Protocol-Version'] = $script:SynapseMcpProtocolVersion
    }

    $request = [ordered]@{
        jsonrpc = '2.0'
        method = $Method
        params = $Params
    }
    if ($Id -ne 0) {
        $request['id'] = $Id
    }
    $body = $request | ConvertTo-Json -Depth 30 -Compress

    try {
        $response = Invoke-WebRequest `
            -Uri "http://$Bind/mcp" `
            -Method Post `
            -Headers $headers `
            -ContentType 'application/json' `
            -Body $body `
            -TimeoutSec $TimeoutSec `
            -UseBasicParsing `
            -ErrorAction Stop
        return [pscustomobject]@{
            Content = Get-SynapseWebResponseUtf8Content -Response $response
            Headers = $response.Headers
            StatusCode = $response.StatusCode
        }
    } catch {
        Die "SYNAPSE_MCP_TOOL_SURFACE_READ_FAILED stage=$Method bind=$Bind error=$($_.Exception.Message) remediation=repair streamable HTTP MCP before accepting setup"
    }
}

function Close-SynapseMcpSetupSession {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)][string]$SessionId,
        [switch]$Required
    )

    $headers = @{
        Authorization = "Bearer $Token"
        Accept = 'application/json, text/event-stream'
        'Mcp-Session-Id' = $SessionId
        'MCP-Protocol-Version' = $script:SynapseMcpProtocolVersion
    }
    $timeoutSec = $script:SynapseMcpSessionDeleteTimeoutSec

    try {
        Invoke-WebRequest -Uri "http://$Bind/mcp" -Method Delete -Headers $headers -TimeoutSec $timeoutSec -UseBasicParsing -ErrorAction Stop | Out-Null
    } catch {
        $diagnostic = "SYNAPSE_MCP_TOOL_SURFACE_SESSION_DELETE_FAILED bind=$Bind session_id=$SessionId timeout_sec=$timeoutSec error=$($_.Exception.Message) remediation=inspect health active_sessions plus MCP_SESSION_TEARDOWN_COMPLETED/MCP_HTTP_SESSION_LIFECYCLE_CLEANUP logs and candidate process/socket SoT"
        if ($Required) {
            Die $diagnostic
        }
        Info "WARN: $diagnostic"
    }
}

function Invoke-SynapseSetupMcpTool {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)][string]$Name,
        [Parameter(Mandatory=$true)]$Arguments,
        [string]$Profile,
        [string]$ProfileReason,
        [int]$TimeoutSec = 8
    )

    $sessionId = $null
    $mcpReadSucceeded = $false
    try {
        $initParams = [ordered]@{
            protocolVersion = $script:SynapseMcpProtocolVersion
            capabilities = @{}
            clientInfo = [ordered]@{ name = 'synapse-setup'; version = '0' }
        }
        $initResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -Method 'initialize' -Params $initParams -Id 1
        $sessionId = @($initResponse.Headers['Mcp-Session-Id'])[0]
        if ([string]::IsNullOrWhiteSpace($sessionId)) {
            Die "SYNAPSE_MCP_TOOL_SESSION_MISSING bind=$Bind tool=$Name remediation=streamable HTTP initialize did not return Mcp-Session-Id"
        }
        $initMessage = Read-SynapseMcpSseJsonResponse -Content $initResponse.Content -Operation 'initialize' -ExpectedId 1
        if ($null -eq $initMessage.result -or $null -eq $initMessage.result.capabilities) {
            Die "SYNAPSE_MCP_TOOL_INITIALIZE_INVALID bind=$Bind session_id=$sessionId tool=$Name remediation=daemon initialize response is missing capabilities"
        }

        Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'notifications/initialized' -Params @{} | Out-Null

        $requestId = 2
        $toolsResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'tools/list' -Params @{} -Id $requestId -TimeoutSec $TimeoutSec
        $toolsMessage = Read-SynapseMcpSseJsonResponse -Content $toolsResponse.Content -Operation 'tools/list setup session' -ExpectedId $requestId
        $toolNames = @($toolsMessage.result.tools | ForEach-Object { [string]$_.name })
        if ($toolNames -notcontains $Name) {
            $visible = if ($toolNames.Count -eq 0) { '<none>' } else { $toolNames -join ',' }
            Die "SYNAPSE_MCP_SETUP_TOOL_NOT_VISIBLE bind=$Bind session_id=$sessionId requested_tool=$Name visible_tools=$visible remediation=setup may only call public facade tools visible through tools/list; route hidden implementation tools through their public facade/profile path"
        }
        $requestId++
        if (-not [string]::IsNullOrWhiteSpace($Profile)) {
            if ($toolNames -notcontains 'profile') {
                $visible = if ($toolNames.Count -eq 0) { '<none>' } else { $toolNames -join ',' }
                Die "SYNAPSE_MCP_SETUP_PROFILE_TOOL_NOT_VISIBLE bind=$Bind session_id=$sessionId requested_profile=$Profile visible_tools=$visible remediation=setup profile escalation requires the public profile facade in tools/list"
            }
            if ([string]::IsNullOrWhiteSpace($ProfileReason)) {
                Die "SYNAPSE_MCP_PROFILE_REASON_MISSING bind=$Bind session_id=$sessionId tool=$Name requested_profile=$Profile remediation=setup profile escalation requires an explicit reason for audit readback"
            }
            $profileArgs = [ordered]@{
                operation = 'set'
                profile = $Profile
                confirm_break_glass = $true
                reason = $ProfileReason
            }
            $profileCallParams = @{ name = 'profile'; arguments = $profileArgs }
            $profileResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'tools/call' -Params $profileCallParams -Id $requestId -TimeoutSec $TimeoutSec
            $profileMessage = Read-SynapseMcpSseJsonResponse -Content $profileResponse.Content -Operation "tools/call profile set $Profile" -ExpectedId $requestId
            if ($profileMessage.result.isError -eq $true) {
                $profileErrorText = @($profileMessage.result.content | Where-Object { [string]$_.type -eq 'text' } | Select-Object -First 1).text
                Die "SYNAPSE_MCP_PROFILE_SET_ERROR bind=$Bind session_id=$sessionId requested_profile=$Profile tool=$Name error=$profileErrorText remediation=repair the setup MCP profile policy path before accepting setup"
            }
            Info "Setup MCP session profile set session_id=$sessionId profile=$Profile reason=$ProfileReason"
            $requestId++
        }
        $callParams = @{ name = $Name; arguments = $Arguments }
        $callResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'tools/call' -Params $callParams -Id $requestId -TimeoutSec $TimeoutSec
        $callMessage = Read-SynapseMcpSseJsonResponse -Content $callResponse.Content -Operation "tools/call $Name" -ExpectedId $requestId
        if ($callMessage.result.isError -eq $true) {
            $errorText = @($callMessage.result.content | Where-Object { [string]$_.type -eq 'text' } | Select-Object -First 1).text
            Die "SYNAPSE_MCP_TOOL_CALL_ERROR bind=$Bind session_id=$sessionId tool=$Name error=$errorText remediation=repair the live daemon/bridge before accepting setup"
        }
        $text = @($callMessage.result.content | Where-Object { [string]$_.type -eq 'text' } | Select-Object -First 1).text
        $json = $null
        if (-not [string]::IsNullOrWhiteSpace($text)) {
            try {
                $json = $text | ConvertFrom-Json
            } catch {
                $json = $null
            }
        }
        $result = [pscustomobject]@{
            SessionId = $sessionId
            Message = $callMessage
            Text = $text
            Json = $json
        }
        $mcpReadSucceeded = $true
        return $result
    } finally {
        if (-not [string]::IsNullOrWhiteSpace($sessionId)) {
            Close-SynapseMcpSetupSession -Bind $Bind -Token $Token -SessionId $sessionId -Required:$mcpReadSucceeded
        }
    }
}

function Get-SynapseChromeBridgeHostWaitStep {
    # #2271: next step of the post-start bridge host wait. Pure.
    param([string]$Detail = '', [int64]$PauseRemainingMs = 0)
    if ($Detail -notmatch 'no_active_chrome_bridge_host') { return 'stop_waiting' }
    # Setup's own maintenance pause is no reason for foreground chrome://extensions
    # UI repair: the worker re-registers when the pause ends, so keep waiting (the
    # wait deadline already covers the remaining pause plus the alarm cushion).
    return 'wait'
}

function Assert-SynapseChromeBridgeLiveAfterSetup {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)]$Health,
        [Parameter(Mandatory=$true)][string]$ChromeBridgeInstallerPath,
        [Parameter(Mandatory=$true)][string]$ChromeNativeHostExePath
    )

    $chromeBridge = $Health.subsystems.chrome_bridge
    $status = [string]$chromeBridge.status
    $detail = [string]$chromeBridge.detail
    $isClean = $status -eq 'ok' -and $detail -match '(?:^|\s)extension_stale=false(?:\s|$)' -and $detail -match '(?:^|\s)extension_capabilities=(?:[^\s,]+,)*pageScreenshot(?:,|\s|$)'
    if ($isClean) {
        Info "Chrome bridge OK after daemon start: stale=false capability=pageScreenshot"
        return $Health
    }

    $currentHealth = $Health
    $nowMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $postStartWaitMs = $SynapseChromeBridgeDefaultPostStartWaitMs
    if ($null -ne $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs) {
        $remainingPauseMs = [Math]::Max(0, [int64]$script:SynapseChromeBridgeMaintenancePauseUntilUnixMs - [int64]$nowMs)
        $postStartWaitMs = [Math]::Max(
            [int64]$postStartWaitMs,
            [int64]$remainingPauseMs + [int64]$SynapseChromeBridgeReconnectAlarmCushionMs)
        $postStartWaitMs = [Math]::Min([int64]$postStartWaitMs, [int64]$SynapseChromeBridgeMaxPostStartWaitMs)
    }
    $deadlineMs = [int64]$nowMs + [int64]$postStartWaitMs
    $attempt = 0
    while ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() -lt $deadlineMs) {
        $currentMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        $pauseRemainingMs = 0
        if ($null -ne $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs) {
            $pauseRemainingMs = [Math]::Max(0, [int64]$script:SynapseChromeBridgeMaintenancePauseUntilUnixMs - [int64]$currentMs)
        }
        if ((Get-SynapseChromeBridgeHostWaitStep -Detail $detail -PauseRemainingMs $pauseRemainingMs) -ne 'wait') {
            break
        }
        $attempt += 1
        $waitRemainingMs = [Math]::Max(0, [int64]$deadlineMs - [int64]$currentMs)
        Info ("Chrome bridge host absent after daemon start; waiting for alarmReconnect readback attempt={0} pause_until_unix_ms={1} pause_remaining_ms={2} wait_remaining_ms={3}" -f `
            $attempt,
            ($(if ($null -eq $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs) { '<none>' } else { $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs })),
            $pauseRemainingMs,
            $waitRemainingMs)
        Start-Sleep -Seconds 2
        try {
            $currentHealth = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $Token" } -TimeoutSec 4
        } catch {
            Die "SYNAPSE_CHROME_BRIDGE_WAIT_HEALTH_FAILED bind=$Bind error=$($_.Exception.Message) remediation=daemon was live before Chrome bridge wait but /health failed during reconnect wait"
        }
        $chromeBridge = $currentHealth.subsystems.chrome_bridge
        $status = [string]$chromeBridge.status
        $detail = [string]$chromeBridge.detail
        $isClean = $status -eq 'ok' -and $detail -match '(?:^|\s)extension_stale=false(?:\s|$)' -and $detail -match '(?:^|\s)extension_capabilities=(?:[^\s,]+,)*pageScreenshot(?:,|\s|$)'
        if ($isClean) {
            Info "Chrome bridge OK after daemon start wait: stale=false capability=pageScreenshot"
            return $currentHealth
        }
    }
    if ($detail -match 'no_active_chrome_bridge_host') {
        Info "Chrome bridge host still absent after alarmReconnect wait; invoking bounded existing-Chrome UI repair for the installed bridge. status=$status detail=$detail"
        $uiRepairReadback = Invoke-SynapseChromeBridgeUiRepair `
            -InstallerPath $ChromeBridgeInstallerPath `
            -NativeHostExePath $ChromeNativeHostExePath
        $repairReason = [string]$uiRepairReadback.synapse_chrome_auto_install.reason
        if ($repairReason -eq $SynapseChromeBridgeUiReloadDeferredReason) {
            # No eligible Chrome window can host the live extension reload and no
            # bridge host can register without one, so neither waiting nor failing
            # here can converge (#132): complete setup with the healthy daemon and
            # surface the deferred repair through chrome_bridge health.
            Info ("WARN: Chrome bridge host absent with no eligible Chrome window; setup completes with the deferred bridge repair. reason={0} bridge_self_reload_command=browser_debugger.reload_bridge status={1}" -f `
                $repairReason,
                $status)
            return $currentHealth
        }
        $uiDeadlineMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() + [int64]45000
        $uiAttempt = 0
        do {
            $uiAttempt += 1
            Start-Sleep -Seconds 2
            try {
                $currentHealth = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $Token" } -TimeoutSec 4
            } catch {
                Die "SYNAPSE_CHROME_BRIDGE_UI_REPAIR_HEALTH_FAILED bind=$Bind error=$($_.Exception.Message) remediation=daemon was live before Chrome bridge UI repair but /health failed afterward"
            }
            $chromeBridge = $currentHealth.subsystems.chrome_bridge
            $status = [string]$chromeBridge.status
            $detail = [string]$chromeBridge.detail
            $isClean = $status -eq 'ok' -and $detail -match '(?:^|\s)extension_stale=false(?:\s|$)' -and $detail -match '(?:^|\s)extension_capabilities=(?:[^\s,]+,)*pageScreenshot(?:,|\s|$)'
            if ($isClean) {
                Info "Chrome bridge OK after existing-Chrome UI repair: reason=$repairReason stale=false capability=pageScreenshot"
                return $currentHealth
            }
            $waitRemainingMs = [Math]::Max(0, [int64]$uiDeadlineMs - [int64][DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())
            Info ("Chrome bridge still not clean after UI repair; waiting for registration attempt={0} repair_reason={1} status={2} wait_remaining_ms={3} detail={4}" -f `
                $uiAttempt,
                $repairReason,
                $status,
                $waitRemainingMs,
                $detail)
        } while ([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() -lt $uiDeadlineMs)

        $repairAutoInstall = $uiRepairReadback.synapse_chrome_auto_install | ConvertTo-Json -Depth 12 -Compress
        Die "SYNAPSE_CHROME_BRIDGE_HOST_ABSENT_AFTER_UI_REPAIR status=$status detail=$detail ui_repair=$repairAutoInstall remediation=setup invoked the already-open Chrome extension UI reload/install path and still did not observe an active bridge host in /health; inspect the Chrome extension details UI, service-worker console, and daemon chrome_bridge health before accepting setup"
    }

    Info "WARN: Chrome bridge not clean after daemon start; requesting in-place browser_debugger.reload_bridge through the new live MCP daemon. status=$status detail=$detail"
    $reloadArgs = [ordered]@{
        operation = 'reload_bridge'
        reload_bridge = [ordered]@{ wait_timeout_ms = 30000 }
    }
    $reload = Invoke-SynapseSetupMcpTool `
        -Bind $Bind `
        -Token $Token `
        -Name 'browser_debugger' `
        -Arguments $reloadArgs `
        -Profile 'browser_debugger' `
        -ProfileReason 'synapse-setup Chrome bridge post-start reload through public browser_debugger facade' `
        -TimeoutSec 45
    $reloadReadback = if ($reload.Json -and $reload.Json.reload_bridge) { $reload.Json.reload_bridge } else { $null }
    $afterBuild = if ($reloadReadback -and $reloadReadback.after) { [string]$reloadReadback.after.extension_build_id } else { 'unknown' }
    Info "Chrome bridge reload completed through public browser_debugger facade after_build_id=$afterBuild"

    try {
        $afterHealth = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $Token" } -TimeoutSec 4
    } catch {
        Die "SYNAPSE_CHROME_BRIDGE_POST_RELOAD_HEALTH_FAILED bind=$Bind error=$($_.Exception.Message) remediation=daemon was live before bridge reload but /health failed afterward"
    }
    $afterBridge = $afterHealth.subsystems.chrome_bridge
    $afterStatus = [string]$afterBridge.status
    $afterDetail = [string]$afterBridge.detail
    if ($afterStatus -ne 'ok' -or $afterDetail -notmatch '(?:^|\s)extension_stale=false(?:\s|$)' -or $afterDetail -notmatch '(?:^|\s)extension_capabilities=(?:[^\s,]+,)*pageScreenshot(?:,|\s|$)') {
        Die "SYNAPSE_CHROME_BRIDGE_STALE_AFTER_SETUP_RELOAD status=$afterStatus detail=$afterDetail remediation=setup requires the already-open Chrome profile to load the bundled bridge build; run scripts\\install-synapse-chrome-debugger.ps1 from the interactive desktop and keep normal bridge commands failed closed until health is clean"
    }
    Info "Chrome bridge OK after setup reload: stale=false capability=pageScreenshot"
    return $afterHealth
}

function Assert-SynapseBridgeLiveIdentityAfterSetup {
    # #251: the installer runs deferred their live identity checks to here,
    # after the new daemon is up and the bridge passed the short-health gate.
    # One authenticated browser_debugger MCP session reads detailed health; the
    # mandatory Chrome bridge fails closed, optional browsers are WARN-only.
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)]$Health,
        $ChromeReadback,
        [object[]]$OptionalBridges = @()
    )

    $checks = @()
    if ([string]$Health.subsystems.chrome_bridge.detail -match 'no_active_chrome_bridge_host') {
        # Only the #132 deferred UI repair completes setup without a host.
        Info "WARN: SYNAPSE_CHROME_BRIDGE_LIVE_IDENTITY_DEFERRED reason=no_active_chrome_bridge_host remediation=no bridge host can register yet; the deferred repair is surfaced in chrome_bridge health and the identity is checkable once a host registers"
    } else {
        $expected = $ChromeReadback.runtime_verification.expected_identity
        if ($null -eq $expected) {
            Die "SYNAPSE_CHROME_BRIDGE_LIVE_IDENTITY_EXPECTATION_MISSING remediation=the bridge installer readback carries no expected identity; rerun setup so the installer deploys and reports the bridge build"
        }
        $checks += [pscustomobject]@{ browser = $SynapseLegacyBrowserId; expected = $expected; mandatory = $true }
    }
    foreach ($bridge in @($OptionalBridges | Where-Object { $null -ne $_.readback.runtime_verification.expected_identity })) {
        $checks += [pscustomobject]@{ browser = [string]$bridge.browser; expected = $bridge.readback.runtime_verification.expected_identity; mandatory = $false }
    }
    if ($checks.Count -eq 0) { return }
    $mandatory = @($checks | Where-Object { $_.mandatory }).Count -gt 0
    try {
        $detailedHealth = Read-SynapseChromeBridgeDetailedHealth -Bind $Bind -Token $Token
    } catch {
        $reason = Get-SynapseBridgeIdentityFailureReason -Message $_.Exception.Message
        if ($mandatory) {
            Die "SYNAPSE_CHROME_BRIDGE_LIVE_IDENTITY_FAILED bind=$Bind reason=$reason remediation=the running daemon did not return detailed bridge identity through an authenticated browser_debugger MCP session; repair the daemon MCP/profile path before accepting setup"
        }
        Info "WARN: SYNAPSE_BROWSER_BRIDGE_LIVE_IDENTITY_UNVERIFIED browsers=$((@($checks | ForEach-Object { $_.browser })) -join ',') reason=$reason"
        return
    }
    foreach ($check in $checks) {
        $outcome = Test-SynapseBridgeLiveIdentity -Health $detailedHealth -Expected $check.expected
        if ($outcome.ok) {
            Info "Bridge live identity verified browser=$($check.browser) contract=$($check.expected.contract) source=$($outcome.source)"
            foreach ($warning in @($outcome.warnings)) {
                Info "WARN: SYNAPSE_BRIDGE_LOADED_FROM_NONSTANDARD_FOLDER browser=$($check.browser) $warning remediation=the running code matches the deployed build; Load unpacked from the expected folder to keep future setup reloads on the stable path"
            }
        } elseif ($check.mandatory) {
            Die "SYNAPSE_CHROME_BRIDGE_LIVE_IDENTITY_FAILED bind=$Bind browser=$($check.browser) reason=$($outcome.reason) remediation=the running bridge host does not report the deployed bridge identity; run scripts\install-synapse-chrome-debugger.ps1 from the interactive desktop, then browser_debugger.reload_bridge, and keep normal bridge commands failed closed until identity matches"
        } else {
            Info "WARN: SYNAPSE_BROWSER_BRIDGE_LIVE_IDENTITY_UNVERIFIED browser=$($check.browser) reason=$($outcome.reason) remediation=open $($check.browser) with the bridge loaded and rerun scripts\install-synapse-browser-bridge.ps1 -Browser $($check.browser); the mandatory $SynapseLegacyBrowserId bridge is unaffected"
        }
    }
}

function Read-SynapseDaemonToolSurface {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)]$Health
    )

    $sessionId = $null
    $mcpReadSucceeded = $false
    try {
        $initParams = [ordered]@{
            protocolVersion = $script:SynapseMcpProtocolVersion
            capabilities = @{}
            clientInfo = [ordered]@{ name = 'synapse-setup'; version = '0' }
        }
        $initResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -Method 'initialize' -Params $initParams -Id 1
        $sessionId = @($initResponse.Headers['Mcp-Session-Id'])[0]
        if ([string]::IsNullOrWhiteSpace($sessionId)) {
            Die "SYNAPSE_MCP_TOOL_SURFACE_SESSION_MISSING bind=$Bind remediation=streamable HTTP initialize did not return Mcp-Session-Id; repair daemon transport"
        }
        $initMessage = Read-SynapseMcpSseJsonResponse -Content $initResponse.Content -Operation 'initialize' -ExpectedId 1
        if ($null -eq $initMessage.result -or $null -eq $initMessage.result.capabilities) {
            Die "SYNAPSE_MCP_INITIALIZE_RESULT_INVALID bind=$Bind session_id=$sessionId remediation=daemon initialize response is missing capabilities"
        }

        Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'notifications/initialized' -Params @{} | Out-Null

        $tools = @()
        $cursor = $null
        $requestId = 2
        do {
            $listParams = @{}
            if (-not [string]::IsNullOrWhiteSpace($cursor)) {
                $listParams['cursor'] = $cursor
            }
            $listResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'tools/list' -Params $listParams -Id $requestId
            $listMessage = Read-SynapseMcpSseJsonResponse -Content $listResponse.Content -Operation 'tools/list' -ExpectedId $requestId
            if ($null -eq $listMessage.result -or $null -eq $listMessage.result.tools) {
                Die "SYNAPSE_MCP_TOOLS_LIST_RESULT_INVALID bind=$Bind session_id=$sessionId request_id=$requestId remediation=tools/list did not return a tools array"
            }
            $tools += @($listMessage.result.tools)
            $cursor = [string]$listMessage.result.nextCursor
            $requestId += 1
        } while (-not [string]::IsNullOrWhiteSpace($cursor))

        $sortedTools = @($tools | Sort-Object name)
        $toolNames = @($sortedTools | ForEach-Object { [string]$_.name })

        $healthCallParams = @{ name = 'health'; arguments = @{} }
        $healthCallResponse = Invoke-SynapseMcpHttpPost -Bind $Bind -Token $Token -SessionId $sessionId -Method 'tools/call' -Params $healthCallParams -Id $requestId
        $healthCallMessage = Read-SynapseMcpSseJsonResponse -Content $healthCallResponse.Content -Operation 'tools/call health' -ExpectedId $requestId
        $healthText = @($healthCallMessage.result.content | Where-Object { [string]$_.type -eq 'text' } | Select-Object -First 1).text
        if ([string]::IsNullOrWhiteSpace($healthText)) {
            Die "SYNAPSE_MCP_HEALTH_TOOL_RESULT_INVALID bind=$Bind session_id=$sessionId request_id=$requestId remediation=health tools/call did not return JSON text content"
        }
        try {
            $sessionHealth = $healthText | ConvertFrom-Json
        } catch {
            Die "SYNAPSE_MCP_HEALTH_TOOL_JSON_INVALID bind=$Bind session_id=$sessionId request_id=$requestId error=$($_.Exception.Message) remediation=health tools/call returned non-JSON text"
        }
        $runtimeHash = [string]$sessionHealth.tool_surface_sha256
        $runtimeToolCount = try { [int]$sessionHealth.tool_count } catch { -1 }
        $runtimeNames = @($sessionHealth.tool_names | ForEach-Object { [string]$_ } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Sort-Object)
        $toolNamesSorted = @($toolNames | Sort-Object)
        $runtimeNamesJoined = ($runtimeNames -join "`n")
        $toolNamesJoined = ($toolNamesSorted -join "`n")
        if ([string]::IsNullOrWhiteSpace($runtimeHash) -or $runtimeToolCount -ne $toolNames.Count -or $runtimeNamesJoined -ne $toolNamesJoined) {
            Die ("SYNAPSE_MCP_HEALTH_TOOL_SURFACE_MISMATCH bind={0} session_id={1} tools_list_count={2} health_count={3} health_hash={4} tools_list_only={5} health_only={6} remediation=repair health/tool-list fingerprint agreement before writing Codex snapshot" -f `
                $Bind,
                $sessionId,
                $toolNames.Count,
                $runtimeToolCount,
                $runtimeHash,
                (Format-SynapseLimitedList -Items @($toolNamesSorted | Where-Object { $runtimeNames -notcontains $_ })),
                (Format-SynapseLimitedList -Items @($runtimeNames | Where-Object { $toolNamesSorted -notcontains $_ })))
        }

        $toolSchemas = @($sortedTools | ForEach-Object {
            $tool = $_
            $inputSchema = Get-SynapseObjectPropertyValue -Object $tool -Names @('inputSchema', 'input_schema')
            $outputSchema = Get-SynapseObjectPropertyValue -Object $tool -Names @('outputSchema', 'output_schema')
            $toolCanonical = Get-SynapseCanonicalJson -Value $tool
            [ordered]@{
                name = [string]$tool.name
                description = [string]$tool.description
                input_schema = $inputSchema
                input_schema_sha256 = Get-SynapseSha256Hex -Text (Get-SynapseCanonicalJson -Value $inputSchema)
                output_schema = $outputSchema
                output_schema_sha256 = if ($null -eq $outputSchema) { $null } else { Get-SynapseSha256Hex -Text (Get-SynapseCanonicalJson -Value $outputSchema) }
                tool_sha256 = Get-SynapseSha256Hex -Text $toolCanonical
            }
        })
        $canonical = Get-SynapseCanonicalJson -Value ([ordered]@{
            mcp_surface = 'tools/list'
            tools = $sortedTools
        })
        $setupCanonicalHash = Get-SynapseSha256Hex -Text $canonical
        $daemonPid = try { [int]$Health.pid } catch { $null }

        $surface = [pscustomobject]([ordered]@{
            schema = 2
            created_at_utc = [DateTime]::UtcNow.ToString('o')
            bind = $Bind
            daemon_pid = $daemonPid
            tool_count = $toolNames.Count
            tool_surface_sha256 = $runtimeHash
            tool_surface_sha256_source = 'mcp_health_tool'
            tool_surface_setup_canonical_sha256 = $setupCanonicalHash
            tool_names = $toolNames
            tool_schemas = $toolSchemas
        })
        $mcpReadSucceeded = $true
        return $surface
    } finally {
        if (-not [string]::IsNullOrWhiteSpace($sessionId)) {
            Close-SynapseMcpSetupSession -Bind $Bind -Token $Token -SessionId $sessionId -Required:$mcpReadSucceeded
        }
    }
}

function Get-SynapseFileSha256 {
    param([Parameter(Mandatory=$true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        Die "SYNAPSE_FILE_HASH_MISSING path=$Path remediation=build or install the daemon binary before hashing it"
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
        Die "SYNAPSE_FILE_HASH_FAILED path=$Path error=$($_.Exception.Message) remediation=verify the file exists, is readable by this user, and is not protected by an exclusive writer before retrying setup"
    }
}

function Get-SynapseCheckoutCargoTarget {
    # Key the persistent target by the source checkout so two checkouts never
    # share one fingerprint database. Used by both the build and -SkipBuild.
    param(
        [Parameter(Mandatory=$true)][string]$Root,
        [Parameter(Mandatory=$true)][string]$SourceDir
    )

    $resolvedSource = (Resolve-Path $SourceDir).Path.TrimEnd('\')
    $sourceBytes = [System.Text.Encoding]::UTF8.GetBytes($resolvedSource.ToLowerInvariant())
    $sourceHash = ([System.Security.Cryptography.SHA1]::Create().ComputeHash($sourceBytes) |
        ForEach-Object { $_.ToString('x2') }) -join ''
    $sourceLeaf = (Split-Path $resolvedSource -Leaf) -replace '[^A-Za-z0-9._-]', '_'
    return (Join-Path $Root "$sourceLeaf-$($sourceHash.Substring(0,12))")
}

function Get-SynapseRetryDelayMs {
    param(
        [Parameter(Mandatory=$true)][int]$Attempt,
        [Parameter(Mandatory=$true)][int]$BaseMs,
        [Parameter(Mandatory=$true)][int]$MaxMs
    )

    $shift = [math]::Min([math]::Max($Attempt - 1, 0), 20)
    return [int][math]::Min([double]$BaseMs * [math]::Pow(2, $shift), [double]$MaxMs)
}

function Resolve-SynapseSkipBuildCandidatePath {
    param(
        [Parameter(Mandatory=$true)][string]$CargoTarget,
        [AllowNull()][Nullable[datetime]]$MinimumWriteTimeUtc = $null
    )

    $candidate = Get-SynapseCargoReleaseArtifactPath -CargoTarget $CargoTarget
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
        Die "SYNAPSE_SKIP_BUILD_BINARY_MISSING path=$candidate cargo_target=$CargoTarget remediation=-SkipBuild requires CargoTarget\release\synapse-mcp.exe; build that release artifact or pass the Cargo target directory that contains it"
    }
    $written = (Get-Item -LiteralPath $candidate).LastWriteTimeUtc
    if ($null -ne $MinimumWriteTimeUtc -and $written -lt $MinimumWriteTimeUtc) {
        Die "SYNAPSE_SKIP_BUILD_ARTIFACT_STALE FATAL path=$candidate written_utc=$($written.ToString('o')) head_commit_utc=$(([datetime]$MinimumWriteTimeUtc).ToString('o')) remediation=the artifact is older than the source HEAD commit; run setup without -SkipBuild to rebuild the per-checkout target"
    }
    return (Resolve-Path -LiteralPath $candidate).Path
}

function Get-SynapseSkipBuildHeadIdentity {
    param([Parameter(Mandatory=$true)][string]$SourceDir)

    $revision = $null
    $commitUnix = $null
    $dirty = $true
    try {
        $revision = @(& git -C $SourceDir rev-parse --short=12 HEAD 2>$null) | Select-Object -First 1
        $commitUnix = @(& git -C $SourceDir log -1 --format=%ct 2>$null) | Select-Object -First 1
        & git -C $SourceDir diff --quiet HEAD -- 2>$null
        $dirty = ($LASTEXITCODE -ne 0)
    } catch {
        $revision = $null
    }
    if ([string]$revision -notmatch '^[0-9a-f]{7,40}$' -or [string]$commitUnix -notmatch '^\d+$') {
        Die "SYNAPSE_SKIP_BUILD_HEAD_UNREADABLE FATAL source=$SourceDir remediation=-SkipBuild must prove the candidate was built from the current HEAD, but git could not read HEAD from this source directory; fix the checkout or run setup without -SkipBuild"
    }
    return [pscustomobject]@{
        Revision = [string]$revision
        CommitTimeUtc = [DateTimeOffset]::FromUnixTimeSeconds([long]$commitUnix).UtcDateTime
        Dirty = $dirty
    }
}

function Read-SynapseSourceGit {
    param([string]$SourceDir, [string[]]$GitArguments)

    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = 'git.exe'
    $start.WorkingDirectory = $SourceDir
    $start.Arguments = (($GitArguments | ForEach-Object { Quote-WindowsCommandArgument $_ }) -join ' ')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.StandardOutputEncoding = [Text.Encoding]::UTF8
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    try {
        $null = $process.Start()
        $output = $process.StandardOutput.ReadToEnd()
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "git source read failed: $($GitArguments -join ' ')" }
        return $output
    } finally { $process.Dispose() }
}

function Get-SynapseBuildSourceFingerprint {
    param([Parameter(Mandatory=$true)][string]$SourceDir)

    $head = (Read-SynapseSourceGit -SourceDir $SourceDir -GitArguments @('rev-parse', '--verify', 'HEAD')).Trim()
    if ($head -notmatch '^[0-9a-f]{40,64}$') { throw 'Source HEAD is unreadable' }
    $root = (Read-SynapseSourceGit -SourceDir $SourceDir -GitArguments @('rev-parse', '--show-toplevel')).TrimEnd("`r", "`n")
    $changed = Read-SynapseSourceGit -SourceDir $root -GitArguments @('diff', '--no-ext-diff', '--name-only', '--no-renames', '-z', 'HEAD', '--')
    $records = New-Object System.Collections.Generic.List[string]
    $records.Add($head)
    foreach ($name in @($changed.Split([char]0) | Where-Object { $_ } | Sort-Object -CaseSensitive)) {
        $path = Join-Path $root $name
        $encodedName = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($name))
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            if ((Get-Item -LiteralPath $path -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
                throw "Modified source is a reparse point: $name"
            }
            $records.Add("${encodedName}:$(Get-SynapseFileSha256 -Path $path)")
        } elseif (Test-Path -LiteralPath $path) {
            throw "Modified source is not a regular file: $name"
        } else { $records.Add("${encodedName}:deleted") }
    }
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return (($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes(($records -join "`n"))) |
            ForEach-Object { $_.ToString('x2') }) -join '')
    } finally { $sha.Dispose() }
}

function Write-SynapseBuildSourceFingerprint {
    param([Parameter(Mandatory=$true)][string]$SourceDir, [Parameter(Mandatory=$true)][string]$CandidatePath,
        [string]$ExpectedFingerprint)

    $fingerprint = Get-SynapseBuildSourceFingerprint -SourceDir $SourceDir
    if ($ExpectedFingerprint -and $fingerprint -ine $ExpectedFingerprint) {
        Die "SYNAPSE_BUILD_SOURCE_CHANGED FATAL source=$SourceDir path=$CandidatePath remediation=source changed during the build; rebuild from a stable source tree before recording proof"
    }
    [IO.File]::WriteAllText("$CandidatePath.source-fingerprint", $fingerprint, [Text.Encoding]::ASCII)
}

function Assert-SynapseSkipBuildSourceFingerprint {
    param([Parameter(Mandatory=$true)][string]$SourceDir, [Parameter(Mandatory=$true)][string]$CandidatePath)

    try {
        $proof = [IO.File]::ReadAllText("$CandidatePath.source-fingerprint")
        if ($proof -notmatch '\A[0-9a-fA-F]{64}\z') { throw 'Missing or malformed source fingerprint' }
        $current = Get-SynapseBuildSourceFingerprint -SourceDir $SourceDir
        if ($proof -ine $current) { throw 'Source fingerprint no longer matches the build' }
    } catch {
        Die "SYNAPSE_SKIP_BUILD_SOURCE_UNVERIFIED FATAL path=$CandidatePath source=$SourceDir detail=$($_.Exception.Message) remediation=run setup without -SkipBuild to rebuild and record the current source fingerprint"
    }
}

function Assert-SynapseSkipBuildRevision {
    # The daemon embeds its build revision (build.rs VERGEN_GIT_SHA, reported as
    # health.build). That identity, not file mtime, proves the artifact matches HEAD.
    param(
        [AllowNull()][AllowEmptyString()][string]$CandidateBuild,
        [Parameter(Mandatory=$true)]$HeadIdentity,
        [Parameter(Mandatory=$true)][string]$CandidatePath
    )

    $candidateRevision = ([string]$CandidateBuild) -replace '-dirty$', ''
    $head = [string]$HeadIdentity.Revision
    if ($candidateRevision -notmatch '^[0-9a-f]{7,40}$') {
        Die "SYNAPSE_SKIP_BUILD_REVISION_UNKNOWN FATAL path=$CandidatePath candidate_build=$CandidateBuild head_revision=$head remediation=the candidate daemon does not report an embedded git revision (not a release build from a git checkout); run setup without -SkipBuild"
    }
    if (-not ($candidateRevision.StartsWith($head) -or $head.StartsWith($candidateRevision))) {
        Die "SYNAPSE_SKIP_BUILD_REVISION_MISMATCH FATAL path=$CandidatePath candidate_build=$CandidateBuild head_revision=$head remediation=the artifact was built from a different revision than the current HEAD (for example another branch); run setup without -SkipBuild to rebuild the per-checkout target"
    }
    if (([string]$CandidateBuild) -match '-dirty$') {
        Info "WARN: SkipBuild candidate build=$CandidateBuild was built from a dirty tree; revision matches HEAD=$head but uncommitted changes cannot be verified"
    }
    if ($HeadIdentity.Dirty) {
        Info "WARN: SkipBuild source tree has uncommitted changes relative to HEAD=$head; candidate build=$CandidateBuild may not include them"
    }
}

function Get-SynapseCargoReleaseArtifactPath {
    param([Parameter(Mandatory=$true)][string]$CargoTarget)

    $artifactRoot = [System.IO.Path]::GetFullPath($CargoTarget)
    if (-not [string]::IsNullOrWhiteSpace($env:CARGO_BUILD_TARGET)) {
        if ($env:CARGO_BUILD_TARGET -notmatch '^[A-Za-z0-9_.-]+$') {
            Die "SYNAPSE_CARGO_BUILD_TARGET_UNSUPPORTED value=$($env:CARGO_BUILD_TARGET) remediation=use a Cargo target triple name (for example x86_64-pc-windows-msvc), not a target JSON path, or unset CARGO_BUILD_TARGET"
        }
        $artifactRoot = Join-Path $artifactRoot $env:CARGO_BUILD_TARGET
    }
    return [System.IO.Path]::GetFullPath((Join-Path $artifactRoot 'release\synapse-mcp.exe'))
}

function Get-SynapseInstallDisposition {
    param(
        [Parameter(Mandatory=$true)][string]$CandidateHash,
        [Parameter(Mandatory=$true)][string]$ExePath
    )

    $installedHash = if (Test-Path -LiteralPath $ExePath -PathType Leaf) {
        Get-SynapseFileSha256 -Path $ExePath
    } else {
        $null
    }
    return [pscustomobject]@{
        InstalledHash = $installedHash
        InstallRequired = ($null -eq $installedHash -or $installedHash -ne $CandidateHash)
    }
}

function Restore-SynapseBinaryBackupAfterInstallFailure {
    param(
        [Parameter(Mandatory=$true)][string]$BackupPath,
        [Parameter(Mandatory=$true)][string]$ExePath,
        [Parameter(Mandatory=$true)][string]$ExpectedHash,
        [Parameter(Mandatory=$true)][string]$TaskName,
        [Parameter(Mandatory=$true)][string]$InstallError
    )

    # #129: a failed publication may already have put the verified old image back at
    # $ExePath (connector bridges still map it, so it cannot be overwritten). Verify the
    # bytes first; when they already match, the overwrite is redundant. Otherwise restore
    # through rename (a mapped image can be retired aside) instead of an in-place copy.
    $currentHash = $null
    if (Test-Path -LiteralPath $ExePath -PathType Leaf) {
        try { $currentHash = Get-SynapseFileSha256 -Path $ExePath } catch { $currentHash = $null }
    }
    try {
        if ($currentHash -and $currentHash -eq $ExpectedHash) {
            Info "SYNAPSE_BINARY_ROLLBACK_ALREADY_RESTORED path=$ExePath sha256=$currentHash reason=installed bytes already match the verified backup; overwrite skipped"
        } else {
            $null = Install-SynapseBinaryOverHeldImage -Source $BackupPath -Destination $ExePath
        }
    } catch {
        Die "SYNAPSE_BINARY_INSTALL_COPY_FAILED_ROLLBACK_COPY_FAILED path=$ExePath backup=$BackupPath install_error=$InstallError rollback_error=$($_.Exception.Message) remediation=restore the verified backup manually, then rerun setup"
    }
    $restoredHash = Get-SynapseFileSha256 -Path $ExePath
    if ($restoredHash -ne $ExpectedHash) {
        Die "SYNAPSE_BINARY_INSTALL_COPY_FAILED_ROLLBACK_HASH_MISMATCH expected_sha256=$ExpectedHash actual_sha256=$restoredHash backup=$BackupPath install_error=$InstallError"
    }
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Start-ScheduledTask -TaskName $TaskName
    }
    return $restoredHash
}

function New-SynapseSetupRunDirectory {
    param(
        [Parameter(Mandatory=$true)][string]$Root,
        [Parameter(Mandatory=$true)][string]$Purpose
    )

    $safePurpose = $Purpose -replace '[^A-Za-z0-9_.-]', '_'
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssfffZ')
    $path = Join-Path $Root "$safePurpose-$stamp-$PID"
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    return $path
}

# #16: run directories created by setup (staged binary copy, candidate DB) are removed
# after use. Only an exact path strictly inside its owning root is ever deleted, never
# through a reparse point, and cleanup failure never fails setup.
function Test-SynapseTreeHasReparsePoint {
    param([Parameter(Mandatory=$true)][string]$Path)

    $pending = New-Object System.Collections.Generic.Queue[string]
    $pending.Enqueue($Path)
    while ($pending.Count -gt 0) {
        $dir = $pending.Dequeue()
        foreach ($child in @(Get-ChildItem -LiteralPath $dir -Force -ErrorAction Stop)) {
            if ($child.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { return $true }
            if ($child.PSIsContainer) { $pending.Enqueue($child.FullName) }
        }
    }
    return $false
}

function Remove-SynapseSetupOwnedDirectory {
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [Parameter(Mandatory=$true)][string]$Root
    )

    try {
        if (-not (Test-SynapsePathStrictlyInside -Path $Path -Root $Root)) {
            Info "WARN: SYNAPSE_SETUP_CLEANUP_REFUSED reason=outside_owning_root path=$Path root=$Root"
            return $false
        }
        $full = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
        $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
        if ((Split-Path -Parent $full) -ine $rootFull -or
            (Split-Path -Leaf $full) -cnotmatch '^(daemon-binary|candidate)-\d{8}T\d{9}Z-\d+$') {
            Info "WARN: SYNAPSE_SETUP_CLEANUP_REFUSED reason=not_owned_direct_child path=$full root=$rootFull"
            return $false
        }
        if (-not (Test-Path -LiteralPath $full)) { return $true }
        $reparse = [System.IO.FileAttributes]::ReparsePoint
        if (-not (Test-Path -LiteralPath $Root -PathType Container) -or
            ((Get-Item -LiteralPath $Root -Force).Attributes -band $reparse)) {
            Info "WARN: SYNAPSE_SETUP_CLEANUP_REFUSED reason=root_missing_or_reparse_point path=$full root=$Root"
            return $false
        }
        $item = Get-Item -LiteralPath $full -Force
        $ancestorPath = $rootFull
        while ($ancestorPath) {
            if ((Get-Item -LiteralPath $ancestorPath -Force -ErrorAction Stop).Attributes -band $reparse) {
                Info "WARN: SYNAPSE_SETUP_CLEANUP_REFUSED reason=reparse_ancestor path=$ancestorPath"
                return $false
            }
            $ancestorPath = Split-Path -Parent $ancestorPath
        }
        if (-not $item.PSIsContainer -or ($item.Attributes -band $reparse) -or (Test-SynapseTreeHasReparsePoint -Path $full)) {
            Info "WARN: SYNAPSE_SETUP_CLEANUP_REFUSED reason=reparse_point_or_not_a_directory path=$full"
            return $false
        }
        $lastError = $null
        for ($attempt = 1; $attempt -le 3; $attempt++) {
            try {
                Remove-Item -LiteralPath $full -Recurse -Force -ErrorAction Stop
                break
            } catch {
                $lastError = $_.Exception.Message
                Start-Sleep -Milliseconds 500
            }
        }
        if (Test-Path -LiteralPath $full) {
            Info "WARN: SYNAPSE_SETUP_CLEANUP_INCOMPLETE path=$full error=$lastError remediation=remove the leftover setup run directory manually"
            return $false
        }
        Info "Removed setup run directory path=$full"
        return $true
    } catch {
        Info "WARN: SYNAPSE_SETUP_CLEANUP_FAILED path=$Path error=$($_.Exception.Message)"
        return $false
    }
}

function Remove-SynapseLegacySetupRunDirectories {
    param([Parameter(Mandatory=$true)][string]$LogDir)

    foreach ($name in @('setup-staging', 'setup-candidates')) {
        $root = Join-Path $LogDir $name
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { continue }
        try {
            foreach ($child in @(Get-ChildItem -LiteralPath $root -Directory -Force -ErrorAction Stop)) {
                if ($child.Name -cmatch '^(daemon-binary|candidate)-\d{8}T\d{9}Z-\d+$' -and
                    (Remove-SynapseSetupOwnedDirectory -Path $child.FullName -Root $root)) {
                    $child.FullName
                }
            }
        } catch {
            Info "WARN: SYNAPSE_SETUP_CLEANUP_FAILED path=$root error=$($_.Exception.Message)"
        }
    }
}

function Register-SynapseSetupOwnedDirectory {
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [Parameter(Mandatory=$true)][string]$Root
    )

    $script:SynapseSetupOwnedDirectories = @($script:SynapseSetupOwnedDirectories) + @([pscustomobject]@{ Path = $Path; Root = $Root })
}

function Remove-SynapseSetupOwnedDirectories {
    $remaining = @()
    foreach ($entry in @($script:SynapseSetupOwnedDirectories)) {
        if ($null -eq $entry) { continue }
        if (-not (Remove-SynapseSetupOwnedDirectory -Path $entry.Path -Root $entry.Root)) { $remaining += $entry }
    }
    $script:SynapseSetupOwnedDirectories = $remaining
}

function Test-SynapseSharingViolation {
    param([System.Exception]$Exception)

    $current = $Exception
    while ($null -ne $current) {
        $win32Code = $current.HResult -band 0xFFFF
        if ($win32Code -in @(32, 33)) {
            return $true
        }
        $current = $current.InnerException
    }
    return $false
}

function Copy-SynapseFileWithSharingRetry {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [ValidateRange(1, 120000)][int]$TimeoutMs = 15000,
        [ValidateRange(1, 5000)][int]$DelayMs = 200
    )

    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
            if ($attempt -gt 1) {
                Info "SYNAPSE_BINARY_COPY_RETRY_SUCCEEDED destination=$Destination attempts=$attempt elapsed_ms=$($timer.ElapsedMilliseconds)"
            }
            return
        } catch {
            if (-not (Test-SynapseSharingViolation -Exception $_.Exception) -or $timer.ElapsedMilliseconds -ge $TimeoutMs) {
                throw
            }
            $sleepMs = Get-SynapseRetryDelayMs -Attempt $attempt -BaseMs $DelayMs -MaxMs 2000
            Info "WARN: SYNAPSE_BINARY_COPY_SHARING_RETRY destination=$Destination attempt=$attempt elapsed_ms=$($timer.ElapsedMilliseconds) timeout_ms=$TimeoutMs next_delay_ms=$sleepMs"
            Start-Sleep -Milliseconds $sleepMs
        }
    }
}

function Move-SynapseFileWithSharingRetry {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [ValidateRange(1, 120000)][int]$TimeoutMs = 15000,
        [ValidateRange(1, 5000)][int]$DelayMs = 200
    )

    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            Move-Item -LiteralPath $Source -Destination $Destination -ErrorAction Stop
            return
        } catch {
            if (-not (Test-SynapseSharingViolation -Exception $_.Exception) -or $timer.ElapsedMilliseconds -ge $TimeoutMs) { throw }
            Start-Sleep -Milliseconds (Get-SynapseRetryDelayMs -Attempt $attempt -BaseMs $DelayMs -MaxMs 2000)
        }
    }
}

function Install-SynapseBinaryOverHeldImage {
    # Connector-held images support rename. Prepare and hash the adjacent candidate
    # before retiring the live image, then publish by same-directory rename.
    param([Parameter(Mandatory = $true)][string]$Source, [Parameter(Mandatory = $true)][string]$Destination)
    if (-not (Test-Path -LiteralPath $Source)) { throw "SYNAPSE_BINARY_INSTALL_SOURCE_MISSING source=$Source" }
    $incomingPath = '{0}.incoming-{1}' -f $Destination, [guid]::NewGuid().ToString('N')
    $sourceHash = Get-SynapseFileSha256 -Path $Source
    $retiredPath = $null
    try {
        Copy-SynapseFileWithSharingRetry -Source $Source -Destination $incomingPath
        if ((Get-SynapseFileSha256 -Path $incomingPath) -ne $sourceHash) {
            throw "SYNAPSE_INSTALL_PREPARED_HASH_MISMATCH source=$Source incoming=$incomingPath expected_sha256=$sourceHash"
        }
        if (Test-Path -LiteralPath $Destination) {
            $retiredPath = '{0}.retired-{1}' -f $Destination, [guid]::NewGuid().ToString('N')
            Move-SynapseFileWithSharingRetry -Source $Destination -Destination $retiredPath
        }
        Move-SynapseFileWithSharingRetry -Source $incomingPath -Destination $Destination
    } catch {
        $installError = $_.Exception.Message
        if ($retiredPath -and (Test-Path -LiteralPath $retiredPath) -and -not (Test-Path -LiteralPath $Destination)) {
            Move-SynapseFileWithSharingRetry -Source $retiredPath -Destination $Destination
        }
        throw "SYNAPSE_BINARY_INSTALL_PUBLISH_FAILED destination=$Destination error=$installError"
    } finally {
        if (Test-Path -LiteralPath $incomingPath) { Remove-Item -LiteralPath $incomingPath -Force -ErrorAction SilentlyContinue }
    }
    if ($retiredPath) {
        try {
            Remove-Item -LiteralPath $retiredPath -Force -ErrorAction Stop
            Info "SYNAPSE_BINARY_HELD_IMAGE_RETIRED destination=$Destination retired=<deleted> holders=none"
            $retiredPath = $null
        } catch {
            Info "SYNAPSE_BINARY_HELD_IMAGE_RETIRED destination=$Destination retired=$retiredPath holders=live (connect bridges keep running from the retired image; removed by a later setup run)"
        }
    }
    $leaf = Split-Path -Leaf $Destination
    Get-ChildItem -LiteralPath (Split-Path -Parent $Destination) -Filter "$leaf.retired-*" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -ine $retiredPath } | ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
    return [pscustomobject]@{ Destination = $Destination; RetiredPath = $retiredPath }
}

function Test-SynapseInstalledHealthIdentity {
    param($Health, [string]$ExePath, [string]$DbPath)

    if (-not $Health.ok) { return $false }
    try {
        $liveProcess = Get-Process -Id ([int]$Health.pid) -ErrorAction Stop
        $run = Get-Content -LiteralPath (Join-Path $DbPath 'daemon-run-current.json') -Raw | ConvertFrom-Json
        return (
            [System.IO.Path]::GetFullPath($liveProcess.Path) -ieq [System.IO.Path]::GetFullPath($ExePath) -and
            [int]$run.pid -eq [int]$Health.pid -and
            $run.mode -eq 'http' -and
            [int64]$run.started_at_unix_ms -gt 0 -and
            $null -eq $run.ended_at_unix_ms -and
            [System.IO.Path]::GetFullPath($run.db_path) -ieq [System.IO.Path]::GetFullPath($DbPath)
        )
    } catch { return $false }
}

function New-SynapseStagedDaemonBinary {
    param(
        [Parameter(Mandatory=$true)][string]$BuiltPath,
        [Parameter(Mandatory=$true)][string]$LogDir
    )

    $stagingRoot = Join-Path $LogDir 'setup-staging'
    $stagingDir = New-SynapseSetupRunDirectory -Root $stagingRoot -Purpose 'daemon-binary'
    Register-SynapseSetupOwnedDirectory -Path $stagingDir -Root $stagingRoot
    $builtHash = Get-SynapseFileSha256 -Path $BuiltPath
    $stagedPath = Join-Path $stagingDir "synapse-mcp-$builtHash.exe"
    Copy-Item -LiteralPath $BuiltPath -Destination $stagedPath -Force
    $stagedHash = Get-SynapseFileSha256 -Path $stagedPath
    if ($stagedHash -ne $builtHash) {
        Die "SYNAPSE_STAGED_BINARY_HASH_MISMATCH built=$BuiltPath staged=$stagedPath built_hash=$builtHash staged_hash=$stagedHash remediation=inspect disk/storage; refusing to install an unverified binary"
    }
    Info "Staged daemon binary path=$stagedPath sha256=$stagedHash"
    return [pscustomobject]@{
        Path = $stagedPath
        Sha256 = $stagedHash
        SourcePath = $BuiltPath
    }
}

function Resolve-SynapseInstallSource {
    param(
        [Parameter(Mandatory=$true)][string]$ExePath,
        [Parameter(Mandatory=$true)][string]$CargoTarget,
        [Parameter(Mandatory=$true)][bool]$CargoTargetExplicit,
        [Parameter(Mandatory=$true)][string]$LogDir,
        [string]$SourceDir, [string]$BuiltPath,
        [switch]$SkipBuild, [switch]$PostExitUseInstalledBinary,
        [string]$PostExitInstalledSha256
    )

    $headIdentity = $null
    if ($SkipBuild -and $PostExitUseInstalledBinary) {
        if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
            Die "SYNAPSE_POST_EXIT_INSTALLED_BINARY_MISSING path=$ExePath remediation=continuation requires the already-installed verified daemon binary; rerun full setup to rebuild and reinstall"
        }
        $installSourcePath = $ExePath
        $installSourceHash = Get-SynapseFileSha256 -Path $installSourcePath
        if ($installSourceHash -ne $PostExitInstalledSha256) {
            Die "SYNAPSE_POST_EXIT_INSTALLED_BINARY_REPLACED path=$ExePath expected_sha256=$PostExitInstalledSha256 actual_sha256=$installSourceHash remediation=the installed daemon bytes changed after the parent verified them; rerun full setup without -SkipBuild"
        }
        Info "Post-exit SkipBuild candidate binary path=$installSourcePath sha256=$installSourceHash"
    } elseif ($SkipBuild) {
        $skipSource = if ($SourceDir) { $SourceDir } else { Split-Path -Parent $PSScriptRoot }
        if (-not $CargoTargetExplicit) {
            $CargoTarget = Get-SynapseCheckoutCargoTarget -Root $CargoTarget -SourceDir $skipSource
            Info "Per-checkout build target: $CargoTarget (source: $skipSource)"
        }
        $headIdentity = Get-SynapseSkipBuildHeadIdentity -SourceDir $skipSource
        $installSourcePath = Resolve-SynapseSkipBuildCandidatePath -CargoTarget $CargoTarget -MinimumWriteTimeUtc $headIdentity.CommitTimeUtc
        $installSourceHash = Get-SynapseFileSha256 -Path $installSourcePath
        Info "SkipBuild candidate binary path=$installSourcePath cargo_target=$CargoTarget sha256=$installSourceHash"
    } else {
        $staged = New-SynapseStagedDaemonBinary -BuiltPath $BuiltPath -LogDir $LogDir
        $installSourcePath = $staged.Path
        $installSourceHash = $staged.Sha256
    }
    return [pscustomobject]@{ Path = $installSourcePath; Sha256 = $installSourceHash; CargoTarget = $CargoTarget; HeadIdentity = $headIdentity }
}

function Invoke-SynapseInstallHealthRollback {
    param(
        [Parameter(Mandatory=$true)][string]$BackupPath,
        [Parameter(Mandatory=$true)][string]$ExePath,
        [Parameter(Mandatory=$true)][string]$ExpectedHash,
        [Parameter(Mandatory=$true)][string]$TaskName,
        [Parameter(Mandatory=$true)][string]$FailureDetail,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath,
        [Parameter(Mandatory=$true)][string]$TokenPath,
        [Parameter(Mandatory=$true)][string]$Token,
        [ValidateRange(1, 3600)][int]$HealthTimeoutSeconds = 600
    )

    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }
    Stop-SynapseMcpProcesses -Reason 'install_health_failed_rollback' -Bind $Bind -DbPath $DbPath -TokenPath $TokenPath -ForceRestart -TimeoutSeconds 300
    $hash = Restore-SynapseBinaryBackupAfterInstallFailure -BackupPath $BackupPath -ExePath $ExePath `
        -ExpectedHash $ExpectedHash -TaskName $TaskName -InstallError $FailureDetail
    $deadline = (Get-Date).AddSeconds($HealthTimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        try {
            $health = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $Token" } -TimeoutSec 4
            if (Test-SynapseInstalledHealthIdentity -Health $health -ExePath $ExePath -DbPath $DbPath) {
                return [pscustomobject]@{ Ok = $true; RollbackHash = $hash; Pid = [int]$health.pid }
            }
        } catch { }
    }
    return [pscustomobject]@{ Ok = $false; RollbackHash = $hash; Pid = $null }
}

function New-SynapseCandidateBind {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse('127.0.0.1'), 0)
    try {
        $listener.Start()
        $port = [int]$listener.LocalEndpoint.Port
    } finally {
        $listener.Stop()
    }
    return "127.0.0.1:$port"
}

function Stop-SynapseExactCandidateProcess {
    param(
        [Parameter(Mandatory=$true)][int]$ProcessId,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [string]$Reason = 'candidate_health'
    )

    $current = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
    if (-not $current) {
        Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds 5
        return
    }

    $shutdown = Request-SynapseGracefulShutdown -Bind $Bind -Token $Token -ExpectedPids @($ProcessId) -Reason $Reason
    if ($shutdown.Ok) {
        $deadline = (Get-Date).AddSeconds(10)
        do {
            Start-Sleep -Milliseconds 250
            $current = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
            if (-not $current) {
                Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds 5
                Info "Candidate daemon graceful shutdown verified pid=$ProcessId bind=$Bind"
                return
            }
        } while ((Get-Date) -lt $deadline)
    } else {
        Info "WARN: candidate graceful shutdown failed pid=$ProcessId bind=$Bind code=$($shutdown.Code) error=$($shutdown.Error); falling back to exact spawned PID stop"
    }

    $current = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
    if ($current) {
        $exeLeaf = if ($current.ExecutablePath) { Split-Path -Leaf $current.ExecutablePath } else { '' }
        if ($current.Name -inotlike 'synapse-mcp*.exe' -and $exeLeaf -inotlike 'synapse-mcp*.exe') {
            Die "SYNAPSE_CANDIDATE_STOP_TARGET_MISMATCH pid=$ProcessId actual_name=$($current.Name) actual_path=$($current.ExecutablePath) remediation=PID was reused before candidate cleanup; refusing to stop it"
        }
        Stop-Process -Id $ProcessId -Force -ErrorAction Stop
        Info "Candidate daemon exact spawned PID stop issued pid=$ProcessId bind=$Bind"
    }
    Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds 5
}

function Test-SynapseCandidateDaemon {
    param(
        [Parameter(Mandatory=$true)][string]$CandidateExePath,
        [Parameter(Mandatory=$true)][string]$ProfilesDir,
        [Parameter(Mandatory=$true)][string]$TokenPath,
        [Parameter(Mandatory=$true)][string]$LogDir
    )

    if (-not (Test-Path -LiteralPath $CandidateExePath)) {
        Die "SYNAPSE_CANDIDATE_BINARY_MISSING path=$CandidateExePath remediation=build or provide a real synapse-mcp.exe before setup can touch the live daemon"
    }
    if (-not (Test-Path -LiteralPath $ProfilesDir)) {
        Die "SYNAPSE_CANDIDATE_PROFILES_MISSING path=$ProfilesDir remediation=build/deploy profiles before validating the daemon candidate"
    }
    $tokenRead = Read-SynapseSetupTokenForRestartGuard -TokenPath $TokenPath
    if (-not $tokenRead.Ok) {
        Die "$($tokenRead.Code) stage=candidate_health $($tokenRead.Detail) remediation=setup must have a valid bearer token before candidate health can be proven"
    }

    $candidatesRoot = Join-Path $LogDir 'setup-candidates'
    $candidateRoot = New-SynapseSetupRunDirectory -Root $candidatesRoot -Purpose 'candidate'
    Register-SynapseSetupOwnedDirectory -Path $candidateRoot -Root $candidatesRoot
    $candidateDb = Join-Path $candidateRoot 'db'
    New-Item -ItemType Directory -Force -Path $candidateDb | Out-Null
    $candidateBind = New-SynapseCandidateBind
    $candidateHash = Get-SynapseFileSha256 -Path $CandidateExePath
    Info "Candidate daemon health preflight starting exe=$CandidateExePath sha256=$candidateHash bind=$candidateBind db=$candidateDb profiles=$ProfilesDir"

    $candidate = $null
    $health = $null
    $surface = $null
    try {
        $candidate = Start-Process `
            -FilePath $CandidateExePath `
            -ArgumentList @('--mode','http','--bind',$candidateBind,'--db',$candidateDb,'--profile-dir',$ProfilesDir,'--log-level','info') `
            -WindowStyle Hidden `
            -PassThru
        $deadline = (Get-Date).AddSeconds(25)
        do {
            Start-Sleep -Milliseconds 500
            $read = Read-SynapseHealthForRestartGuard -Bind $candidateBind -Token $tokenRead.Token
            if ($read.Ok -and $read.Health.ok) {
                $health = $read.Health
                break
            }
        } while ((Get-Date) -lt $deadline)

        if ($null -eq $health) {
            $alive = [bool](Get-Process -Id $candidate.Id -ErrorAction SilentlyContinue)
            $listeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $candidateBind)
            Die ("SYNAPSE_CANDIDATE_HEALTH_FAILED exe={0} sha256={1} pid={2} alive={3} bind={4} listeners={5} remediation=the newly built daemon was not healthy on an isolated DB/port; old live daemon was not touched. Inspect candidate logs and setup-build.log." -f `
                $CandidateExePath,
                $candidateHash,
                $candidate.Id,
                $alive,
                $candidateBind,
                (Format-SynapseTcpBindListenerSnapshot -Snapshot $listeners))
        }

        $healthPid = [int]$health.pid
        if ($healthPid -ne [int]$candidate.Id) {
            Die "SYNAPSE_CANDIDATE_PID_MISMATCH expected_pid=$($candidate.Id) health_pid=$healthPid bind=$candidateBind remediation=health came from an unexpected process; refusing handoff"
        }
        $surface = Read-SynapseDaemonToolSurface -Bind $candidateBind -Token $tokenRead.Token -Health $health
        if ($surface.tool_count -lt 1) {
            Die "SYNAPSE_CANDIDATE_TOOL_SURFACE_EMPTY pid=$healthPid bind=$candidateBind remediation=tools/list returned no tools; refusing handoff"
        }
        Info "Candidate daemon health preflight passed pid=$healthPid bind=$candidateBind tool_count=$($surface.tool_count) tool_surface_sha256=$($surface.tool_surface_sha256)"
        return [pscustomobject]@{
            Ok = $true
            Pid = $healthPid
            Bind = $candidateBind
            DbPath = $candidateDb
            ExePath = $CandidateExePath
            Sha256 = $candidateHash
            Build = [string]$health.build
            ToolCount = $surface.tool_count
            ToolSurfaceSha256 = $surface.tool_surface_sha256
            ToolNames = $surface.tool_names
            ToolSchemas = $surface.tool_schemas
            ToolSurface = $surface
            tool_count = $surface.tool_count
            tool_surface_sha256 = $surface.tool_surface_sha256
            tool_names = $surface.tool_names
            tool_schemas = $surface.tool_schemas
            daemon_pid = $healthPid
        }
    } finally {
        if ($candidate -and (Get-Process -Id $candidate.Id -ErrorAction SilentlyContinue)) {
            Stop-SynapseExactCandidateProcess -ProcessId ([int]$candidate.Id) -Bind $candidateBind -Token $tokenRead.Token -Reason 'candidate_health'
        } elseif ($candidateBind) {
            Wait-SynapseBindReleased -Reason 'candidate_health' -Bind $candidateBind -TimeoutSeconds 5
        }
        # #16: the candidate DB is disposable; remove the exact run directory after the process exits.
        $null = Remove-SynapseSetupOwnedDirectory -Path $candidateRoot -Root $candidatesRoot
    }
}

function Assert-SynapseRefreshToolSurface {
    # #65: the expected surface size comes from the daemon's own public tool registry
    # (PUBLIC_TOOL_LIMIT authority, reported in health), never from a literal in this script.
    param(
        [Parameter(Mandatory=$true)]$Surface,
        [Parameter(Mandatory=$true)]$Health,
        [Parameter(Mandatory=$true)][string]$Bind
    )

    $registry = $null
    if ($Health.subsystems -and $Health.subsystems.PSObject.Properties['public_tool_registry']) {
        $registry = $Health.subsystems.public_tool_registry
    }
    $detail = if ($registry) { [string]$registry.detail } else { '' }
    if ($detail -notmatch '\bpublic_tool_count=(?<public>\d+)\s+max_public_tool_count=(?<max>\d+)\b') {
        Die "SYNAPSE_TOOL_SURFACE_REFRESH_AUTHORITY_MISSING bind=$Bind remediation=health.subsystems.public_tool_registry.detail did not report public_tool_count and max_public_tool_count; repair the daemon before refreshing"
    }
    $registryCount = [int]$Matches['public']
    $registryMax = [int]$Matches['max']
    $count = [int]$Surface.tool_count
    if ($count -lt 1 -or $count -ne $registryCount -or $registryCount -gt $registryMax -or [int]$Surface.daemon_pid -ne [int]$Health.pid) {
        Die "SYNAPSE_TOOL_SURFACE_REFRESH_MISMATCH bind=$Bind tools_list_count=$count registry_public_tool_count=$registryCount registry_max_public_tool_count=$registryMax surface_pid=$($Surface.daemon_pid) health_pid=$($Health.pid) remediation=repair daemon PID or tool surface so tools/list matches the public tool registry before refreshing"
    }
}

function Write-SynapseCodexToolSurfaceSnapshot {
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [Parameter(Mandatory=$true)]$Surface
    )

    if ([string]$Surface.tool_surface_setup_canonical_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [string]$Surface.tool_surface_sha256 -cnotmatch '^[0-9a-f]{64}$') {
        Die "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_HASH_INVALID path=$Path remediation=refresh from a live MCP tools/list and health tool; placeholder or non-digest hashes are forbidden"
    }

    $temporaryPath = $null
    try {
        $dir = Split-Path -Parent $Path
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $json = $Surface | ConvertTo-Json -Depth 20
        $encoding = [System.Text.UTF8Encoding]::new($false)
        $temporaryPath = Join-Path $dir ('.{0}.incoming-{1}' -f (Split-Path -Leaf $Path), [guid]::NewGuid().ToString('N'))
        [System.IO.File]::WriteAllText($temporaryPath, $json, $encoding)
        if (Test-Path -LiteralPath $Path) {
            [System.IO.File]::Replace($temporaryPath, $Path, [System.Management.Automation.Language.NullString]::Value)
        } else {
            [System.IO.File]::Move($temporaryPath, $Path)
        }
    } catch {
        Die "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_WRITE_FAILED path=$Path error=$($_.Exception.Message) remediation=repair permissions on the Synapse appdata directory before starting Codex"
    } finally {
        if ($temporaryPath -and (Test-Path -LiteralPath $temporaryPath)) {
            Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
        }
    }
    Info "Codex tool-surface snapshot written path=$Path daemon_pid=$($Surface.daemon_pid) tool_count=$($Surface.tool_count) tool_surface_sha256=$($Surface.tool_surface_sha256)"
}

function Read-SynapseCodexToolSurfaceSnapshotOrNull {
    param([AllowNull()][string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
        return $null
    }
    try {
        return Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
    } catch {
        return [pscustomobject]@{
            unreadable = $true
            path = $Path
            error = $_.Exception.Message
        }
    }
}

function New-SynapseToolRecordMap {
    param([AllowNull()]$Surface)

    $map = @{}
    if ($null -eq $Surface -or $Surface.unreadable) {
        return $map
    }
    foreach ($record in @($Surface.tool_schemas)) {
        $name = [string]$record.name
        if (-not [string]::IsNullOrWhiteSpace($name)) {
            $map[$name] = $record
        }
    }
    return $map
}

function Get-SynapseNullableSchemaHash {
    param([AllowNull()]$Schema)

    if ($null -eq $Schema) {
        return '<null>'
    }
    return Get-SynapseSha256Hex -Text (Get-SynapseCanonicalJson -Value $Schema)
}

function Get-SynapseStoredHashOrEmpty {
    param(
        [AllowNull()]$Record,
        [Parameter(Mandatory=$true)][string]$Name
    )

    if ($null -eq $Record) {
        return ''
    }
    if ($Record -is [System.Collections.IDictionary] -and $Record.Contains($Name)) {
        if ($null -eq $Record[$Name]) {
            return ''
        }
        return [string]$Record[$Name]
    }
    $property = $Record.PSObject.Properties[$Name]
    if (-not $property -or $null -eq $property.Value) {
        return ''
    }
    return [string]$property.Value
}

function Get-SynapseRecordSchemaHash {
    param(
        [AllowNull()]$Record,
        [Parameter(Mandatory=$true)][string]$StoredHashName,
        [Parameter(Mandatory=$true)][string]$SchemaPropertyName
    )

    $stored = Get-SynapseStoredHashOrEmpty -Record $Record -Name $StoredHashName
    if (-not [string]::IsNullOrWhiteSpace($stored)) {
        return $stored
    }
    if ($null -eq $Record) {
        return '<null>'
    }
    if ($Record -is [System.Collections.IDictionary] -and $Record.Contains($SchemaPropertyName)) {
        return Get-SynapseNullableSchemaHash -Schema $Record[$SchemaPropertyName]
    }
    $property = $Record.PSObject.Properties[$SchemaPropertyName]
    $schema = if ($property) { $property.Value } else { $null }
    return Get-SynapseNullableSchemaHash -Schema $schema
}

function Format-SynapseLimitedList {
    param(
        [AllowNull()]$Items,
        [int]$Limit = 20
    )

    $values = @($Items | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) } | Sort-Object -Unique)
    if ($values.Count -eq 0) {
        return 'none'
    }
    $shown = @($values | Select-Object -First $Limit)
    $suffix = if ($values.Count -gt $Limit) { ",+$($values.Count - $Limit)_more" } else { '' }
    return (($shown -join ',') + $suffix)
}

function Get-SynapseToolSurfaceDiff {
    param(
        [AllowNull()]$StartSurface,
        [Parameter(Mandatory=$true)]$CurrentSurface
    )

    if ($null -eq $StartSurface) {
        return [pscustomobject]([ordered]@{
            Summary = 'start_snapshot=missing added=unknown removed=unknown callable_schema_changed=unknown'
            SchemaDetail = 'missing'
            HasRestartRequired = $true
            HasNameDelta = $true
            HasCallableSchemaChange = $true
        })
    }
    if ($StartSurface.unreadable) {
        return [pscustomobject]([ordered]@{
            Summary = "start_snapshot=unreadable error=$($StartSurface.error) added=unknown removed=unknown callable_schema_changed=unknown"
            SchemaDetail = 'unreadable'
            HasRestartRequired = $true
            HasNameDelta = $true
            HasCallableSchemaChange = $true
        })
    }

    $startNames = @($StartSurface.tool_names | ForEach-Object { [string]$_ } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Sort-Object -Unique)
    $currentNames = @($CurrentSurface.tool_names | ForEach-Object { [string]$_ } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Sort-Object -Unique)
    $added = @($currentNames | Where-Object { $startNames -notcontains $_ })
    $removed = @($startNames | Where-Object { $currentNames -notcontains $_ })

    $startMap = New-SynapseToolRecordMap -Surface $StartSurface
    $currentMap = New-SynapseToolRecordMap -Surface $CurrentSurface
    $inputChanged = @()
    $outputChanged = @()
    $descriptionChanged = @()
    $storedHashChanged = @()
    $storedSchemaHashOnlyChanged = @()
    if ($startMap.Count -gt 0 -and $currentMap.Count -gt 0) {
        foreach ($name in $currentNames) {
            if ($startMap.ContainsKey($name) -and $currentMap.ContainsKey($name)) {
                $startRecord = $startMap[$name]
                $currentRecord = $currentMap[$name]
                $startInputHash = Get-SynapseRecordSchemaHash -Record $startRecord -StoredHashName 'input_schema_sha256' -SchemaPropertyName 'input_schema'
                $currentInputHash = Get-SynapseRecordSchemaHash -Record $currentRecord -StoredHashName 'input_schema_sha256' -SchemaPropertyName 'input_schema'
                $startOutputHash = Get-SynapseRecordSchemaHash -Record $startRecord -StoredHashName 'output_schema_sha256' -SchemaPropertyName 'output_schema'
                $currentOutputHash = Get-SynapseRecordSchemaHash -Record $currentRecord -StoredHashName 'output_schema_sha256' -SchemaPropertyName 'output_schema'
                if ($startInputHash -ne $currentInputHash) {
                    $inputChanged += $name
                }
                if ($startOutputHash -ne $currentOutputHash) {
                    $outputChanged += $name
                }
                if ([string]$startRecord.description -ne [string]$currentRecord.description) {
                    $descriptionChanged += $name
                }
                $storedInputChanged = (Get-SynapseStoredHashOrEmpty -Record $startRecord -Name 'input_schema_sha256') -ne (Get-SynapseStoredHashOrEmpty -Record $currentRecord -Name 'input_schema_sha256')
                $storedOutputChanged = (Get-SynapseStoredHashOrEmpty -Record $startRecord -Name 'output_schema_sha256') -ne (Get-SynapseStoredHashOrEmpty -Record $currentRecord -Name 'output_schema_sha256')
                $storedToolChanged = (Get-SynapseStoredHashOrEmpty -Record $startRecord -Name 'tool_sha256') -ne (Get-SynapseStoredHashOrEmpty -Record $currentRecord -Name 'tool_sha256')
                if ($storedToolChanged) {
                    $storedHashChanged += $name
                }
                if (($storedInputChanged -or $storedOutputChanged) -and $startInputHash -eq $currentInputHash -and $startOutputHash -eq $currentOutputHash) {
                    $storedSchemaHashOnlyChanged += $name
                }
            }
        }
    }
    $schemaDetail = if ($startMap.Count -gt 0 -and $currentMap.Count -gt 0) { 'present' } else { 'missing' }
    $callableSchemaChanged = @($inputChanged + $outputChanged | Sort-Object -Unique)
    $hasNameDelta = ($added.Count -gt 0 -or $removed.Count -gt 0)
    $hasCallableSchemaChange = ($schemaDetail -ne 'present' -or $callableSchemaChanged.Count -gt 0)
    $summary = ("start_snapshot_schema_detail={0} added={1} removed={2} callable_schema_changed={3} input_schema_changed={4} output_schema_changed={5} description_changed={6} stored_tool_hash_changed={7} stored_schema_hash_only_changed={8}" -f `
        $schemaDetail,
        (Format-SynapseLimitedList -Items $added),
        (Format-SynapseLimitedList -Items $removed),
        (Format-SynapseLimitedList -Items $callableSchemaChanged),
        (Format-SynapseLimitedList -Items $inputChanged),
        (Format-SynapseLimitedList -Items $outputChanged),
        (Format-SynapseLimitedList -Items $descriptionChanged),
        (Format-SynapseLimitedList -Items $storedHashChanged),
        (Format-SynapseLimitedList -Items $storedSchemaHashOnlyChanged))
    return [pscustomobject]([ordered]@{
        Summary = $summary
        SchemaDetail = $schemaDetail
        Added = $added
        Removed = $removed
        InputSchemaChanged = $inputChanged
        OutputSchemaChanged = $outputChanged
        CallableSchemaChanged = $callableSchemaChanged
        DescriptionChanged = $descriptionChanged
        StoredToolHashChanged = $storedHashChanged
        StoredSchemaHashOnlyChanged = $storedSchemaHashOnlyChanged
        HasDescriptionChange = ($descriptionChanged.Count -gt 0)
        HasNameDelta = $hasNameDelta
        HasCallableSchemaChange = $hasCallableSchemaChange
        HasRestartRequired = ($hasNameDelta -or $hasCallableSchemaChange -or $descriptionChanged.Count -gt 0)
    })
}

function Get-SynapseToolSurfaceDiffSummary {
    param(
        [AllowNull()]$StartSurface,
        [Parameter(Mandatory=$true)]$CurrentSurface
    )

    $diff = Get-SynapseToolSurfaceDiff -StartSurface $StartSurface -CurrentSurface $CurrentSurface
    return $diff.Summary
}

function Write-SynapseUtf8NoBomFile {
    param(
        [Parameter(Mandatory=$true)][string]$Path,
        [Parameter(Mandatory=$true)][string]$Text
    )

    $dir = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $encoding = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Text, $encoding)
}

function Get-SynapseHandoffGitReadback {
    param([AllowNull()][string]$SourceDir)

    if ([string]::IsNullOrWhiteSpace($SourceDir) -or -not (Test-Path -LiteralPath $SourceDir)) {
        return [ordered]@{
            available = $false
            reason = 'source_dir_missing_or_not_supplied'
            source_dir = $SourceDir
        }
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        return [ordered]@{
            available = $false
            reason = 'git_not_found'
            source_dir = $SourceDir
        }
    }

    try {
        $status = @(& git -C $SourceDir status --short --branch 2>&1)
        $head = @(& git -C $SourceDir rev-parse HEAD 2>&1)
        $origin = @(& git -C $SourceDir rev-parse origin/main 2>&1)
        $branch = @(& git -C $SourceDir branch --show-current 2>&1)
        return [ordered]@{
            available = $true
            source_dir = $SourceDir
            branch = (($branch | Select-Object -First 1) -join '').Trim()
            head = (($head | Select-Object -First 1) -join '').Trim()
            origin_main = (($origin | Select-Object -First 1) -join '').Trim()
            status_short_branch = @($status)
        }
    } catch {
        return [ordered]@{
            available = $false
            reason = 'git_readback_failed'
            source_dir = $SourceDir
            error = $_.Exception.Message
        }
    }
}

function Get-SynapseRecoveryNotesPath {
    param([AllowNull()][string]$SourceDir)

    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($SourceDir)) {
        $candidates += (Join-Path $SourceDir 'STATE\RECOVERY_NOTES.md')
    }
    $repoRootFromScript = Split-Path -Parent $PSScriptRoot
    if (-not [string]::IsNullOrWhiteSpace($repoRootFromScript)) {
        $candidates += (Join-Path $repoRootFromScript 'STATE\RECOVERY_NOTES.md')
    }

    foreach ($candidate in @($candidates | Select-Object -Unique)) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }
    $first = @($candidates | Select-Object -First 1)
    if ($first.Count -gt 0) {
        return [string]$first[0]
    }
    return $null
}

function Get-SynapseNormalizedIssueRef {
    param([AllowNull()][string]$Issue)

    if ([string]::IsNullOrWhiteSpace($Issue)) {
        return $null
    }

    $trimmed = $Issue.Trim()
    if ($trimmed -match '^#?(?<number>[0-9]+)$') {
        return "#$($Matches['number'])"
    }
    if ($trimmed -match '^https://github\.com/ChrisRoyse/Synapse/issues/(?<number>[0-9]+)(?:[/?#].*)?$') {
        return "#$($Matches['number'])"
    }

    Die "SYNAPSE_ACTIVE_ISSUE_INVALID value=$trimmed remediation=pass an issue number like 1441, #1441, or https://github.com/ChrisRoyse/Synapse/issues/1441"
}

function Get-SynapseIssueNumberFromRef {
    param([AllowNull()][string]$IssueRef)

    if ([string]::IsNullOrWhiteSpace($IssueRef)) {
        return $null
    }
    if ($IssueRef -match '^#(?<number>[0-9]+)$') {
        return $Matches['number']
    }
    return $null
}

function ConvertTo-SynapseHandoffDiffObject {
    param([AllowNull()]$Diff)

    if ($null -eq $Diff) {
        return [ordered]@{
            available = $false
            reason = 'diff_not_supplied'
        }
    }

    return [ordered]@{
        available = $true
        summary = [string]$Diff.Summary
        schema_detail = [string]$Diff.SchemaDetail
        has_restart_required = [bool]$Diff.HasRestartRequired
        has_name_delta = [bool]$Diff.HasNameDelta
        has_callable_schema_change = [bool]$Diff.HasCallableSchemaChange
        has_description_change = [bool]$Diff.HasDescriptionChange
        added = @($Diff.Added | ForEach-Object { [string]$_ })
        removed = @($Diff.Removed | ForEach-Object { [string]$_ })
        input_schema_changed = @($Diff.InputSchemaChanged | ForEach-Object { [string]$_ })
        output_schema_changed = @($Diff.OutputSchemaChanged | ForEach-Object { [string]$_ })
        callable_schema_changed = @($Diff.CallableSchemaChanged | ForEach-Object { [string]$_ })
        description_changed = @($Diff.DescriptionChanged | ForEach-Object { [string]$_ })
        stored_tool_hash_changed = @($Diff.StoredToolHashChanged | ForEach-Object { [string]$_ })
        stored_schema_hash_only_changed = @($Diff.StoredSchemaHashOnlyChanged | ForEach-Object { [string]$_ })
    }
}

function ConvertTo-SynapseTcpClientEvidenceObject {
    param([object[]]$TcpClients)

    return @($TcpClients | ForEach-Object {
        [ordered]@{
            state = [string]$_.State
            local_address = [string]$_.LocalAddress
            local_port = [int]$_.LocalPort
            remote_address = [string]$_.RemoteAddress
            remote_port = [int]$_.RemotePort
            owning_process = [int]$_.OwningProcess
            owner_name = [string]$_.OwnerName
            owner_command_line = [string]$_.OwnerCommandLine
            peer_owning_process = [int]$_.PeerOwningProcess
            peer_owner_exists = [bool]$_.PeerOwnerExists
            peer_owner_name = [string]$_.PeerOwnerName
            peer_owner_command_line = [string]$_.PeerOwnerCommandLine
            has_live_peer = [bool]$_.HasLivePeer
        }
    })
}

function ConvertTo-SynapseListenerEvidenceObject {
    param([object[]]$Listeners)

    return @($Listeners | ForEach-Object {
        [ordered]@{
            state = [string]$_.State
            local_address = [string]$_.LocalAddress
            local_port = [int]$_.LocalPort
            owning_process = [int]$_.OwningProcess
            owner_exists = [bool]$_.OwnerExists
            owner_name = [string]$_.OwnerName
            owner_command_line = [string]$_.OwnerCommandLine
            creation_time = [string]$_.CreationTime
        }
    })
}

function Get-SynapseCodexPeerRows {
    param([object[]]$TcpClients)

    return @($TcpClients | Where-Object {
        if (-not $_.HasLivePeer -or [int]$_.PeerOwningProcess -le 0) {
            $false
        } else {
            $peer = Get-CimInstance Win32_Process -Filter "ProcessId=$([int]$_.PeerOwningProcess)" -ErrorAction SilentlyContinue
            Test-SynapseCodexProcess -Process $peer
        }
    })
}

function Write-SynapseCodexSocketRestartHandoff {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][object[]]$CodexTcpClients,
        [Parameter(Mandatory=$true)][object[]]$TcpClients,
        [Parameter(Mandatory=$true)][object[]]$StaleListeners,
        [Parameter(Mandatory=$true)][AllowNull()][string]$BindProbeError,
        [AllowNull()][string]$SourceDir,
        [AllowNull()][string]$TokenPath,
        [AllowNull()][string]$ActiveIssue
    )

    $root = Join-Path $env:LOCALAPPDATA 'synapse\codex-restart-handoffs'
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
    $firstCodexPid = @($CodexTcpClients | ForEach-Object { [int]$_.PeerOwningProcess } | Sort-Object -Unique | Select-Object -First 1)
    $codexPidForName = if ($firstCodexPid.Count -gt 0) { [int]$firstCodexPid[0] } else { 0 }
    $baseName = "codex-socket-handoff-$codexPidForName-$stamp"
    $jsonPath = Join-Path $root "$baseName.json"
    $mdPath = Join-Path $root "$baseName.md"
    $recoveryNotesPath = Get-SynapseRecoveryNotesPath -SourceDir $SourceDir
    $activeIssueRef = Get-SynapseNormalizedIssueRef -Issue $ActiveIssue
    $activeIssueNumber = Get-SynapseIssueNumberFromRef -IssueRef $activeIssueRef
    $activeIssueRead = if ([string]::IsNullOrWhiteSpace($activeIssueNumber)) {
        $null
    } else {
        "gh issue view $activeIssueNumber --repo ChrisRoyse/Synapse --comments"
    }
    $codexPeers = @($CodexTcpClients | ForEach-Object {
        $peerPid = [int]$_.PeerOwningProcess
        $peer = Get-CimInstance Win32_Process -Filter "ProcessId=$peerPid" -ErrorAction SilentlyContinue
        [ordered]@{
            pid = $peerPid
            name = if ($peer) { [string]$peer.Name } else { [string]$_.PeerOwnerName }
            command_line = if ($peer) { [string]$peer.CommandLine } else { [string]$_.PeerOwnerCommandLine }
            tcp_local = "$($_.LocalAddress):$($_.LocalPort)"
            tcp_remote = "$($_.RemoteAddress):$($_.RemotePort)"
        }
    })
    $codexPeerPids = @($codexPeers | ForEach-Object { [int]$_.pid } | Sort-Object -Unique)
    $postRestartRequiredReads = @(
        'C:\Users\hotra\Downloads\AICodingAgentSuperPrompt.md',
        'C:\code\Synapse\docs\compressionprompt.md',
        'C:\code\Synapse\AGENTS.md',
        $recoveryNotesPath
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) }
    $githubReads = @(
        'gh issue view 351 --repo ChrisRoyse/Synapse --comments',
        $activeIssueRead,
        'gh issue view 1405 --repo ChrisRoyse/Synapse --comments',
        'gh issue list --repo ChrisRoyse/Synapse --state open --limit 100'
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) }
    $restartCommandHint = if ([string]::IsNullOrWhiteSpace($activeIssueRef)) {
        "Close the exact Codex peer process(es) named in this handoff by ending their owning Codex session(s), start a new Codex session through the patched launcher, verify those stale peer PID(s) are gone, then resume from GitHub issue state."
    } else {
        "Close the exact Codex peer process(es) named in this handoff by ending their owning Codex session(s), start a new Codex session through the patched launcher, verify those stale peer PID(s) are gone, then resume $activeIssueRef."
    }

    $record = [ordered]@{
        schema_version = 1
        artifact_kind = 'synapse_codex_socket_restart_handoff'
        created_at_utc = [DateTime]::UtcNow.ToString('o')
        reason_code = 'SYNAPSE_CODEX_CURRENT_PROCESS_SOCKET_STALE'
        reason = $Reason
        phase = 'dead_owner_bind_drain'
        required_restart = $true
        no_in_process_socket_release = $true
        explanation = 'The old synapse-mcp daemon exited, but Windows still reports dead-owner listener/socket rows because a live Codex MCP client peer is attached to the stopped daemon socket. Setup must not kill Codex, terminal, IDE, or WSL processes globally; restart the exact Codex session named here so Windows releases the stale socket, then rerun setup repair.'
        bind = $Bind
        bind_probe = [ordered]@{
            ok = $false
            error = $BindProbeError
        }
        codex_peer_pids = $codexPeerPids
        codex_peers = $codexPeers
        stale_listeners = ConvertTo-SynapseListenerEvidenceObject -Listeners $StaleListeners
        tcp_clients = ConvertTo-SynapseTcpClientEvidenceObject -TcpClients $TcpClients
        active_issue = [ordered]@{
            issue_ref = $activeIssueRef
            issue_number = $activeIssueNumber
            source = 'ActiveIssue parameter or SYNAPSE_ACTIVE_ISSUE environment variable'
            status = if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'unknown' } else { 'provided' }
        }
        github_reads = $githubReads
        post_restart_required_reads = $postRestartRequiredReads
        post_restart_verification = @(
            'Run git status --short --branch and confirm the working tree matches this handoff/recovery note.',
            "Read the OS process table and confirm stale Codex peer PID(s) $($codexPeerPids -join ',') no longer exist.",
            'Read Get-NetTCPConnection for 127.0.0.1:7700 and confirm no rows point at the stopped daemon PID from this handoff.',
            'Run deferred Synapse tool discovery, then call real mcp__synapse.health and setup status from the fresh Codex session.',
            'Rerun setup.repair through the real MCP setup facade; direct HTTP/stdio helper calls are diagnostics only.'
        )
        restart_command_hint = $restartCommandHint
        repo_readback = Get-SynapseHandoffGitReadback -SourceDir $SourceDir
        token_path = $TokenPath
        recovery_notes_path = $recoveryNotesPath
    }

    try {
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        Write-SynapseUtf8NoBomFile -Path $jsonPath -Text (($record | ConvertTo-Json -Depth 40) + "`n")
        $md = @(
            '# Synapse Codex Socket Restart Handoff',
            '',
            "- Reason: SYNAPSE_CODEX_CURRENT_PROCESS_SOCKET_STALE ($Reason)",
            '- Phase: dead_owner_bind_drain',
            "- Created UTC: $($record.created_at_utc)",
            "- Bind: $Bind",
            "- Codex peer PID(s): $($codexPeerPids -join ',')",
            "- Active issue: $(if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'unknown; recover from caller/session context or open issue queue' } else { $activeIssueRef })",
            '',
            '## Required Restart',
            'The running Codex peer still has a TCP connection to the stopped daemon. Setup cannot safely kill Codex or terminal/IDE/WSL hosts. End the exact Codex session owning the listed PID(s), start a new Codex session through the patched launcher, and prove those PID(s) disappeared before retrying setup repair.',
            '',
            '## Socket Evidence',
            '```text',
            "stale_listeners:",
            (Format-SynapseTcpBindListenerSnapshot -Snapshot $StaleListeners),
            '',
            "tcp_clients:",
            (Format-SynapseTcpClientSnapshot -Snapshot $TcpClients),
            '',
            "bind_probe_error=$BindProbeError",
            '```',
            '',
            '## Read After Restart'
        )
        foreach ($item in $postRestartRequiredReads) {
            $md += "- $item"
        }
        $md += @(
            '',
            '## GitHub Reads'
        )
        foreach ($item in $record.github_reads) {
            $md += "- $item"
        }
        $md += @(
            '',
            '## Verification'
        )
        foreach ($item in $record.post_restart_verification) {
            $md += "- $item"
        }
        $md += @(
            '',
            "JSON artifact: $jsonPath",
            ''
        )
        Write-SynapseUtf8NoBomFile -Path $mdPath -Text (($md -join "`n") + "`n")

        if (-not [string]::IsNullOrWhiteSpace($recoveryNotesPath)) {
            $notes = @(
                '# Synapse Recovery Notes',
                '',
                '## Latest Codex Socket Restart Handoff',
                '',
                "- Reason: SYNAPSE_CODEX_CURRENT_PROCESS_SOCKET_STALE ($Reason)",
                '- Phase: dead_owner_bind_drain',
                "- Created UTC: $($record.created_at_utc)",
                "- JSON: $jsonPath",
                "- Markdown: $mdPath",
                "- Stale Codex peer PID(s): $($codexPeerPids -join ',')",
                "- Daemon bind: $Bind",
                "- Active issue: $(if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'unknown; recover from caller/session context or open issue queue' } else { $activeIssueRef })",
                '',
                "After restart, re-read AGENTS.md, #351, #1405, $(if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'the active issue from the caller/session context' } else { $activeIssueRef }), git status, and this file before resuming. Prove the stale Codex peer PID(s) are gone and rerun real mcp__synapse setup repair; direct helper calls are diagnostics only.",
                ''
            )
            Write-SynapseUtf8NoBomFile -Path $recoveryNotesPath -Text (($notes -join "`n") + "`n")
        }
    } catch {
        Die "SYNAPSE_CODEX_SOCKET_RESTART_HANDOFF_WRITE_FAILED reason=$Reason bind=$Bind path=$jsonPath error=$($_.Exception.Message) remediation=repair permissions on %LOCALAPPDATA%\synapse\codex-restart-handoffs and the repo STATE directory, then rerun setup"
    }

    Info "SYNAPSE_CODEX_CURRENT_PROCESS_SOCKET_STALE handoff_written reason=$Reason json=$jsonPath markdown=$mdPath recovery_notes=$recoveryNotesPath codex_peer_pids=$($codexPeerPids -join ',')"
    return [pscustomobject]@{
        JsonPath = $jsonPath
        MarkdownPath = $mdPath
        RecoveryNotesPath = $recoveryNotesPath
        CodexPeerPids = $codexPeerPids
    }
}

function Start-SynapsePostExitSetupContinuation {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$SourceDir,
        [Parameter(Mandatory=$true)][string]$ExePath,
        [Parameter(Mandatory=$true)][string]$ChromeNativeHostExePath,
        [Parameter(Mandatory=$true)][string]$CargoTarget,
        [Parameter(Mandatory=$true)][string]$DbPath,
        [Parameter(Mandatory=$true)][string]$ProfilesDir,
        [Parameter(Mandatory=$true)][string]$LogDir,
        [Parameter(Mandatory=$true)][string]$TokenPath,
        [Parameter(Mandatory=$true)][string]$CodexToolSurfaceSnapshotPath,
        [Parameter(Mandatory=$true)][string]$TaskName,
        [Parameter(Mandatory=$true)][string]$MaintenanceLockPath,
        [Parameter(Mandatory=$true)][string]$InstalledBinarySha256,
        [string]$ActiveIssue,
        [object]$DeadOwnerDetail
    )

    $root = Join-Path $env:LOCALAPPDATA 'synapse\setup-continuations'
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    $stamp = (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssfffZ')
    $runId = "post-exit-$PID-$stamp"
    $runDir = Join-Path $root $runId
    New-Item -ItemType Directory -Force -Path $runDir | Out-Null
    $manifestPath = Join-Path $runDir 'continuation.json'
    $stdoutPath = Join-Path $runDir 'stdout.log'
    $stderrPath = Join-Path $runDir 'stderr.log'
    $wrapperPath = Join-Path $runDir 'launch-continuation.ps1'
    $launcherPath = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (-not (Test-Path -LiteralPath $launcherPath)) {
        Die "SYNAPSE_POST_EXIT_CONTINUATION_POWERSHELL_MISSING path=$launcherPath remediation=repair Windows PowerShell before retrying setup"
    }

    $args = @(
        '-NoProfile',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        $PSCommandPath,
        '-SourceDir',
        $SourceDir,
        '-Bind',
        $Bind,
        '-ExePath',
        $ExePath,
        '-ChromeNativeHostExePath',
        $ChromeNativeHostExePath,
        '-CargoTarget',
        $CargoTarget,
        '-DbPath',
        $DbPath,
        '-ProfilesDir',
        $ProfilesDir,
        '-LogDir',
        $LogDir,
        '-TokenPath',
        $TokenPath,
        '-CodexToolSurfaceSnapshotPath',
        $CodexToolSurfaceSnapshotPath,
        '-TaskName',
        $TaskName,
        '-MaintenanceLockPath',
        $MaintenanceLockPath,
        '-BuildTimeoutMinutes',
        ([string]$BuildTimeoutMinutes),
        '-PostExitParentPid',
        ([string]$PID),
        '-PostExitContinuationReason',
        'dead_owner_bind_after_install',
        '-PostExitManifestPath',
        $manifestPath,
        '-ForceRestart',
        '-SkipBuild',
        '-PostExitUseInstalledBinary',
        '-PostExitInstalledSha256',
        $InstalledBinarySha256
    )
    if (-not [string]::IsNullOrWhiteSpace($ActiveIssue)) {
        $args += @('-ActiveIssue', $ActiveIssue)
    }
    if ($SkipClientWiring) {
        $args += '-SkipClientWiring'
    }
    $continuationTaskName = "SynapsePostExitSetup-$runId"
    # Never hand Task Scheduler a console program directly: powershell.exe would flash
    # a visible terminal before -WindowStyle Hidden applies. run-hidden.vbs starts it
    # hidden from the windowless wscript.exe host and returns its exit code.
    $hiddenRunnerPath = Join-Path $PSScriptRoot 'run-hidden.vbs'
    if (-not (Test-Path -LiteralPath $hiddenRunnerPath -PathType Leaf)) {
        Die "SYNAPSE_HIDDEN_RUNNER_MISSING path=$hiddenRunnerPath remediation=restore scripts\run-hidden.vbs from the Synapse checkout; the continuation task must never start powershell.exe with a visible console"
    }
    $hiddenAction = Format-SynapseHiddenRunnerAction `
        -WscriptPath (Join-Path $env:SystemRoot 'System32\wscript.exe') `
        -RunnerPath $hiddenRunnerPath `
        -ArgumentList @($launcherPath, '-NoProfile', '-NonInteractive', '-WindowStyle', 'Hidden', '-ExecutionPolicy', 'Bypass', '-File', $wrapperPath)
    $taskArgument = $hiddenAction.Argument
    $argLiteralLines = @($args | ForEach-Object { "    $(Quote-PowerShellSingleQuotedString -Value $_)" })
    $wrapperLines = @(
        '$ErrorActionPreference = ''Stop''',
        "`$taskName = $(Quote-PowerShellSingleQuotedString -Value $continuationTaskName)",
        "`$launcherPath = $(Quote-PowerShellSingleQuotedString -Value $launcherPath)",
        "`$sourceDir = $(Quote-PowerShellSingleQuotedString -Value $SourceDir)",
        "`$stdoutPath = $(Quote-PowerShellSingleQuotedString -Value $stdoutPath)",
        "`$stderrPath = $(Quote-PowerShellSingleQuotedString -Value $stderrPath)",
        '$argList = @('
    )
    $wrapperLines += $argLiteralLines
    $wrapperLines += @(
        ')',
        '$exitCode = 1',
        'try {',
        '    Set-Location -LiteralPath $sourceDir',
        '    & $launcherPath @argList 1> $stdoutPath 2> $stderrPath 3>&1 4>&1 5>&1 6>&1',
        '    if ($null -ne $global:LASTEXITCODE) {',
        '        $exitCode = [int]$global:LASTEXITCODE',
        '    } else {',
        '        $exitCode = 0',
        '    }',
        '} catch {',
        '    try { ($_ | Out-String) | Add-Content -LiteralPath $stderrPath -Encoding UTF8 } catch {}',
        '    $exitCode = 1',
        '} finally {',
        '    try { Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue } catch {}',
        '}',
        'exit $exitCode'
    )
    Write-SynapseUtf8NoBomFile -Path $wrapperPath -Text (($wrapperLines -join "`n") + "`n")

    $manifest = [ordered]@{
        schema = 'synapse_setup_post_exit_continuation/v1'
        state = 'launching'
        run_id = $runId
        reason = $Reason
        bind = $Bind
        parent_pid = $PID
        launch_mode = 'scheduled_task'
        task_name = $continuationTaskName
        task_execute = $hiddenAction.Execute
        task_argument = $taskArgument
        launcher_path = $launcherPath
        wrapper_path = $wrapperPath
        setup_script_path = $PSCommandPath
        source_dir = $SourceDir
        stdout_log = $stdoutPath
        stderr_log = $stderrPath
        command_args = $args
        active_issue = $ActiveIssue
        dead_owner_detail = $DeadOwnerDetail
        created_at_utc = (Get-Date).ToUniversalTime().ToString('o')
        remediation = 'continuation waits for parent setup process exit, reacquires setup maintenance lock, then reruns setup with -SkipBuild against the installed verified daemon bytes'
    }
    Write-SynapseUtf8NoBomFile -Path $manifestPath -Text (($manifest | ConvertTo-Json -Depth 24) + "`n")

    try {
        if (Get-ScheduledTask -TaskName $continuationTaskName -ErrorAction SilentlyContinue) {
            Unregister-ScheduledTask -TaskName $continuationTaskName -Confirm:$false -ErrorAction SilentlyContinue
        }
        $action = New-ScheduledTaskAction -Execute $hiddenAction.Execute -Argument $hiddenAction.Argument -WorkingDirectory $SourceDir
        $trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5)
        $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Hours 2)
        $settings.Hidden = $true
        Register-ScheduledTask -TaskName $continuationTaskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Description "Synapse post-exit setup continuation $runId" | Out-Null
        Start-ScheduledTask -TaskName $continuationTaskName
        Start-Sleep -Seconds 1
        $child = Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.CommandLine -and ($_.CommandLine -like "*$wrapperPath*" -or $_.CommandLine -like "*$runId*") } |
            Sort-Object CreationDate -Descending |
            Select-Object -First 1
    } catch {
        Die "SYNAPSE_POST_EXIT_CONTINUATION_START_FAILED run_id=$runId manifest=$manifestPath error=$($_.Exception.Message) remediation=repair process creation permissions and rerun setup; no daemon was started while the bind probe failed"
    }

    $manifest.state = 'started'
    $manifest.child_pid = if ($child) { [int]$child.ProcessId } else { 0 }
    $manifest.task_state = try { (Get-ScheduledTask -TaskName $continuationTaskName -ErrorAction Stop).State.ToString() } catch { "unknown:$($_.Exception.Message)" }
    $manifest.started_at_utc = (Get-Date).ToUniversalTime().ToString('o')
    Write-SynapseUtf8NoBomFile -Path $manifestPath -Text (($manifest | ConvertTo-Json -Depth 24) + "`n")

    Info "SYNAPSE_POST_EXIT_CONTINUATION_STARTED run_id=$runId launch_mode=scheduled_task task_name=$continuationTaskName parent_pid=$PID child_pid=$($manifest.child_pid) manifest=$manifestPath wrapper=$wrapperPath stdout=$stdoutPath stderr=$stderrPath"
    return [pscustomobject]@{
        RunId = $runId
        RunDir = $runDir
        ManifestPath = $manifestPath
        StdoutPath = $stdoutPath
        StderrPath = $stderrPath
        ChildPid = $manifest.child_pid
        LaunchMode = 'scheduled_task'
        TaskName = $continuationTaskName
        WrapperPath = $wrapperPath
    }
}

function Write-SynapseCodexRestartHandoff {
    param(
        [Parameter(Mandatory=$true)][string]$Phase,
        [Parameter(Mandatory=$true)][string]$Reason,
        [AllowNull()]$CodexAncestor,
        [AllowNull()]$Surface,
        [AllowNull()]$Diff,
        [AllowNull()][string]$ProcessHashAtStart,
        [AllowNull()][string]$ProcessSnapshotAtStart,
        [AllowNull()][string]$CurrentSnapshotPath,
        [AllowNull()][string]$SourceDir,
        [AllowNull()][string]$Bind,
        [AllowNull()][string]$TokenPath,
        [AllowNull()][string]$ActiveIssue
    )

    $root = Join-Path $env:LOCALAPPDATA 'synapse\codex-restart-handoffs'
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
    $codexPid = if ($CodexAncestor) { [int]$CodexAncestor.ProcessId } else { 0 }
    $baseName = "codex-restart-handoff-$codexPid-$stamp"
    $jsonPath = Join-Path $root "$baseName.json"
    $mdPath = Join-Path $root "$baseName.md"
    $recoveryNotesPath = Get-SynapseRecoveryNotesPath -SourceDir $SourceDir
    $startSnapshotStatus = if ([string]::IsNullOrWhiteSpace($ProcessSnapshotAtStart)) {
        'missing_env'
    } elseif (Test-Path -LiteralPath $ProcessSnapshotAtStart) {
        'readable'
    } else {
        'missing_file'
    }

    $daemon = [ordered]@{
        bind = $Bind
        pid = if ($Surface -and $Surface.PSObject.Properties['daemon_pid']) { $Surface.daemon_pid } else { $null }
        pid_role = if ($Phase -eq 'pre_handoff_candidate') { 'preflight_candidate' } else { 'installed_configured_daemon' }
        pid_authoritative_for_configured_bind = ($Phase -ne 'pre_handoff_candidate')
        pid_expectation = if ($Phase -eq 'pre_handoff_candidate') {
            'This PID belongs to the isolated candidate daemon used before live handoff and is expected to be stopped before the configured daemon is installed.'
        } else {
            'This PID is the installed daemon observed after live handoff and should own the configured bind unless a later setup run superseded it.'
        }
        tool_count = if ($Surface -and $Surface.PSObject.Properties['tool_count']) { $Surface.tool_count } else { $null }
        tool_surface_sha256 = if ($Surface -and $Surface.PSObject.Properties['tool_surface_sha256']) { [string]$Surface.tool_surface_sha256 } else { $null }
        snapshot_path = $CurrentSnapshotPath
    }
    $codexProcess = if ($CodexAncestor) {
        [ordered]@{
            pid = [int]$CodexAncestor.ProcessId
            name = [string]$CodexAncestor.Name
            command_line = [string]$CodexAncestor.CommandLine
        }
    } else {
        [ordered]@{
            pid = $null
            name = $null
            command_line = $null
        }
    }
    $diffObject = ConvertTo-SynapseHandoffDiffObject -Diff $Diff
    $gitReadback = Get-SynapseHandoffGitReadback -SourceDir $SourceDir
    $activeIssueRef = Get-SynapseNormalizedIssueRef -Issue $ActiveIssue
    $activeIssueNumber = Get-SynapseIssueNumberFromRef -IssueRef $activeIssueRef
    $activeIssueRead = if ([string]::IsNullOrWhiteSpace($activeIssueNumber)) {
        $null
    } else {
        "gh issue view $activeIssueNumber --repo ChrisRoyse/Synapse --comments"
    }
    $resumeInstruction = if ([string]::IsNullOrWhiteSpace($activeIssueRef)) {
        'Resume the active GitHub issue from the caller/session context; if unknown, read the open issue queue and choose the issue that produced this setup handoff. Perform manual real-MCP FSV; do not use direct helper calls as acceptance.'
    } else {
        "Resume $activeIssueRef and perform manual real-MCP FSV; do not use direct helper calls as acceptance."
    }
    $restartCommandHint = if ([string]::IsNullOrWhiteSpace($activeIssueRef)) {
        "Close this Codex session completely, start a new Codex session through the patched Codex launcher, verify the active codex.exe PID is not $codexPid, then resume the active issue from the caller/session context."
    } else {
        "Close this Codex session completely, start a new Codex session through the patched Codex launcher, verify the active codex.exe PID is not $codexPid, then resume $activeIssueRef."
    }
    $postRestartRequiredReads = @(
        'C:\Users\hotra\Downloads\AICodingAgentSuperPrompt.md',
        'C:\code\Synapse\docs\compressionprompt.md',
        'C:\code\Synapse\AGENTS.md',
        $recoveryNotesPath
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) }
    $githubReads = @(
        'gh issue view 351 --repo ChrisRoyse/Synapse --comments',
        $activeIssueRead,
        'gh issue list --repo ChrisRoyse/Synapse --state open --limit 100'
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) }

    $record = [ordered]@{
        schema_version = 2
        artifact_kind = 'synapse_codex_restart_handoff'
        created_at_utc = [DateTime]::UtcNow.ToString('o')
        reason_code = 'SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE'
        reason = $Reason
        phase = $Phase
        required_restart = $true
        no_in_process_hot_refresh = $true
        explanation = 'The already-running Codex process has process-local MCP callable metadata that does not match the current daemon tools/list surface. Restart through the patched Codex launcher is the same-agent recovery boundary.'
        codex_process = $codexProcess
        daemon = $daemon
        current_process_start_surface = [ordered]@{
            env_hash_present = (-not [string]::IsNullOrWhiteSpace($ProcessHashAtStart))
            env_hash = $ProcessHashAtStart
            env_snapshot_path = $ProcessSnapshotAtStart
            snapshot_status = $startSnapshotStatus
        }
        diff = $diffObject
        post_restart_required_reads = $postRestartRequiredReads
        active_issue = [ordered]@{
            issue_ref = $activeIssueRef
            issue_number = $activeIssueNumber
            source = 'ActiveIssue parameter or SYNAPSE_ACTIVE_ISSUE environment variable'
            status = if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'unknown' } else { 'provided' }
        }
        stale_schema_context_issue = [ordered]@{
            issue_ref = '#1398'
            role = 'background context for the stale-schema bug class; not the resume target'
        }
        github_reads = $githubReads
        post_restart_verification = @(
            'Run git status --short --branch and confirm the working tree matches the handoff/recovery notes.',
            "Read the active Codex process parent chain and confirm the active codex.exe PID is not stale PID $codexPid from this handoff.",
            'Run deferred tool discovery for Synapse first, then call real mcp__synapse.health and verify daemon pid/tool_surface_sha256 matches or intentionally supersedes this handoff.',
            'If Synapse tool discovery, approval, or metadata is still stale, rerun scripts\synapse-setup.ps1 and keep the issue open.',
            $resumeInstruction
        )
        restart_command_hint = $restartCommandHint
        repo_readback = $gitReadback
        token_path = $TokenPath
        recovery_notes_path = $recoveryNotesPath
    }

    try {
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        Write-SynapseUtf8NoBomFile -Path $jsonPath -Text (($record | ConvertTo-Json -Depth 40) + "`n")
        $md = @(
            '# Synapse Codex Restart Handoff',
            '',
            "- Reason: SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE ($Reason)",
            "- Phase: $Phase",
            "- Created UTC: $($record.created_at_utc)",
            "- Codex PID: $codexPid",
            "- Daemon: pid=$($daemon.pid) bind=$($daemon.bind) tool_count=$($daemon.tool_count) tool_surface_sha256=$($daemon.tool_surface_sha256)",
            "- Active issue: $(if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'unknown; recover from caller/session context or open issue queue' } else { $activeIssueRef })",
            "- Stale-schema context issue: #1398 (background only; not the resume target)",
            "- Current process start snapshot: status=$startSnapshotStatus hash=$ProcessHashAtStart path=$ProcessSnapshotAtStart",
            "- Current daemon snapshot: $CurrentSnapshotPath",
            "- Diff: $($diffObject.summary)",
            '',
            '## Required Restart',
            "The running Codex process cannot hot-add changed MCP tools or mutate cached tool schemas. Close stale Codex PID $codexPid completely, restart Codex through the patched launcher, and prove the active codex.exe PID changed before continuing. Typing continue into the same PID is not a restart.",
            '',
            '## Read After Restart'
        )
        foreach ($item in $postRestartRequiredReads) {
            $md += "- $item"
        }
        $md += @(
            '',
            '## GitHub Reads'
        )
        foreach ($item in $record.github_reads) {
            $md += "- $item"
        }
        $md += @(
            '',
            '## Verification'
        )
        foreach ($item in $record.post_restart_verification) {
            $md += "- $item"
        }
        $md += @(
            '',
            "JSON artifact: $jsonPath",
            ''
        )
        Write-SynapseUtf8NoBomFile -Path $mdPath -Text (($md -join "`n") + "`n")

        if (-not [string]::IsNullOrWhiteSpace($recoveryNotesPath)) {
            $notes = @(
                '# Synapse Recovery Notes',
                '',
                '## Latest Codex Restart Handoff',
                '',
                "- Reason: SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE ($Reason)",
                "- Phase: $Phase",
                "- Created UTC: $($record.created_at_utc)",
                "- JSON: $jsonPath",
                "- Markdown: $mdPath",
                "- Stale Codex PID: $codexPid",
                "- Daemon bind: $Bind",
                "- Daemon tool surface: $($daemon.tool_surface_sha256)",
                "- Active issue: $(if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'unknown; recover from caller/session context or open issue queue' } else { $activeIssueRef })",
                "- Stale-schema context issue: #1398 (background only; not the resume target)",
                '',
                "After restart, re-read AGENTS.md, #351, $(if ([string]::IsNullOrWhiteSpace($activeIssueRef)) { 'the active issue from the caller/session context' } else { $activeIssueRef }), git status, and this file before resuming. #1398 is stale-schema background context only. Run deferred Synapse tool discovery before calling real mcp__synapse tools for FSV; direct helper calls are diagnostics only.",
                ''
            )
            Write-SynapseUtf8NoBomFile -Path $recoveryNotesPath -Text (($notes -join "`n") + "`n")
        }
    } catch {
        Die "SYNAPSE_CODEX_RESTART_HANDOFF_WRITE_FAILED phase=$Phase reason=$Reason path=$jsonPath error=$($_.Exception.Message) remediation=repair permissions on %LOCALAPPDATA%\synapse\codex-restart-handoffs and the repo STATE directory, then rerun setup"
    }

    Info "SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE handoff_written phase=$Phase reason=$Reason json=$jsonPath markdown=$mdPath recovery_notes=$recoveryNotesPath"
    return [pscustomobject]@{
        JsonPath = $jsonPath
        MarkdownPath = $mdPath
        RecoveryNotesPath = $recoveryNotesPath
    }
}

function Assert-CodexCandidateHandoffPreservesCurrentProcess {
    param(
        [AllowNull()]$CodexAncestor,
        [Parameter(Mandatory=$true)]$CandidateSurface,
        [AllowNull()][string]$ProcessHashAtStart,
        [AllowNull()][string]$ProcessSnapshotAtStart,
        [AllowNull()][string]$SourceDir,
        [AllowNull()][string]$Bind,
        [AllowNull()][string]$TokenPath,
        [AllowNull()][string]$ActiveIssue
    )

    if ($null -eq $CodexAncestor) {
        return
    }

    $candidateHash = [string]$CandidateSurface.tool_surface_sha256
    if ([string]::IsNullOrWhiteSpace($candidateHash)) {
        Die "SYNAPSE_CODEX_CANDIDATE_TOOL_SURFACE_HASH_MISSING codex_pid=$($CodexAncestor.ProcessId) remediation=candidate tools/list preflight did not produce a usable tool_surface_sha256; refusing to touch the live daemon"
    }

    if ($ProcessHashAtStart -eq $candidateHash) {
        Info "Codex current-process tool surface will survive candidate handoff codex_pid=$($CodexAncestor.ProcessId) tool_surface_sha256=$candidateHash tool_count=$($CandidateSurface.tool_count)"
        return
    }

    $startSurface = Read-SynapseCodexToolSurfaceSnapshotOrNull -Path $ProcessSnapshotAtStart
    $diff = Get-SynapseToolSurfaceDiff -StartSurface $startSurface -CurrentSurface $CandidateSurface
    $diffSummary = $diff.Summary

    if ([string]::IsNullOrWhiteSpace($ProcessHashAtStart)) {
        $handoff = Write-SynapseCodexRestartHandoff `
            -Phase 'pre_handoff_candidate' `
            -Reason 'start_snapshot_missing_before_candidate_handoff' `
            -CodexAncestor $CodexAncestor `
            -Surface $CandidateSurface `
            -Diff $diff `
            -ProcessHashAtStart $ProcessHashAtStart `
            -ProcessSnapshotAtStart $ProcessSnapshotAtStart `
            -CurrentSnapshotPath $null `
            -SourceDir $SourceDir `
            -Bind $Bind `
            -TokenPath $TokenPath `
            -ActiveIssue $ActiveIssue
        Info ("WARN: SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE_PRE_HANDOFF codex_pid={0} tool_surface_at_process_start=missing candidate_tool_surface_sha256={1} candidate_tool_count={2} candidate_pid={3} start_snapshot={4} handoff={5} {6} remediation=setup will continue only to install the verified daemon; final setup must fail closed if this Codex process remains stale." -f `
            $CodexAncestor.ProcessId,
            $candidateHash,
            $CandidateSurface.tool_count,
            $CandidateSurface.daemon_pid,
            $ProcessSnapshotAtStart,
            $handoff.JsonPath,
            $diffSummary)
        return
    }

    if (-not $diff.HasRestartRequired) {
        Info ("Codex current-process tool surface hash will change after candidate handoff but callable schema is unchanged; continuing codex_pid={0} start_tool_surface_sha256={1} candidate_tool_surface_sha256={2} candidate_tool_count={3} candidate_pid={4} start_snapshot={5} {6}" -f `
            $CodexAncestor.ProcessId,
            $ProcessHashAtStart,
            $candidateHash,
            $CandidateSurface.tool_count,
            $CandidateSurface.daemon_pid,
            $ProcessSnapshotAtStart,
            $diffSummary)
        return
    }

    $handoff = Write-SynapseCodexRestartHandoff `
        -Phase 'pre_handoff_candidate' `
        -Reason 'start_snapshot_hash_mismatch_before_candidate_handoff' `
        -CodexAncestor $CodexAncestor `
        -Surface $CandidateSurface `
        -Diff $diff `
        -ProcessHashAtStart $ProcessHashAtStart `
        -ProcessSnapshotAtStart $ProcessSnapshotAtStart `
        -CurrentSnapshotPath $null `
        -SourceDir $SourceDir `
        -Bind $Bind `
        -TokenPath $TokenPath `
        -ActiveIssue $ActiveIssue
    Info ("WARN: SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE_PRE_HANDOFF codex_pid={0} start_tool_surface_sha256={1} candidate_tool_surface_sha256={2} candidate_tool_count={3} candidate_pid={4} start_snapshot={5} handoff={6} {7} remediation=setup will continue only to install the verified daemon; final setup must fail closed if this Codex process remains stale." -f `
        $CodexAncestor.ProcessId,
        $ProcessHashAtStart,
        $candidateHash,
        $CandidateSurface.tool_count,
        $CandidateSurface.daemon_pid,
        $ProcessSnapshotAtStart,
        $handoff.JsonPath,
        $diffSummary)
    return
}

function Assert-CodexCurrentProcessToolSurfaceFresh {
    param(
        [AllowNull()]$CodexAncestor,
        [Parameter(Mandatory=$true)]$CurrentSurface,
        [AllowNull()][string]$ProcessHashAtStart,
        [AllowNull()][string]$ProcessSnapshotAtStart,
        [Parameter(Mandatory=$true)][string]$SnapshotPath,
        [AllowNull()][string]$SourceDir,
        [AllowNull()][string]$Bind,
        [AllowNull()][string]$TokenPath,
        [AllowNull()][string]$ActiveIssue,
        [switch]$NonFatal
    )

    if ($null -eq $CodexAncestor) {
        return
    }

    $currentHash = [string]$CurrentSurface.tool_surface_sha256
    $startSurface = Read-SynapseCodexToolSurfaceSnapshotOrNull -Path $ProcessSnapshotAtStart
    $diff = Get-SynapseToolSurfaceDiff -StartSurface $startSurface -CurrentSurface $CurrentSurface
    $diffSummary = $diff.Summary
    if ([string]::IsNullOrWhiteSpace($ProcessHashAtStart)) {
        $handoff = Write-SynapseCodexRestartHandoff `
            -Phase 'post_handoff_current_daemon' `
            -Reason 'start_snapshot_missing_after_daemon_handoff' `
            -CodexAncestor $CodexAncestor `
            -Surface $CurrentSurface `
            -Diff $diff `
            -ProcessHashAtStart $ProcessHashAtStart `
            -ProcessSnapshotAtStart $ProcessSnapshotAtStart `
            -CurrentSnapshotPath $SnapshotPath `
            -SourceDir $SourceDir `
            -Bind $Bind `
            -TokenPath $TokenPath `
            -ActiveIssue $ActiveIssue
        $message = ("SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE codex_pid={0} tool_surface_at_process_start=missing current_tool_surface_sha256={1} tool_count={2} daemon_pid={3} snapshot={4} start_snapshot={5} handoff={6} {7} remediation=restart Codex through the patched launcher, read the handoff plus STATE\\RECOVERY_NOTES.md, then resume the active issue named in the handoff and verify real mcp__synapse metadata." -f `
            $CodexAncestor.ProcessId,
            $currentHash,
            $CurrentSurface.tool_count,
            $CurrentSurface.daemon_pid,
            $SnapshotPath,
            $ProcessSnapshotAtStart,
            $handoff.JsonPath,
            $diffSummary)
        if ($NonFatal) {
            Info "WARN: $message"
            return
        }
        Die $message
    }

    if ($ProcessHashAtStart -ne $currentHash) {
        if (-not $diff.HasRestartRequired) {
            Info ("Codex current-process tool surface hash changed but callable schema is unchanged; continuing without restart handoff codex_pid={0} start_tool_surface_sha256={1} current_tool_surface_sha256={2} tool_count={3} daemon_pid={4} snapshot={5} start_snapshot={6} {7}" -f `
                $CodexAncestor.ProcessId,
                $ProcessHashAtStart,
                $currentHash,
                $CurrentSurface.tool_count,
                $CurrentSurface.daemon_pid,
                $SnapshotPath,
                $ProcessSnapshotAtStart,
                $diffSummary)
            return
        }
        $handoff = Write-SynapseCodexRestartHandoff `
            -Phase 'post_handoff_current_daemon' `
            -Reason 'start_snapshot_hash_mismatch_after_daemon_handoff' `
            -CodexAncestor $CodexAncestor `
            -Surface $CurrentSurface `
            -Diff $diff `
            -ProcessHashAtStart $ProcessHashAtStart `
            -ProcessSnapshotAtStart $ProcessSnapshotAtStart `
            -CurrentSnapshotPath $SnapshotPath `
            -SourceDir $SourceDir `
            -Bind $Bind `
            -TokenPath $TokenPath `
            -ActiveIssue $ActiveIssue
        $message = ("SYNAPSE_CODEX_CURRENT_PROCESS_SCHEMA_STALE codex_pid={0} tool_surface_at_process_start=mismatch start_tool_surface_sha256={1} current_tool_surface_sha256={2} tool_count={3} daemon_pid={4} snapshot={5} start_snapshot={6} handoff={7} {8} remediation=restart Codex through the patched launcher, read the handoff plus STATE\\RECOVERY_NOTES.md, then resume the active issue named in the handoff and verify real mcp__synapse metadata." -f `
            $CodexAncestor.ProcessId,
            $ProcessHashAtStart,
            $currentHash,
            $CurrentSurface.tool_count,
            $CurrentSurface.daemon_pid,
            $SnapshotPath,
            $ProcessSnapshotAtStart,
            $handoff.JsonPath,
            $diffSummary)
        if ($NonFatal) {
            Info "WARN: $message"
            return
        }
        Die $message
    }

    Info "Codex current-process tool surface matches daemon snapshot codex_pid=$($CodexAncestor.ProcessId) tool_surface_sha256=$currentHash tool_count=$($CurrentSurface.tool_count)"
}

function Request-SynapseGracefulShutdown {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)][int[]]$ExpectedPids,
        [Parameter(Mandatory=$true)][string]$Reason
    )

    try {
        $response = Invoke-RestMethod `
            -Method Post `
            -Uri "http://$Bind/shutdown" `
            -Headers @{ Authorization = "Bearer $Token" } `
            -UserAgent "synapse-setup/$Reason" `
            -TimeoutSec 4
    } catch {
        return [pscustomobject]@{ Ok = $false; Code = 'SYNAPSE_GRACEFUL_SHUTDOWN_REQUEST_FAILED'; Response = $null; Error = $_.Exception.Message }
    }

    $responsePid = 0
    try {
        $responsePid = [int]$response.pid
    } catch {
        return [pscustomobject]@{ Ok = $false; Code = 'SYNAPSE_GRACEFUL_SHUTDOWN_PID_UNREADABLE'; Response = $response; Error = $_.Exception.Message }
    }

    if ($ExpectedPids -notcontains $responsePid) {
        return [pscustomobject]@{
            Ok = $false
            Code = 'SYNAPSE_GRACEFUL_SHUTDOWN_PID_MISMATCH'
            Response = $response
            Error = "response_pid=$responsePid expected_pids=$($ExpectedPids -join ',')"
        }
    }

    if ($response.ok -ne $true -or "$($response.shutdown)" -ne 'requested') {
        return [pscustomobject]@{
            Ok = $false
            Code = 'SYNAPSE_GRACEFUL_SHUTDOWN_RESPONSE_INVALID'
            Response = $response
            Error = "response=$($response | ConvertTo-Json -Compress -Depth 6)"
        }
    }

    [pscustomobject]@{ Ok = $true; Code = 'OK'; Response = $response; Error = $null }
}

function Read-SynapseHttpErrorResponseBody {
    param([Parameter(Mandatory=$true)]$ErrorRecord)

    $response = $ErrorRecord.Exception.Response
    if ($null -eq $response) {
        return $null
    }
    try {
        $stream = $response.GetResponseStream()
        if ($null -eq $stream) {
            return $null
        }
        $reader = New-Object System.IO.StreamReader($stream)
        try {
            return $reader.ReadToEnd()
        } finally {
            $reader.Dispose()
        }
    } catch {
        return "SYNAPSE_HTTP_ERROR_BODY_READ_FAILED error=$($_.Exception.Message)"
    }
}

function Read-SynapseHttpErrorStatus {
    param([Parameter(Mandatory=$true)]$ErrorRecord)

    $response = $ErrorRecord.Exception.Response
    if ($null -eq $response) {
        return $null
    }
    try {
        return [int]$response.StatusCode
    } catch {
        return $null
    }
}

function Request-SynapseChromeBridgeMaintenancePause {
    param(
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$Token,
        [Parameter(Mandatory=$true)][string]$Reason,
        [int]$PauseMs = $SynapseChromeBridgeMaintenancePauseMs,
        [int]$HealthTimeoutSec = $SynapseChromeBridgeMaintenancePauseHealthTimeoutSec
    )

    # #140: the daemon answers a degraded payload when /health exceeds its 3 s
    # bound (old daemons omit chrome_bridge, new ones report it as unknown).
    # That is a slow host, not a missing bridge: retry within the budget, then
    # skip the pause with a warning instead of aborting the restart.
    $degradedAttempts = 0
    while ($true) {
        try {
            $health = Invoke-RestMethod `
                -Method Get `
                -Uri "http://$Bind/health" `
                -Headers @{ Authorization = "Bearer $Token" } `
                -UserAgent "synapse-setup/$Reason" `
                -TimeoutSec $HealthTimeoutSec
        } catch {
            return [pscustomobject]@{
                Ok = $false
                Skipped = $false
                Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_HEALTH_FAILED'
                Response = $null
                Error = $_.Exception.Message
                Detail = $null
            }
        }

        $healthSubsystem = $health.subsystems.health
        $bridgeProbe = $health.subsystems.chrome_bridge
        $healthDegraded = ($null -ne $healthSubsystem -and "$($healthSubsystem.status)" -eq 'degraded') -or
            ($null -ne $bridgeProbe -and "$($bridgeProbe.status)" -eq 'unknown')
        if (-not $healthDegraded) {
            break
        }
        if ($degradedAttempts -ge $SynapseChromeBridgeMaintenancePauseDegradedRetries) {
            $degradedDetail = "health degraded after $($degradedAttempts + 1) probes: $($healthSubsystem.detail) chrome_bridge=$($bridgeProbe.detail)"
            Write-Warning "SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SKIPPED_HEALTH_DEGRADED reason=$Reason bind=$Bind $degradedDetail; continuing without a bridge pause"
            return [pscustomobject]@{
                Ok = $true
                Skipped = $true
                Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SKIPPED_HEALTH_DEGRADED'
                Response = $health
                Error = $null
                Detail = $degradedDetail
            }
        }
        $degradedAttempts++
        Start-Sleep -Seconds $SynapseChromeBridgeMaintenancePauseDegradedRetryDelaySec
    }

    $chromeBridge = $health.subsystems.chrome_bridge
    if ($null -eq $chromeBridge) {
        return [pscustomobject]@{
            Ok = $false
            Skipped = $false
            Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_HEALTH_MISSING'
            Response = $health
            Error = 'health.subsystems.chrome_bridge missing'
            Detail = $null
        }
    }

    $status = "$($chromeBridge.status)"
    $detail = "$($chromeBridge.detail)"
    if ($detail -match 'no_active_chrome_bridge_host' -or $status -in @('unavailable', 'disconnected', 'disabled')) {
        return [pscustomobject]@{
            Ok = $true
            Skipped = $true
            Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SKIPPED_NO_ACTIVE_HOST'
            Response = $health
            Error = $null
            Detail = $detail
        }
    }
    if ([string]::IsNullOrWhiteSpace($status)) {
        return [pscustomobject]@{
            Ok = $false
            Skipped = $false
            Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_STATUS_UNREADABLE'
            Response = $health
            Error = 'health.subsystems.chrome_bridge.status missing'
            Detail = $detail
        }
    }

    $body = [ordered]@{
        pause_ms = $PauseMs
        reason = $Reason
    } | ConvertTo-Json -Compress -Depth 4

    $attempts = @()
    $maxAttempts = 5
    for ($attempt = 1; $attempt -le $maxAttempts; $attempt++) {
        try {
            $response = Invoke-RestMethod `
                -Method Post `
                -Uri "http://$Bind/chrome-debugger/native/maintenance-pause" `
                -Headers @{ Authorization = "Bearer $Token" } `
                -ContentType 'application/json' `
                -UserAgent "synapse-setup/$Reason" `
                -Body $body `
                -TimeoutSec 8
        } catch {
            $responseBody = Read-SynapseHttpErrorResponseBody -ErrorRecord $_
            $statusCode = Read-SynapseHttpErrorStatus -ErrorRecord $_
            $attempts += [pscustomobject]@{
                attempt = $attempt
                code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_REQUEST_FAILED'
                ok = $false
                status = $statusCode
                error = $_.Exception.Message
                response = $responseBody
            }
            if ($attempt -lt $maxAttempts) {
                Start-Sleep -Seconds 1
                continue
            }
            return [pscustomobject]@{
                Ok = $false
                Skipped = $false
                Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_REQUEST_FAILED'
                Response = $attempts
                Error = "attempts=$($attempts.Count) last_status=$statusCode message=$($_.Exception.Message)"
                Detail = $detail
                Attempts = $attempts
            }
        }

        $pause = $response.pause
        $websocketClose = $null
        if ($null -ne $pause) {
            $websocketClose = $pause.websocket_close
        }
        $activeSocketWasOpen = $false
        if ($null -ne $websocketClose -and $null -ne $websocketClose.ready_state_before) {
            $readyStateBefore = [int]$websocketClose.ready_state_before
            $activeSocketWasOpen = ($readyStateBefore -eq 0 -or $readyStateBefore -eq 1)
        }
        $websocketCloseFailed = $false
        if ($null -eq $websocketClose) {
            $websocketCloseFailed = $true
        } elseif ($null -ne $websocketClose.close_error) {
            $websocketCloseFailed = $true
        } elseif ($activeSocketWasOpen -and $websocketClose.close_requested -ne $true) {
            $websocketCloseFailed = $true
        }

        $attemptReadback = [pscustomobject]@{
            attempt = $attempt
            code = 'OK'
            ok = ($response.ok -eq $true)
            status = 200
            pause_ms = if ($null -eq $pause) { $null } else { $pause.pause_ms }
            reconnect_suppressed = if ($null -eq $pause) { $null } else { $pause.reconnect_suppressed }
            persisted = if ($null -eq $pause) { $null } else { $pause.persisted }
            websocket_close = $websocketClose
        }
        $attempts += $attemptReadback

        if ($response.ok -eq $true -and $null -ne $pause -and $pause.reconnect_suppressed -eq $true -and $pause.persisted -eq $true -and -not $websocketCloseFailed) {
            return [pscustomobject]@{
                Ok = $true
                Skipped = $false
                Code = 'OK'
                Response = $response
                Error = $null
                Detail = $detail
                Attempts = $attempts
            }
        }

        $attemptReadback.code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_RESPONSE_INVALID'
        if ($attempt -lt $maxAttempts) {
            Start-Sleep -Seconds 1
        }
    }

    [pscustomobject]@{
        Ok = $false
        Skipped = $false
        Code = 'SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_RESPONSE_INVALID'
        Response = $attempts
        Error = "attempts=$($attempts.Count) last_response=$($attempts[-1] | ConvertTo-Json -Compress -Depth 8)"
        Detail = $detail
        Attempts = $attempts
    }
}

function Get-SynapseActiveSessionCount {
    param([Parameter(Mandatory=$true)]$Health)

    $value = $Health.subsystems.http.active_sessions
    if ($null -eq $value) {
        return $null
    }

    try {
        return [int]$value
    } catch {
        return $null
    }
}

function Assert-SynapseRestartAllowed {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath,
        [Parameter(Mandatory=$true)][string]$TokenPath,
        [switch]$ForceRestart,
        [switch]$AllowActiveClientDrain
    )

    $allProcesses = @(Get-SynapseMcpProcessSnapshot)
    $processes = @(Select-SynapseMcpDeployTargetProcesses -Snapshot $allProcesses -Bind $Bind -DbPath $DbPath)
    $ignoredProcesses = @(Select-SynapseMcpDeployTargetProcesses -Snapshot $allProcesses -Bind $Bind -DbPath $DbPath -Invert)
    if ($ignoredProcesses.Count -gt 0) {
        Info ("Synapse restart guard reason={0} ignored_non_target_process_count={1}`nignored:`n{2}" -f `
            $Reason,
            $ignoredProcesses.Count,
            (Format-SynapseMcpProcessSnapshot -Snapshot $ignoredProcesses))
    }
    if ($processes.Count -eq 0) {
        Info "Synapse restart guard reason=$Reason target_process_count=0 ignored_non_target_process_count=$($ignoredProcesses.Count) verdict=clear"
        return
    }

    $null = Get-SynapseBindEndpoint -Bind $Bind
    $nonHttpProcesses = @($processes | Where-Object { $_.CommandLine -notmatch '(?i)--mode\s+http' })
    $tokenRead = Read-SynapseSetupTokenForRestartGuard -TokenPath $TokenPath
    if (-not $tokenRead.Ok) {
        $message = "$($tokenRead.Code) reason=$Reason process_count=$($processes.Count) $($tokenRead.Detail) remediation=do not restart blindly while the daemon may have clients; repair token state or rerun with -ForceRestart after coordinating a maintenance window"
        if ($ForceRestart) {
            Info "FORCE_RESTART: $message"
        } else {
            Die $message
        }
    }

    $activeSessions = $null
    $healthRead = $null
    if ($tokenRead.Ok) {
        $healthRead = Read-SynapseHealthForRestartGuard -Bind $Bind -Token $tokenRead.Token
        if (-not $healthRead.Ok) {
            $message = "SYNAPSE_RESTART_GUARD_HEALTH_UNREADABLE reason=$Reason bind=$Bind error=$($healthRead.Error) remediation=do not restart blindly; repair the daemon/token or rerun with -ForceRestart after coordinating a maintenance window"
            if ($ForceRestart) {
                Info "FORCE_RESTART: $message"
            } else {
                Die $message
            }
        }
    }

    if ($tokenRead.Ok -and $healthRead.Ok) {
        $activeSessions = Get-SynapseActiveSessionCount -Health $healthRead.Health
        if ($null -eq $activeSessions) {
            $message = "SYNAPSE_RESTART_GUARD_ACTIVE_SESSIONS_UNREADABLE reason=$Reason bind=$Bind remediation=health did not expose subsystems.http.active_sessions; do not restart blindly"
            if ($ForceRestart) {
                Info "FORCE_RESTART: $message"
            } else {
                Die $message
            }
        }
    }

    $tcpConnections = @(Get-SynapseTcpClientSnapshot -Bind $Bind)
    $setupLineagePids = @((Get-ProcessLineage -StartPid $PID) | ForEach-Object { [int]$_.ProcessId })
    $selfProbeTcpClients = @($tcpConnections | Where-Object {
        $_.HasLivePeer -and $setupLineagePids -contains [int]$_.PeerOwningProcess
    })
    $tcpClients = @($tcpConnections | Where-Object {
        $_.HasLivePeer -and $setupLineagePids -notcontains [int]$_.PeerOwningProcess
    })
    $staleTcpConnections = @($tcpConnections | Where-Object { -not $_.HasLivePeer })
    $blockers = @()
    $clientDrainBlockers = @()
    if ($nonHttpProcesses.Count -gt 0) { $blockers += "non_http_synapse_processes=$($nonHttpProcesses.Count)" }
    if ($tcpClients.Count -gt 0) {
        $blockers += "live_tcp_clients=$($tcpClients.Count)"
        $clientDrainBlockers += "live_tcp_clients=$($tcpClients.Count)"
    }
    if ($null -ne $activeSessions -and $activeSessions -gt 0 -and $tcpClients.Count -gt 0) {
        $blockers += "active_sessions=$activeSessions"
        $clientDrainBlockers += "active_sessions=$activeSessions"
    }
    if ($null -ne $activeSessions -and $activeSessions -gt 0 -and $tcpClients.Count -eq 0) {
        Info "Synapse restart guard reason=$Reason idle_session_map_entries=$activeSessions live_tcp_clients=0 verdict=not_blocking_idle_sessions"
    }
    if ($staleTcpConnections.Count -gt 0) {
        Info ("Synapse restart guard reason={0} stale_tcp_connections={1}`nstale_tcp:`n{2}" -f `
            $Reason,
            $staleTcpConnections.Count,
            (Format-SynapseTcpClientSnapshot -Snapshot $staleTcpConnections))
    }
    if ($selfProbeTcpClients.Count -gt 0) {
        Info ("Synapse restart guard reason={0} self_probe_tcp_connections={1}`nself_probe_tcp:`n{2}" -f `
            $Reason,
            $selfProbeTcpClients.Count,
            (Format-SynapseTcpClientSnapshot -Snapshot $selfProbeTcpClients))
    }

    if ($blockers.Count -gt 0) {
        $message = ("SYNAPSE_ACTIVE_CLIENTS_PRESENT reason={0} blockers={1} process_count={2} active_sessions={3} live_tcp_clients={4} idle_session_map_entries={5} stale_tcp_connections={6}`nprocesses:`n{7}`ntcp_clients:`n{8}`nstale_tcp:`n{9}`nremediation=wait for MCP clients to disconnect, close only the exact owner-known helper process listed here, or rerun with -ForceRestart only after coordinating a maintenance window. Do not close terminal windows or broad shell processes." -f `
            $Reason,
            ($blockers -join ','),
            $processes.Count,
            ($(if ($null -eq $activeSessions) { 'unknown' } else { $activeSessions })),
            $tcpClients.Count,
            ($(if ($null -eq $activeSessions) { 'unknown' } else { $activeSessions })),
            $staleTcpConnections.Count,
            (Format-SynapseMcpProcessSnapshot -Snapshot $processes),
            (Format-SynapseTcpClientSnapshot -Snapshot $tcpClients),
            (Format-SynapseTcpClientSnapshot -Snapshot $staleTcpConnections))
        if ($nonHttpProcesses.Count -gt 0 -and -not $ForceRestart) {
            Die $message
        } elseif ($AllowActiveClientDrain -and $clientDrainBlockers.Count -gt 0 -and $nonHttpProcesses.Count -eq 0) {
            Info ("Synapse restart guard reason={0} verdict=drain_permitted blockers={1} active_sessions={2} live_tcp_clients={3} stale_tcp_connections={4} process_count={5} drain=authenticated_http_shutdown" -f `
                $Reason,
                ($clientDrainBlockers -join ','),
                ($(if ($null -eq $activeSessions) { 'unknown' } else { $activeSessions })),
                $tcpClients.Count,
                $staleTcpConnections.Count,
                $processes.Count)
        } elseif ($ForceRestart) {
            Info "FORCE_RESTART: $message"
        } else {
            Die $message
        }
    } else {
        Info "Synapse restart guard reason=$Reason verdict=clear active_sessions=$activeSessions live_tcp_clients=0 stale_tcp_connections=$($staleTcpConnections.Count) process_count=$($processes.Count)"
    }
}

function Assert-SynapseProcessStopTarget {
    param(
        [Parameter(Mandatory=$true)]$SnapshotProcess,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath
    )

    $pidValue = [int]$SnapshotProcess.ProcessId
    $current = Get-CimInstance Win32_Process -Filter "ProcessId=$pidValue" -ErrorAction SilentlyContinue
    if (-not $current) {
        Info "Synapse process stop target already exited pid=$pidValue"
        return $null
    }

    $protectedNames = @(
        'cmd.exe',
        'powershell.exe',
        'pwsh.exe',
        'WindowsTerminal.exe',
        'OpenConsole.exe',
        'conhost.exe',
        'wsl.exe',
        'wslhost.exe',
        'Code.exe'
    )
    if ($protectedNames -contains $current.Name) {
        Die ("SYNAPSE_PROTECTED_PROCESS_STOP_REFUSED pid={0} name={1} command_line={2} remediation=terminal/IDE/WSL host processes are operator and agent workspaces; never close them from setup, tests, or FSV. Stop only exact owner-known helper PIDs." -f `
            $pidValue,
            $current.Name,
            $current.CommandLine)
    }

    $exeLeaf = if ($current.ExecutablePath) { Split-Path -Leaf $current.ExecutablePath } else { '' }
    if ($current.Name -ine 'synapse-mcp.exe' -and $exeLeaf -ine 'synapse-mcp.exe') {
        Die ("SYNAPSE_PROCESS_STOP_TARGET_MISMATCH pid={0} expected=synapse-mcp.exe actual_name={1} actual_path={2} command_line={3} remediation=PID was reused or snapshot was not a Synapse process; refusing exact-PID stop" -f `
            $pidValue,
            $current.Name,
            $current.ExecutablePath,
            $current.CommandLine)
    }
    if ($current.CommandLine -notmatch '(?i)synapse-mcp(\.exe)?') {
        Die ("SYNAPSE_PROCESS_STOP_TARGET_UNVERIFIED pid={0} name={1} command_line={2} remediation=command line does not prove a Synapse MCP target; refusing exact-PID stop" -f `
            $pidValue,
            $current.Name,
            $current.CommandLine)
    }

    $targetMatch = Get-SynapseMcpDeployTargetMatch -Process $current -Bind $Bind -DbPath $DbPath
    if (-not $targetMatch.IsMatch) {
        Die ("SYNAPSE_PROCESS_STOP_TARGET_SCOPE_MISMATCH pid={0} bind={1} db={2} actual_bind_arg={3} actual_db_arg={4} command_line={5} remediation=setup only stops synapse-mcp.exe processes whose command line targets the deployed --bind or --db; refusing collateral stop" -f `
            $pidValue,
            $Bind,
            $targetMatch.ExpectedDb,
            $targetMatch.BindArg,
            $targetMatch.DbArg,
            $current.CommandLine)
    }
    $rules = $targetMatch.Rules -join ','
    $current | Add-Member -NotePropertyName DeployTargetMatched -NotePropertyValue $true -Force
    $current | Add-Member -NotePropertyName DeployTargetRules -NotePropertyValue $rules -Force
    $current | Add-Member -NotePropertyName DeployTargetBindArg -NotePropertyValue $targetMatch.BindArg -Force
    $current | Add-Member -NotePropertyName DeployTargetDbArg -NotePropertyValue $targetMatch.DbArg -Force
    Info "Synapse process stop target verified pid=$pidValue match_rules=$rules path=$($current.ExecutablePath) cmd=$($current.CommandLine)"

    return $current
}

function Stop-SynapseMcpProcesses {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$Bind,
        [Parameter(Mandatory=$true)][string]$DbPath,
        [Parameter(Mandatory=$true)][string]$TokenPath,
        [switch]$ForceRestart,
        [int]$TimeoutSeconds = 15
    )

    $allBefore = @(Get-SynapseMcpProcessSnapshot)
    $before = @(Select-SynapseMcpDeployTargetProcesses -Snapshot $allBefore -Bind $Bind -DbPath $DbPath)
    $ignoredBefore = @(Select-SynapseMcpDeployTargetProcesses -Snapshot $allBefore -Bind $Bind -DbPath $DbPath -Invert)
    Info "Synapse process stop requested reason=$Reason before_all_count=$($allBefore.Count) target_count=$($before.Count) ignored_non_target_count=$($ignoredBefore.Count) bind=$Bind db=$DbPath"
    Info ("Synapse process stop target candidates:`n{0}" -f (Format-SynapseMcpProcessSnapshot -Snapshot $before))
    if ($ignoredBefore.Count -gt 0) {
        Info ("Synapse process stop ignored non-target processes:`n{0}" -f (Format-SynapseMcpProcessSnapshot -Snapshot $ignoredBefore))
    }
    if ($before.Count -eq 0) {
        Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds $TimeoutSeconds -ForceRestart:$ForceRestart
        return
    }

    foreach ($proc in $before) {
        $null = Assert-SynapseProcessStopTarget -SnapshotProcess $proc -Bind $Bind -DbPath $DbPath
    }

    $httpProcesses = @($before | Where-Object { $_.CommandLine -match '(?i)--mode\s+http' })
    $nonHttpProcesses = @($before | Where-Object { $_.CommandLine -notmatch '(?i)--mode\s+http' })
    if ($nonHttpProcesses.Count -gt 0 -and -not $ForceRestart) {
        Die ("SYNAPSE_GRACEFUL_SHUTDOWN_NON_HTTP_PROCESS reason={0} count={1}`nprocesses:`n{2}`nremediation=setup will not force-stop stdio/bridge/non-http Synapse processes without -ForceRestart; run synapse-mcp --mode doctor to inspect ownership, or coordinate a maintenance window before forcing exact verified PIDs. Do not close terminal windows." -f `
            $Reason,
            $nonHttpProcesses.Count,
            (Format-SynapseMcpProcessSnapshot -Snapshot $nonHttpProcesses))
    }

    if ($httpProcesses.Count -gt 0) {
        if ($ForceRestart) {
            $liveTcpClients = @(Get-SynapseTcpClientSnapshot -Bind $Bind | Where-Object { $_.HasLivePeer })
            if ($liveTcpClients.Count -gt 0) {
                $targetPids = (($httpProcesses | ForEach-Object { [int]$_.ProcessId }) -join ',')
                Info ("FORCE_RESTART: SYNAPSE_FORCE_RESTART_LIVE_CLIENTS_GRACEFUL_FIRST reason={0} bind={1} target_pids={2} live_tcp_client_count={3}`ntcp_clients:`n{4}`nremediation=-ForceRestart is explicit maintenance. Setup still asks the HTTP daemon to shut down first so it can close accepted sockets cleanly; if Windows keeps dead-owner rows because client peers remain connected, setup closes only exact known non-terminal peers and then requires a normal bind probe before installing or starting anything." -f `
                    $Reason,
                    $Bind,
                    $targetPids,
                    $liveTcpClients.Count,
                    (Format-SynapseTcpClientSnapshot -Snapshot $liveTcpClients))
            }
        }

        $tokenRead = Read-SynapseSetupTokenForRestartGuard -TokenPath $TokenPath
        if (-not $tokenRead.Ok) {
            $message = ("{0} reason={1} process_count={2} {3} remediation=graceful shutdown requires the daemon bearer token; repair token state before setup, or use -ForceRestart only after manually verifying no held inputs and no live clients." -f `
                $tokenRead.Code,
                $Reason,
                $httpProcesses.Count,
                $tokenRead.Detail)
            if ($ForceRestart) {
                Info "FORCE_RESTART: $message"
            } else {
                Die $message
            }
        } else {
            $expectedPids = @($httpProcesses | ForEach-Object { [int]$_.ProcessId })
            if ($ForceRestart) {
                $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = $null
                $pause = Request-SynapseChromeBridgeMaintenancePause -Bind $Bind -Token $tokenRead.Token -Reason $Reason -PauseMs $SynapseChromeBridgeMaintenancePauseMs
                if (-not $pause.Ok) {
                    Die ("{0} reason={1} bind={2} error={3} detail={4} response={5} remediation=forced daemon maintenance requires the already-open Chrome bridge to acknowledge a bounded reconnect pause before shutdown when an active bridge host exists. Reload the installed bridge through the existing Chrome profile or inspect daemon/extension logs; setup will not chase an unbounded stream of recreated Chrome NetworkService peers." -f `
                        $pause.Code,
                        $Reason,
                        $Bind,
                        $pause.Error,
                        $pause.Detail,
                        ($(if ($null -eq $pause.Response) { '<none>' } else { $pause.Response | ConvertTo-Json -Compress -Depth 8 })))
                }
                if ($pause.Skipped) {
                    Info ("FORCE_RESTART: {0} reason={1} bind={2} detail={3}" -f `
                        $pause.Code,
                        $Reason,
                        $Bind,
                        $pause.Detail)
                } else {
                    $pauseUntil = $pause.Response.pause.pause_until_unix_ms
                    if ($null -ne $pauseUntil) {
                        try {
                            $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = [int64]$pauseUntil
                        } catch {
                            $script:SynapseChromeBridgeMaintenancePauseUntilUnixMs = $null
                        }
                    }
                    Info ("FORCE_RESTART: SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_ACK reason={0} bind={1} pause={2}" -f `
                        $Reason,
                        $Bind,
                        ($pause.Response.pause | ConvertTo-Json -Compress -Depth 8))
                    $webSocketClose = $pause.Response.pause.websocket_close
                    if ($webSocketClose -and $webSocketClose.had_socket -eq $true -and $webSocketClose.close_requested -eq $true) {
                        Info ("FORCE_RESTART: SYNAPSE_CHROME_BRIDGE_MAINTENANCE_PAUSE_SOCKET_DRAIN_WAIT reason={0} bind={1} wait_ms={2} had_socket={3} close_requested={4} close_deferred={5} remediation=the bridge intentionally sends the pause response before closing its active WebSocket, so setup waits for the bounded response-drain window before daemon shutdown." -f `
                            $Reason,
                            $Bind,
                            $SynapseChromeBridgeMaintenanceCloseDrainMs,
                            $webSocketClose.had_socket,
                            $webSocketClose.close_requested,
                            ($(if ($null -eq $webSocketClose.close_deferred) { '<missing>' } else { $webSocketClose.close_deferred })))
                        Start-Sleep -Milliseconds $SynapseChromeBridgeMaintenanceCloseDrainMs
                    }
                }
            }
            $shutdown = Request-SynapseGracefulShutdown -Bind $Bind -Token $tokenRead.Token -ExpectedPids $expectedPids -Reason $Reason
            if (-not $shutdown.Ok) {
                $message = ("{0} reason={1} bind={2} error={3} response={4} remediation=the running daemon did not accept authenticated graceful shutdown; inspect daemon logs and token/bind state. Use -ForceRestart only for a coordinated legacy-runtime transition after manual input-state readback." -f `
                    $shutdown.Code,
                    $Reason,
                    $Bind,
                    $shutdown.Error,
                    ($(if ($null -eq $shutdown.Response) { '<none>' } else { $shutdown.Response | ConvertTo-Json -Compress -Depth 6 })))
                if ($ForceRestart) {
                    Info "FORCE_RESTART: $message"
                } else {
                    Die $message
                }
            } else {
                Info ("Synapse graceful shutdown requested reason={0} pid={1} active_sessions_before_shutdown={2}" -f `
                    $Reason,
                    $shutdown.Response.pid,
                    $shutdown.Response.active_sessions_before_shutdown)
            }
        }

        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        do {
            Start-Sleep -Milliseconds 250
            $remainingHttpPids = @($httpProcesses | Where-Object {
                $pidValue = [int]$_.ProcessId
                $current = Get-CimInstance Win32_Process -Filter "ProcessId=$pidValue" -ErrorAction SilentlyContinue
                $exeLeaf = if ($current -and $current.ExecutablePath) { Split-Path -Leaf $current.ExecutablePath } else { '' }
                $null -ne $current -and ($current.Name -ieq 'synapse-mcp.exe' -or $exeLeaf -ieq 'synapse-mcp.exe')
            })
            if ($remainingHttpPids.Count -eq 0) {
                Info "Synapse graceful shutdown verified reason=$Reason http_process_count=0"
                Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds $TimeoutSeconds -ForceRestart:$ForceRestart
                break
            }
        } while ((Get-Date) -lt $deadline)

        $remainingHttpPids = @($httpProcesses | Where-Object {
            $pidValue = [int]$_.ProcessId
            $current = Get-CimInstance Win32_Process -Filter "ProcessId=$pidValue" -ErrorAction SilentlyContinue
            $exeLeaf = if ($current -and $current.ExecutablePath) { Split-Path -Leaf $current.ExecutablePath } else { '' }
            $null -ne $current -and ($current.Name -ieq 'synapse-mcp.exe' -or $exeLeaf -ieq 'synapse-mcp.exe')
        })
        if ($remainingHttpPids.Count -gt 0) {
            $message = ("SYNAPSE_GRACEFUL_SHUTDOWN_TIMEOUT reason={0} timeout_s={1} remaining_count={2}`nremaining:`n{3}" -f `
                $Reason,
                $TimeoutSeconds,
                $remainingHttpPids.Count,
                (Format-SynapseMcpProcessSnapshot -Snapshot $remainingHttpPids))
            if ($ForceRestart) {
                Info "FORCE_RESTART: $message"
            } else {
                Die $message
            }
        }
    }

    $remaining = @(Select-SynapseMcpDeployTargetProcesses -Snapshot @(Get-SynapseMcpProcessSnapshot) -Bind $Bind -DbPath $DbPath)
    if ($remaining.Count -eq 0) {
        Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds $TimeoutSeconds -ForceRestart:$ForceRestart
        $ignoredAfter = @(Select-SynapseMcpDeployTargetProcesses -Snapshot @(Get-SynapseMcpProcessSnapshot) -Bind $Bind -DbPath $DbPath -Invert)
        Info "Synapse process stop verified reason=$Reason target_after_count=0 ignored_non_target_after_count=$($ignoredAfter.Count)"
        return
    }

    if (-not $ForceRestart) {
        Die ("SYNAPSE_PROCESS_STOP_INCOMPLETE reason={0} remaining_count={1} remaining=`n{2}" -f `
            $Reason,
            $remaining.Count,
            (Format-SynapseMcpProcessSnapshot -Snapshot $remaining))
    }

    Info ("FORCE_RESTART: exact-PID stop for remaining verified Synapse processes reason={0} remaining_count={1}`nremaining:`n{2}" -f `
        $Reason,
        $remaining.Count,
        (Format-SynapseMcpProcessSnapshot -Snapshot $remaining))
    foreach ($proc in $remaining) {
        $verified = Assert-SynapseProcessStopTarget -SnapshotProcess $proc -Bind $Bind -DbPath $DbPath
        if (-not $verified) { continue }
        $pidValue = [int]$verified.ProcessId
        try {
            Stop-Process -Id $pidValue -Force -ErrorAction Stop
            Info "Synapse process exact-PID force stop issued pid=$pidValue reason=$Reason match_rules=$($verified.DeployTargetRules) path=$($verified.ExecutablePath) cmd=$($verified.CommandLine)"
        } catch {
            Die ("SYNAPSE_PROCESS_STOP_FAILED pid={0} reason={1} error={2} remediation=setup only stops verified synapse-mcp.exe PIDs; inspect process ownership and retry after the daemon exits" -f `
                $pidValue,
                $Reason,
                $_.Exception.Message)
        }
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 250
        $after = @(Select-SynapseMcpDeployTargetProcesses -Snapshot @(Get-SynapseMcpProcessSnapshot) -Bind $Bind -DbPath $DbPath)
        if ($after.Count -eq 0) {
            Wait-SynapseBindReleased -Reason $Reason -Bind $Bind -TimeoutSeconds $TimeoutSeconds -ForceRestart:$ForceRestart
            $ignoredAfter = @(Select-SynapseMcpDeployTargetProcesses -Snapshot @(Get-SynapseMcpProcessSnapshot) -Bind $Bind -DbPath $DbPath -Invert)
            Info "Synapse process stop verified reason=$Reason target_after_count=0 ignored_non_target_after_count=$($ignoredAfter.Count)"
            return
        }
    } while ((Get-Date) -lt $deadline)

    $remaining = @(Select-SynapseMcpDeployTargetProcesses -Snapshot @(Get-SynapseMcpProcessSnapshot) -Bind $Bind -DbPath $DbPath)
    Die ("SYNAPSE_PROCESS_STOP_FAILED reason={0} timeout_s={1} remaining_count={2} remaining=`n{3}" -f `
        $Reason, $TimeoutSeconds, $remaining.Count, (Format-SynapseMcpProcessSnapshot -Snapshot $remaining))
}

function Get-SynapseChromeNativeHostProcessSnapshot {
    param([Parameter(Mandatory=$true)][string]$NativeHostExePath)

    $expectedPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($NativeHostExePath)
    @(Get-CimInstance Win32_Process -Filter "Name='synapse-chrome-native-host.exe'" -ErrorAction SilentlyContinue |
        Where-Object {
            $_.ExecutablePath -and
            ($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($_.ExecutablePath) -ieq $expectedPath)
        } |
        Select-Object ProcessId,ParentProcessId,Name,ExecutablePath,CommandLine)
}

function Format-SynapseChromeNativeHostProcessSnapshot {
    param($Snapshot)
    $rows = @($Snapshot | ForEach-Object {
        "pid=$($_.ProcessId) ppid=$($_.ParentProcessId) path=$($_.ExecutablePath) cmd=$($_.CommandLine)"
    })
    if ($rows.Count -eq 0) { return '<none>' }
    return ($rows -join "`n")
}

function Assert-SynapseChromeNativeHostStopTarget {
    param(
        [Parameter(Mandatory=$true)]$SnapshotProcess,
        [Parameter(Mandatory=$true)][string]$NativeHostExePath
    )

    $pidValue = [int]$SnapshotProcess.ProcessId
    $current = Get-CimInstance Win32_Process -Filter "ProcessId=$pidValue" -ErrorAction SilentlyContinue
    if (-not $current) {
        Info "Chrome native host stop target already exited pid=$pidValue"
        return $null
    }

    $expectedPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($NativeHostExePath)
    $actualPath = if ($current.ExecutablePath) { $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($current.ExecutablePath) } else { '' }
    if ($current.Name -ine 'synapse-chrome-native-host.exe' -or $actualPath -ine $expectedPath) {
        Die ("SYNAPSE_CHROME_NATIVE_HOST_STOP_TARGET_MISMATCH pid={0} expected_path={1} actual_name={2} actual_path={3} command_line={4} remediation=PID was reused or snapshot was not the Synapse Chrome native host; refusing exact-PID stop" -f `
            $pidValue,
            $expectedPath,
            $current.Name,
            $current.ExecutablePath,
            $current.CommandLine)
    }
    $legacyBrowserRow = Get-SynapseBrowserRow -Browser $SynapseLegacyBrowserId
    $legacyBridgeOrigin = "$($legacyBrowserRow.extension.origin_scheme)://$($legacyBrowserRow.extension.id)/"
    if ($current.CommandLine -notmatch [regex]::Escape($legacyBridgeOrigin)) {
        Die ("SYNAPSE_CHROME_NATIVE_HOST_STOP_UNVERIFIED pid={0} name={1} command_line={2} remediation=command line does not prove the Synapse Chrome extension bridge target; refusing exact-PID stop" -f `
            $pidValue,
            $current.Name,
            $current.CommandLine)
    }

    return $current
}

function Stop-SynapseChromeNativeHostProcesses {
    param(
        [Parameter(Mandatory=$true)][string]$Reason,
        [Parameter(Mandatory=$true)][string]$NativeHostExePath,
        [int]$TimeoutSeconds = 10
    )

    $before = @(Get-SynapseChromeNativeHostProcessSnapshot -NativeHostExePath $NativeHostExePath)
    Info "Chrome native host process stop requested reason=$Reason before_count=$($before.Count)"
    Info ("Chrome native host process stop before:`n{0}" -f (Format-SynapseChromeNativeHostProcessSnapshot -Snapshot $before))
    foreach ($proc in $before) {
        $verified = Assert-SynapseChromeNativeHostStopTarget -SnapshotProcess $proc -NativeHostExePath $NativeHostExePath
        if (-not $verified) { continue }
        $pidValue = [int]$verified.ProcessId
        try {
            Stop-Process -Id $pidValue -Force -ErrorAction Stop
            Info "Chrome native host exact-PID stop issued pid=$pidValue reason=$Reason"
        } catch {
            Die ("SYNAPSE_CHROME_NATIVE_HOST_STOP_FAILED pid={0} reason={1} error={2} remediation=setup only stops verified synapse-chrome-native-host.exe PIDs; it never stops cmd.exe/native-messaging wrapper or terminal processes" -f `
                $pidValue,
                $Reason,
                $_.Exception.Message)
        }
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 250
        $after = @(Get-SynapseChromeNativeHostProcessSnapshot -NativeHostExePath $NativeHostExePath)
        if ($after.Count -eq 0) {
            Info "Chrome native host process stop verified reason=$Reason after_count=0"
            return
        }
    } while ((Get-Date) -lt $deadline)

    $remaining = @(Get-SynapseChromeNativeHostProcessSnapshot -NativeHostExePath $NativeHostExePath)
    Die ("SYNAPSE_CHROME_NATIVE_HOST_STOP_FAILED reason={0} timeout_s={1} remaining_count={2} remaining=`n{3}" -f `
        $Reason, $TimeoutSeconds, $remaining.Count, (Format-SynapseChromeNativeHostProcessSnapshot -Snapshot $remaining))
}

# ---------------------------------------------------------------------------
# Purge scope (#181): -Remove -Purge deletes only explicit Synapse-owned paths.
# Nothing is ever derived from a caller-supplied path by taking its parent.
# ---------------------------------------------------------------------------
function Test-SynapsePathStrictlyInside {
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][string]$Root)
    $sep = [System.IO.Path]::DirectorySeparatorChar
    $full = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    if ([string]::IsNullOrWhiteSpace($rootFull)) { return $false }
    return $full.StartsWith($rootFull + $sep, [System.StringComparison]::OrdinalIgnoreCase)
}

function Get-SynapsePurgePlan {
    param(
        [Parameter(Mandatory)][string]$DbPath,
        [Parameter(Mandatory)][string]$ProfilesDir,
        [Parameter(Mandatory)][string]$TokenPath,
        [Parameter(Mandatory)][string]$AppDataSynapseDir,
        [Parameter(Mandatory)][string[]]$AllowedRoots,
        [string[]]$AllowedExactDirs = @()
    )
    $refuse = {
        param([string]$What, [string]$Path)
        throw "SYNAPSE_PURGE_PATH_REFUSED what=$What path=$Path remediation=point it inside a Synapse root ($($AllowedRoots -join '; ')) or omit -Purge"
    }
    # Named Synapse-owned entries under %APPDATA%\synapse; everything else there is foreign.
    $appDataFiles = @('token.txt', 'chrome-bridge-token.txt', 'chrome-bridge-deferred-repair.txt', 'codex-tool-surface.json')
    $appDataDirs = @('chrome-debugger')
    $targets = New-Object System.Collections.Generic.List[object]
    foreach ($entry in @(@('DbPath', $DbPath), @('ProfilesDir', $ProfilesDir))) {
        $full = [System.IO.Path]::GetFullPath($entry[1]).TrimEnd('\', '/')
        $allowed = $false
        foreach ($root in $AllowedRoots) { if (Test-SynapsePathStrictlyInside -Path $full -Root $root) { $allowed = $true } }
        foreach ($exact in $AllowedExactDirs) {
            if ($full -ieq [System.IO.Path]::GetFullPath($exact).TrimEnd('\', '/')) { $allowed = $true }
        }
        if (-not $allowed) { & $refuse $entry[0] $entry[1] }
        if (Test-Path -LiteralPath $full -PathType Leaf) { & $refuse "$($entry[0])_is_a_file" $entry[1] }
        if ((Test-Path -LiteralPath $full -PathType Container) -and
            ((Get-Item -LiteralPath $full -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
            & $refuse "$($entry[0])_is_a_reparse_point" $entry[1]
        }
        $targets.Add([pscustomobject]@{ Path = $full; Recurse = $true })
    }
    # The token is deleted as a single FILE; its parent directory is never touched.
    if (Test-Path -LiteralPath $TokenPath -PathType Container) { & $refuse 'TokenPath_is_a_directory' $TokenPath }
    $targets.Add([pscustomobject]@{ Path = [System.IO.Path]::GetFullPath($TokenPath); Recurse = $false })
    foreach ($name in $appDataFiles) {
        $targets.Add([pscustomobject]@{ Path = [System.IO.Path]::GetFullPath((Join-Path $AppDataSynapseDir $name)); Recurse = $false })
    }
    foreach ($name in $appDataDirs) {
        $targets.Add([pscustomobject]@{ Path = [System.IO.Path]::GetFullPath((Join-Path $AppDataSynapseDir $name)); Recurse = $true })
    }
    # Preflight the complete plan, including ancestors and named appdata entries,
    # before returning any deletion target. PowerShell 5.1 can recurse through junctions.
    foreach ($target in $targets) {
        $ancestorPath = $target.Path
        while ($ancestorPath) {
            if (Test-Path -LiteralPath $ancestorPath) {
                $ancestor = Get-Item -LiteralPath $ancestorPath -Force -ErrorAction Stop
                if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) {
                    & $refuse 'reparse_path' $ancestorPath
                }
            }
            $ancestorPath = Split-Path -Parent $ancestorPath
        }
        if (-not $target.Recurse -or -not (Test-Path -LiteralPath $target.Path -PathType Container)) { continue }
        $pending = New-Object System.Collections.Generic.Queue[string]
        $pending.Enqueue($target.Path)
        while ($pending.Count -gt 0) {
            $directory = $pending.Dequeue()
            foreach ($child in @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop)) {
                if ($child.Attributes -band [IO.FileAttributes]::ReparsePoint) { & $refuse 'reparse_descendant' $child.FullName }
                if ($child.PSIsContainer) { $pending.Enqueue($child.FullName) }
            }
        }
    }
    $seen = @{}
    foreach ($t in $targets) {
        $key = $t.Path.ToLowerInvariant()
        if (-not $seen.ContainsKey($key)) { $seen[$key] = $true; $t }
    }
}

function Invoke-SynapsePurge {
    [CmdletBinding(SupportsShouldProcess, ConfirmImpact = 'High')]
    param(
        [Parameter(Mandatory)][string]$DbPath,
        [Parameter(Mandatory)][string]$ProfilesDir,
        [Parameter(Mandatory)][string]$TokenPath,
        [Parameter(Mandatory)][string]$AppDataSynapseDir,
        [Parameter(Mandatory)][string[]]$AllowedRoots,
        [string[]]$AllowedExactDirs = @(),
        [switch]$Force
    )
    $plan = @(Get-SynapsePurgePlan -DbPath $DbPath -ProfilesDir $ProfilesDir -TokenPath $TokenPath `
            -AppDataSynapseDir $AppDataSynapseDir -AllowedRoots $AllowedRoots -AllowedExactDirs $AllowedExactDirs)
    $existing = @($plan | Where-Object { Test-Path -LiteralPath $_.Path })
    Info "Purge will delete exactly these $($existing.Count) existing Synapse-owned path(s):"
    foreach ($t in $existing) { Info "  $(if ($t.Recurse) { '[dir] ' } else { '[file]' }) $($t.Path)" }
    if ($existing.Count -eq 0) { return }
    if (-not ($Force -or $PSCmdlet.ShouldProcess(($existing.Path -join '; '), 'Delete Synapse data (-Purge)'))) {
        Info 'Purge skipped (not confirmed). Pass -Force to delete without a prompt.'
        return
    }
    foreach ($t in $existing) {
        if ($t.Recurse) { Remove-Item -LiteralPath $t.Path -Recurse -Force } else { Remove-Item -LiteralPath $t.Path -Force }
        Info "Deleted $($t.Path)"
    }
}

# ---------------------------------------------------------------------------
# Uninstall path
# ---------------------------------------------------------------------------
$maintenanceReason = if ($Remove) { 'remove' } elseif ($RefreshToolSurfaceOnly) { 'tool_surface_refresh' } else { 'setup' }
if (-not $Remove -and -not $RefreshToolSurfaceOnly) {
    # #15: pure input is validated before the maintenance lock, build, staging, or
    # candidate preflight so a typo cannot waste a build or leave prepared files.
    $null = Get-SynapseNormalizedIssueRef -Issue $ActiveIssue
}
if ($RefreshToolSurfaceOnly -and ($Remove -or $Purge -or $ForceRestart)) {
    Die 'SYNAPSE_TOOL_SURFACE_REFRESH_FLAGS_INVALID remediation=refresh cannot be combined with remove, purge, or force restart'
}
Wait-SynapsePostExitParent -ParentPid $PostExitParentPid -Reason $PostExitContinuationReason
Acquire-SynapseSetupMaintenanceLock -Path $MaintenanceLockPath -Reason $maintenanceReason
if (-not $Remove -and -not $RefreshToolSurfaceOnly) {
    $null = Remove-SynapseLegacySetupRunDirectories -LogDir $LogDir
}

if ($RefreshToolSurfaceOnly) {
    if (-not (Test-Path -LiteralPath $TokenPath -PathType Leaf)) {
        Die "SYNAPSE_TOOL_SURFACE_REFRESH_TOKEN_MISSING path=$TokenPath remediation=restore the existing daemon token before refreshing"
    }
    $refreshToken = (Get-Content -Raw -LiteralPath $TokenPath).Trim()
    try {
        $refreshHealth = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $refreshToken" } -TimeoutSec 5
    } catch {
        Die "SYNAPSE_TOOL_SURFACE_REFRESH_HEALTH_FAILED bind=$Bind error=$($_.Exception.Message) remediation=start or repair the authenticated daemon before refreshing"
    }
    $refreshSurface = Read-SynapseDaemonToolSurface -Bind $Bind -Token $refreshToken -Health $refreshHealth
    Assert-SynapseRefreshToolSurface -Surface $refreshSurface -Health $refreshHealth -Bind $Bind
    Write-SynapseCodexToolSurfaceSnapshot -Path $CodexToolSurfaceSnapshotPath -Surface $refreshSurface
    $writtenSurface = Get-Content -Raw -LiteralPath $CodexToolSurfaceSnapshotPath | ConvertFrom-Json
    if ($writtenSurface.tool_surface_setup_canonical_sha256 -cne $refreshSurface.tool_surface_setup_canonical_sha256 -or
        $writtenSurface.tool_surface_sha256 -cne $refreshSurface.tool_surface_sha256) {
        Die "SYNAPSE_TOOL_SURFACE_REFRESH_READBACK_MISMATCH path=$CodexToolSurfaceSnapshotPath remediation=repair snapshot storage before using it"
    }
    Info "Tool-surface refresh verified daemon_pid=$($refreshSurface.daemon_pid) tool_count=$($refreshSurface.tool_count) canonical_sha256=$($refreshSurface.tool_surface_setup_canonical_sha256)"
    Release-SynapseSetupMaintenanceLock -State released
    return
}

if ($Remove) {
    $SynapsePurgeArgs = @{
        DbPath = $DbPath; ProfilesDir = $ProfilesDir; TokenPath = $TokenPath
        AppDataSynapseDir = (Join-Path $env:APPDATA 'synapse')
        AllowedRoots = @((Join-Path $env:LOCALAPPDATA 'synapse'), (Join-Path $env:APPDATA 'synapse'))
        AllowedExactDirs = @((Join-Path $env:USERPROFILE '.cargo\bin\profiles'))
    }
    if ($Purge) {
        # Validate every purge path BEFORE stopping anything (#181).
        Get-SynapsePurgePlan @SynapsePurgeArgs | Out-Null
    }
    Step "Removing scheduled task '$TaskName'"
    Assert-SynapseRestartAllowed -Reason 'remove' -Bind $Bind -DbPath $DbPath -TokenPath $TokenPath -ForceRestart:$ForceRestart
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask  -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Info "Unregistered '$TaskName'."
    } else { Info "Task '$TaskName' not present." }
    Stop-SynapseMcpProcesses -Reason 'remove' -Bind $Bind -DbPath $DbPath -TokenPath $TokenPath -ForceRestart:$ForceRestart
    if ($Purge) {
        Invoke-SynapsePurge @SynapsePurgeArgs -Force:$Force
    }
    Info "Done (remove)."
    Release-SynapseSetupMaintenanceLock -State released
    return
}

# ---------------------------------------------------------------------------
# 1. Preflight
# ---------------------------------------------------------------------------
function Initialize-SynapseNativeBuildEnvironment {
    param([string]$LibclangPath = $env:LIBCLANG_PATH)

    $candidates = @()
    if (-not [string]::IsNullOrWhiteSpace($LibclangPath)) {
        $candidates += $LibclangPath
    } else {
        $vsRoot = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Microsoft Visual Studio\2022'
        if (Test-Path -LiteralPath $vsRoot -PathType Container) {
            $candidates += @(Get-ChildItem -LiteralPath $vsRoot -Directory -ErrorAction Stop |
                ForEach-Object { Join-Path $_.FullName 'VC\Tools\Llvm\x64\bin' })
        }
        $candidates += (Join-Path $env:ProgramFiles 'LLVM\bin')
        $clang = Get-Command clang.exe -ErrorAction SilentlyContinue
        if ($clang) { $candidates += (Split-Path -Parent $clang.Source) }
    }

    $dll = $null
    foreach ($candidate in $candidates) {
        $path = if ($candidate -match '(?i)\.dll$') { $candidate } else { Join-Path $candidate 'libclang.dll' }
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            $dll = (Resolve-Path -LiteralPath $path).Path
            break
        }
    }
    if (-not $dll) {
        Die "SYNAPSE_NATIVE_LIBCLANG_MISSING checked=$($candidates -join ',') remediation=install Visual Studio Build Tools with the C++ Clang tools for Windows, or set LIBCLANG_PATH to the directory containing libclang.dll; rerun setup"
    }

    $directory = Split-Path -Parent $dll
    $env:LIBCLANG_PATH = $directory
    if (-not @(($env:Path -split ';') | Where-Object {
        $_.TrimEnd('\') -ieq $directory.TrimEnd('\')
    }).Count) {
        $env:Path = "$directory;$env:Path"
    }
    try {
        if ('System.Runtime.InteropServices.NativeLibrary' -as [type]) {
            $handle = [System.Runtime.InteropServices.NativeLibrary]::Load($dll)
        } else {
            if (-not ('SynapseNativeBuild.Loader' -as [type])) {
                Add-Type -Namespace SynapseNativeBuild -Name Loader -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode, SetLastError = true)]
public static extern System.IntPtr LoadLibraryEx(string path, System.IntPtr file, uint flags);
'@ -ErrorAction Stop
            }
            # LOAD_WITH_ALTERED_SEARCH_PATH searches beside libclang for its dependencies.
            $handle = [SynapseNativeBuild.Loader]::LoadLibraryEx($dll, [IntPtr]::Zero, 8)
            if ($handle -eq [IntPtr]::Zero) {
                throw [System.ComponentModel.Win32Exception]::new([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())
            }
        }
        if ($handle -eq [IntPtr]::Zero) { throw 'native loader returned a null handle' }
    } catch {
        Die "SYNAPSE_NATIVE_LIBCLANG_LOAD_FAILED dll=$dll detail=$($_.Exception.Message) remediation=repair the Visual Studio C++/LLVM installation and its native DLL dependencies, then rerun setup"
    }
    Info "Native build prerequisite loaded: $dll (LIBCLANG_PATH and child PATH configured)"
    return $dll
}

Step "Preflight"
$cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
if (-not $SkipBuild) {
    if (-not (Test-Path $cargo)) {
        Die "cargo not found at $cargo. Install the Rust toolchain (https://rustup.rs) on Windows, then re-run. Synapse builds with the current stable toolchain."
    }
    if (-not $SourceDir) { Die "-SourceDir is required unless -SkipBuild is set." }
    if (-not (Test-Path (Join-Path $SourceDir 'Cargo.toml'))) {
        Die "-SourceDir '$SourceDir' has no Cargo.toml. Point it at a synapse source checkout on a LOCAL drive."
    }
    if ($SourceDir -match '^\\\\' -or $SourceDir -match '^[Zz]:\\home\\') {
        Die "-SourceDir '$SourceDir' looks like a UNC / WSL-mapped path. Build from a real local copy: building over \\wsl.localhost bakes transient drive paths into the binary."
    }
    $null = Initialize-SynapseNativeBuildEnvironment
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $cargoVersionLog = Join-Path $LogDir 'setup-cargo-version.log'
    $cargoVersionDiagnosticsPath = Join-Path $LogDir 'setup-cargo-version-diagnostics.json'
    if (Test-Path -LiteralPath $cargoVersionDiagnosticsPath) { Remove-Item -LiteralPath $cargoVersionDiagnosticsPath -Force }
    $cargoVersionDiagnostics = $null
    $cargoVersionExit = Invoke-SynapseProcessInKillOnCloseJob `
        -FilePath $cargo `
        -ArgumentList @('--version') `
        -WorkingDirectory $SourceDir `
        -TimeoutMinutes 1 `
        -LogPath $cargoVersionLog `
        -Diagnostics ([ref]$cargoVersionDiagnostics)
    $cargoVersionLogSignal = Get-SynapseBuildLogSignal -Path $cargoVersionLog
    if ($cargoVersionExit -ne 0) {
        $failureKind = Get-SynapseCargoVersionFailureKind -Diagnostics $cargoVersionDiagnostics
        $versionFailure = [ordered]@{
            schema = 'synapse_setup_cargo_version_failure/v1'
            code = $failureKind.code
            source_dir = $SourceDir
            cargo = $cargo
            version_log = $cargoVersionLog
            preflight_timeout_minutes = 1
            version_exit = $cargoVersionExit
            remediation = $failureKind.remediation
            invocation = $cargoVersionDiagnostics
            log_signal = $cargoVersionLogSignal
        }
        $versionFailure | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $cargoVersionDiagnosticsPath -Encoding UTF8
        $job = $cargoVersionDiagnostics.process_job
        $childPid = if ($job -and $job.child_pid) { $job.child_pid } else { '<unknown>' }
        $completionKind = if ($job -and $job.completion_kind) { $job.completion_kind } else { '<unknown>' }
        $waitKind = if ($job -and $job.wait_kind) { $job.wait_kind } else { '<unknown>' }
        $terminateJobOk = if ($job) { [string]$job.terminate_job_ok } else { '<unknown>' }
        $cleanupWaitKind = if ($job -and $job.cleanup_wait_kind) { $job.cleanup_wait_kind } else { '<unknown>' }
        $childAliveAfter = if ($cargoVersionDiagnostics.cleanup_result) { [string]$cargoVersionDiagnostics.cleanup_result.child_process_alive_after } else { '<unknown>' }
        Die ("{0} exit={1} child_pid={2} child_alive_after={3} completion={4} wait={5} timeout_minutes=1 terminate_job_ok={6} cleanup_wait={7} diagnostics={8} log={9} remediation={10}`nTail:`n{11}" -f `
            $failureKind.code,
            $cargoVersionExit,
            $childPid,
            $childAliveAfter,
            $completionKind,
            $waitKind,
            $terminateJobOk,
            $cleanupWaitKind,
            $cargoVersionDiagnosticsPath,
            $cargoVersionLog,
            $failureKind.remediation,
            $cargoVersionLogSignal.tail_80)
    }
    $cargoVersionText = if (Test-Path -LiteralPath $cargoVersionLog) {
        ((Get-Content -LiteralPath $cargoVersionLog -ErrorAction SilentlyContinue) -join "`n").Trim()
    } else {
        ''
    }
    if ([string]::IsNullOrWhiteSpace($cargoVersionText)) {
        $emptyVersionFailure = [ordered]@{
            schema = 'synapse_setup_cargo_version_failure/v1'
            code = 'SYNAPSE_CARGO_VERSION_EMPTY'
            source_dir = $SourceDir
            cargo = $cargo
            version_log = $cargoVersionLog
            preflight_timeout_minutes = 1
            version_exit = $cargoVersionExit
            remediation = 'cargo --version exited 0 but produced no version text; repair Rust toolchain stdout/stderr before setup continues'
            invocation = $cargoVersionDiagnostics
            log_signal = $cargoVersionLogSignal
        }
        $emptyVersionFailure | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $cargoVersionDiagnosticsPath -Encoding UTF8
        Die "SYNAPSE_CARGO_VERSION_EMPTY log=$cargoVersionLog remediation=cargo --version exited 0 but produced no version text; repair Rust toolchain stdout/stderr before setup continues"
    }
    Info "cargo: $cargoVersionText"
}

# ---------------------------------------------------------------------------
# 2. Build (local source -> persistent target) and verify the binary
# ---------------------------------------------------------------------------
if (-not $SkipBuild) {
    Step "Building synapse-mcp (release) from $SourceDir"
    $buildSourceFingerprint = Get-SynapseBuildSourceFingerprint -SourceDir $SourceDir
    if (-not $PSBoundParameters.ContainsKey('CargoTarget')) {
        # Key the persistent target by the source checkout so two checkouts
        # can never share (and poison) one fingerprint database. Cargo
        # freshness is mtime-based against the dep-info file list: a build
        # from checkout A marks its units fresh for checkout A's files, and
        # a later build from checkout B silently reuses crates whose sources
        # differ (observed live 2026-06-12: a synapse-core compiled from a
        # sibling clone shadowed new modules and broke the deploy build).
        $resolvedSource = (Resolve-Path $SourceDir).Path.TrimEnd('\')
        $CargoTarget = Get-SynapseCheckoutCargoTarget -Root $CargoTarget -SourceDir $SourceDir
        Info "Per-checkout build target: $CargoTarget (source: $resolvedSource)"
    }
    New-Item -ItemType Directory -Force -Path $CargoTarget, $LogDir | Out-Null
    $env:CARGO_TARGET_DIR = $CargoTarget
    if (-not $env:CARGO_BUILD_JOBS) {
        # Full-machine parallelism even when setup runs directly (not via
        # synapse-update.ps1). The env var outranks any `[build] jobs = N` cap
        # in a user/repo cargo config.toml that would otherwise silently
        # serialize the build; cargo forwards it to build scripts as NUM_JOBS,
        # which parallelizes the RocksDB C++ compile. RAM-guarded (~1.5 GB per
        # heavy rustc/cl.exe job) so low-memory machines don't swap.
        $logicalCpus = [Environment]::ProcessorCount
        $ramGb = [math]::Floor((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)
        $env:CARGO_BUILD_JOBS = [string][int][math]::Min($logicalCpus, [math]::Max(1, [math]::Floor($ramGb / 1.5)))
    }
    Info "Build parallelism: CARGO_BUILD_JOBS=$($env:CARGO_BUILD_JOBS) (logical CPUs: $([Environment]::ProcessorCount))"
    $buildLog = Join-Path $LogDir 'setup-build.log'
    $buildDiagnosticsPath = Join-Path $LogDir 'setup-build-diagnostics.json'
    if (Test-Path -LiteralPath $buildDiagnosticsPath) { Remove-Item -LiteralPath $buildDiagnosticsPath -Force }
    $built = Get-SynapseCargoReleaseArtifactPath -CargoTarget $CargoTarget
    Info "Build process tree is job-owned; log: $buildLog"
    $buildInvocationDiagnostics = $null
    $buildExit = Invoke-SynapseProcessInKillOnCloseJob `
        -FilePath $cargo `
        -ArgumentList @('build','--release','-p','synapse-mcp') `
        -WorkingDirectory $SourceDir `
        -TimeoutMinutes $BuildTimeoutMinutes `
        -LogPath $buildLog `
        -Diagnostics ([ref]$buildInvocationDiagnostics)
    if ($buildExit -ne 0) {
        $buildLogSignal = Get-SynapseBuildLogSignal -Path $buildLog
        $artifactReadback = Get-SynapseArtifactReadback -Path $built
        $failureKind = Get-SynapseReleaseBuildFailureKind `
            -Diagnostics $buildInvocationDiagnostics `
            -LogSignal $buildLogSignal `
            -ArtifactReadback $artifactReadback
        $buildDiagnostics = [ordered]@{
            schema = 'synapse_setup_release_build_failure/v1'
            code = $failureKind.code
            source_dir = $SourceDir
            cargo = $cargo
            cargo_target_dir = $CargoTarget
            expected_artifact = $built
            build_log = $buildLog
            build_timeout_minutes = $BuildTimeoutMinutes
            build_exit = $buildExit
            remediation = $failureKind.remediation
            invocation = $buildInvocationDiagnostics
            log_signal = $buildLogSignal
            artifact_readback = $artifactReadback
        }
        $buildDiagnostics | ConvertTo-Json -Depth 32 | Set-Content -LiteralPath $buildDiagnosticsPath -Encoding UTF8
        $job = $buildInvocationDiagnostics.process_job
        $childPid = if ($job -and $job.child_pid) { $job.child_pid } else { '<unknown>' }
        $completionKind = if ($job -and $job.completion_kind) { $job.completion_kind } else { '<unknown>' }
        $waitKind = if ($job -and $job.wait_kind) { $job.wait_kind } else { '<unknown>' }
        $terminateJobOk = if ($job) { [string]$job.terminate_job_ok } else { '<unknown>' }
        $cleanupWaitKind = if ($job -and $job.cleanup_wait_kind) { $job.cleanup_wait_kind } else { '<unknown>' }
        $compilerError = if ($buildLogSignal.has_compiler_error) { 'true' } else { 'false' }
        $childAliveAfter = if ($buildInvocationDiagnostics.cleanup_result) { [string]$buildInvocationDiagnostics.cleanup_result.child_process_alive_after } else { '<unknown>' }
        $afterBuildToolCount = @($buildInvocationDiagnostics.build_tool_processes_after).Count
        Die ("{0} exit={1} child_pid={2} child_alive_after={3} completion={4} wait={5} timeout_minutes={6} terminate_job_ok={7} cleanup_wait={8} compiler_error={9} live_build_tool_processes_after={10} artifact_exists={11} artifact_sha256={12} artifact_exclusive_open={13} diagnostics={14} log={15} remediation={16}`nTail:`n{17}" -f `
            $failureKind.code,
            $buildExit,
            $childPid,
            $childAliveAfter,
            $completionKind,
            $waitKind,
            $BuildTimeoutMinutes,
            $terminateJobOk,
            $cleanupWaitKind,
            $compilerError,
            $afterBuildToolCount,
            $artifactReadback.exists,
            ($(if ($artifactReadback.sha256) { $artifactReadback.sha256 } else { '<none>' })),
            $artifactReadback.exclusive_open,
            $buildDiagnosticsPath,
            $buildLog,
            $failureKind.remediation,
            $buildLogSignal.tail_80)
    }
    if (-not (Test-Path $built)) { Die "Build reported success but $built is missing." }
    Write-SynapseBuildSourceFingerprint -SourceDir $SourceDir -CandidatePath $built -ExpectedFingerprint $buildSourceFingerprint
    Info "Built: $built ($([math]::Round((Get-Item $built).Length/1MB,1)) MB)"
}

# ---------------------------------------------------------------------------
# 3. Token, data dirs, and profile source resolution
# ---------------------------------------------------------------------------
Step "Bearer token + data dirs"
$tokDir = Split-Path -Parent $TokenPath
New-Item -ItemType Directory -Force -Path $tokDir, $DbPath, $LogDir | Out-Null
if (-not (Test-Path $TokenPath)) {
    $bytes = New-Object byte[] 32
    [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    ($bytes | ForEach-Object { $_.ToString('x2') }) -join '' | Set-Content -Path $TokenPath -NoNewline -Encoding ascii
    Info "Generated token -> $TokenPath"
} else { Info "Reusing token -> $TokenPath" }
$tokenRaw = Get-Content -Raw $TokenPath
$token = if ($null -eq $tokenRaw) { '' } else { $tokenRaw.Trim() }
if ($token.Length -lt 16) { Die "Token at $TokenPath is too short ($($token.Length) chars); delete it and re-run to regenerate." }
[Environment]::SetEnvironmentVariable('SYNAPSE_BEARER_TOKEN', $token, 'User')
$env:SYNAPSE_BEARER_TOKEN = $token
Info "Set Windows User SYNAPSE_BEARER_TOKEN from $TokenPath for native HTTP MCP clients that require env-based bearer auth."
# #259: launcher-side limits for pruning Codex start snapshots other starts left behind.
[Environment]::SetEnvironmentVariable('SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS', [string]$CodexStartSnapshotRetentionHours, 'User')
$env:SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS = [string]$CodexStartSnapshotRetentionHours
[Environment]::SetEnvironmentVariable('SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES', [string]$CodexStartSnapshotGraceMinutes, 'User')
$env:SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES = [string]$CodexStartSnapshotGraceMinutes
Info "Set Windows User SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS=$CodexStartSnapshotRetentionHours and SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES=$CodexStartSnapshotGraceMinutes for Codex launcher start-snapshot pruning."
try {
    $signature = '[DllImport("user32.dll", SetLastError=true, CharSet=CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
    $type = Add-Type -MemberDefinition $signature -Name Win32SendMessageTimeout -Namespace SynapseEnv -PassThru -ErrorAction Stop
    $broadcastResult = [UIntPtr]::Zero
    $rawReturn = $type::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'Environment', 0x0002, 5000, [ref]$broadcastResult)
    if ($rawReturn -eq [IntPtr]::Zero) {
        Info "WARN: environment broadcast returned 0; future GUI clients may need restart before seeing SYNAPSE_BEARER_TOKEN."
    }
} catch {
    Info "WARN: environment broadcast failed: $($_.Exception.Message). Future GUI clients may need restart before seeing SYNAPSE_BEARER_TOKEN."
}

$srcProfiles = if ($SourceDir) { Join-Path $SourceDir 'crates\synapse-profiles\profiles' } else { $null }
$candidateProfilesDir = $null
if ($srcProfiles -and (Test-Path $srcProfiles)) {
    $candidateProfilesDir = $srcProfiles
    Info "Candidate profile source: $candidateProfilesDir"
} elseif (Test-Path $ProfilesDir) {
    $candidateProfilesDir = $ProfilesDir
    Info "Candidate profile source: existing deployed profiles at $candidateProfilesDir"
} else {
    Die "SYNAPSE_CANDIDATE_PROFILES_MISSING source=$srcProfiles deployed=$ProfilesDir remediation=provide bundled profiles from SourceDir or an existing deployed ProfilesDir before setup can touch the live daemon"
}
$candidateProfileCount = (Get-ChildItem $candidateProfilesDir -Filter *.toml -File).Count
if ($candidateProfileCount -lt 1) {
    Die "SYNAPSE_CANDIDATE_PROFILES_EMPTY path=$candidateProfilesDir remediation=profile-dependent tools need at least one .toml profile before setup can touch the live daemon"
}
Info "Candidate profiles verified path=$candidateProfilesDir count=$candidateProfileCount"

# ---------------------------------------------------------------------------
# 4. Stage and health-check the replacement before touching the live daemon
# ---------------------------------------------------------------------------
Step "Validating candidate daemon before handoff"
$sourceArgs = @{ ExePath = $ExePath; CargoTarget = $CargoTarget; CargoTargetExplicit = $PSBoundParameters.ContainsKey('CargoTarget')
    LogDir = $LogDir; SourceDir = $SourceDir; SkipBuild = $SkipBuild
    PostExitUseInstalledBinary = $PostExitUseInstalledBinary; PostExitInstalledSha256 = $PostExitInstalledSha256 }
if (-not $SkipBuild) { $sourceArgs.BuiltPath = $built }
$installSource = Resolve-SynapseInstallSource @sourceArgs
$installSourcePath = $installSource.Path
$installSourceHash = $installSource.Sha256
$CargoTarget = $installSource.CargoTarget
$headIdentity = $installSource.HeadIdentity
$candidatePreflight = Test-SynapseCandidateDaemon -CandidateExePath $installSourcePath -ProfilesDir $candidateProfilesDir -TokenPath $TokenPath -LogDir $LogDir
if ($candidatePreflight.Sha256 -ne $installSourceHash) {
    Die "SYNAPSE_CANDIDATE_HASH_MISMATCH expected_sha256=$installSourceHash actual_sha256=$($candidatePreflight.Sha256) path=$installSourcePath remediation=candidate preflight observed different bytes; refusing handoff"
}
if ($SkipBuild -and -not $PostExitUseInstalledBinary) {
    Assert-SynapseSkipBuildRevision -CandidateBuild $candidatePreflight.Build -HeadIdentity $headIdentity -CandidatePath $installSourcePath
    $skipSource = if ($SourceDir) { $SourceDir } else { Split-Path -Parent $PSScriptRoot }
    Assert-SynapseSkipBuildSourceFingerprint -SourceDir $skipSource -CandidatePath $installSourcePath
    Info "SkipBuild candidate revision verified build=$($candidatePreflight.Build) head=$($headIdentity.Revision)"
}
Info "Candidate daemon accepted for handoff sha256=$installSourceHash tool_count=$($candidatePreflight.ToolCount) tool_surface_sha256=$($candidatePreflight.ToolSurfaceSha256)"

$codexAncestorBeforeHandoff = Get-SynapseCurrentCodexAncestor
if ($codexAncestorBeforeHandoff -and $processTokenAtStart -ne $token) {
    Info ("WARN: SYNAPSE_CODEX_CURRENT_PROCESS_ENV_STALE_PRE_HANDOFF_NONFATAL codex_pid={0} token_at_process_start={1} token_file={2} remediation=setup will keep the replacement handoff path available; after handoff call real mcp__synapse.health from this same Codex session before assuming reconnect failed. The patched launcher has been updated for future clients, and direct HTTP/token probes remain diagnostics only." -f `
        $codexAncestorBeforeHandoff.ProcessId,
        ($(if ([string]::IsNullOrWhiteSpace($processTokenAtStart)) { 'missing' } else { 'mismatch' })),
        $TokenPath)
}
Assert-CodexCandidateHandoffPreservesCurrentProcess `
    -CodexAncestor $codexAncestorBeforeHandoff `
    -CandidateSurface $candidatePreflight.ToolSurface `
    -ProcessHashAtStart $processToolSurfaceHashAtStart `
    -ProcessSnapshotAtStart $processToolSurfaceSnapshotAtStart `
    -SourceDir $SourceDir `
    -Bind $Bind `
    -TokenPath $TokenPath `
    -ActiveIssue $ActiveIssue

$chromeBridgeInstaller = Join-Path $PSScriptRoot 'install-synapse-chrome-debugger.ps1'
if ($script:SynapsePostExitStartOnly) {
    Info "SYNAPSE_POST_EXIT_SKIP_CHROME_BRIDGE_PREFLIGHT reason=$PostExitContinuationReason bind=$Bind remediation=post-exit continuation must not reload or reconnect the Chrome bridge before the daemon bind is reusable; daemon /health after start remains the bridge Source-of-Truth readback."
} elseif ($SkipClientWiring) {
    Info "Skipped Chrome direct bridge preflight because -SkipClientWiring was set."
} else {
    Step "Preflighting Chrome direct localhost bridge before daemon handoff"
    $chromeBridgePreflight = Invoke-SynapseChromeBridgeVerifier `
        -InstallerPath $chromeBridgeInstaller `
        -NativeHostExePath $ChromeNativeHostExePath
    Info ("Chrome direct bridge verifier preflight completed transport={0} extension_id={1} native_host_registry_present={2} native_host_manifest_present={3} policy_cleanup={4} popup_shield={5} {6}" -f `
        $chromeBridgePreflight.daemon_bridge_transport, `
        $chromeBridgePreflight.extension_id, `
        $chromeBridgePreflight.native_host_registry_present, `
        $chromeBridgePreflight.native_host_manifest_present, `
        (($chromeBridgePreflight.chrome_policy_cleanup | ForEach-Object { "$($_.hive):$($_.reason)" }) -join ','), `
        (($chromeBridgePreflight.chrome_policy_popup_shield | ForEach-Object { "$($_.hive):$($_.reason)" }) -join ','), `
        (Format-SynapseChromeBridgeProfileInstallState -Readback $chromeBridgePreflight))
}

function Invoke-SynapseOptionalBrowserBridges {
    # Best-effort per declared non-legacy browser row: not installed -> skip
    # with readback; installed -> run its family installer; a failure is a WARN
    # with evidence, never a setup failure (the legacy row is the mandatory one).
    param([string]$BrowsersDir)

    $dispatcher = Join-Path $PSScriptRoot 'install-synapse-browser-bridge.ps1'
    $results = @()
    foreach ($row in @(Get-SynapseBrowserRows -BrowsersDir $BrowsersDir | Where-Object { [string]$_.id -ne $SynapseLegacyBrowserId })) {
        try {
            $resolved = & $dispatcher -Browser $row.id -BrowsersDir $BrowsersDir -ResolveOnly
            if ($resolved.status -eq 'not_installed') {
                Info "SYNAPSE_BROWSER_BRIDGE_SKIPPED browser=$($row.id) status=not_installed reason=$($resolved.reason) remediation=$($resolved.remediation)"
                $results += [pscustomobject]@{ browser = [string]$row.id; status = 'not_installed' }
                continue
            }
            # Chromium rows run before the new daemon starts, so their live
            # identity check is deferred to Assert-SynapseBridgeLiveIdentityAfterSetup (#251).
            $chromium = [string]$row.family -eq 'chromium'
            $installerParameters = if ($chromium) { @{ DeferLiveVerification = $true } } else { @{} }
            $readback = & $dispatcher -Browser $row.id -BrowsersDir $BrowsersDir -InstallerParameters $installerParameters
            $liveCheck = if (-not $chromium) { 'not_applicable' }
                elseif ($readback.ok -eq $true) { 'verified' }
                elseif ($readback.runtime_verification.deferred -eq $true) { 'deferred_until_daemon_start' }
                else { 'unverified' }
            $status = if ($liveCheck -eq 'unverified') { 'unverified' } else { 'installed' }
            $prefix = if ($status -eq 'unverified') { 'WARN: SYNAPSE_BROWSER_BRIDGE_INSTALLED_UNVERIFIED' } else { 'SYNAPSE_BROWSER_BRIDGE_INSTALLED' }
            Info "$prefix browser=$($row.id) executable=$($resolved.plan.executable) health_key=$($row.health_key) ok=$($readback.ok) reason=$($readback.reason) live_check=$liveCheck"
            $results += [pscustomobject]@{ browser = [string]$row.id; status = $status; reason = [string]$readback.reason; readback = $readback }
        } catch {
            Info "WARN: SYNAPSE_BROWSER_BRIDGE_OPTIONAL_INSTALL_FAILED browser=$($row.id) error=$($_.Exception.Message) remediation=run scripts\install-synapse-browser-bridge.ps1 -Browser $($row.id) from the interactive desktop; the mandatory $SynapseLegacyBrowserId bridge is unaffected"
            $results += [pscustomobject]@{ browser = [string]$row.id; status = 'failed'; error = $_.Exception.Message }
        }
    }
    return $results
}

if (-not $script:SynapsePostExitStartOnly -and -not $SkipClientWiring) {
    Step "Installing optional browser bridges (best-effort; not installed = skip)"
    $optionalBrowserBridges = @(Invoke-SynapseOptionalBrowserBridges)
    Info ("Optional browser bridges: {0}" -f $(if ($optionalBrowserBridges.Count -eq 0) { '<none declared>' } else { ($optionalBrowserBridges | ForEach-Object { "$($_.browser)=$($_.status)" }) -join ',' }))
}

# ---------------------------------------------------------------------------
# 5. Drain the running daemon, then install the proven binary
# ---------------------------------------------------------------------------
$backupPath = $null
$installDisposition = Get-SynapseInstallDisposition -CandidateHash $installSourceHash -ExePath $ExePath
$oldInstalledHash = $installDisposition.InstalledHash
$installRequired = $installDisposition.InstallRequired
if ($installRequired) {
    Step "Draining live daemon and installing verified binary -> $ExePath"
    Assert-SynapseRestartAllowed -Reason 'install_binary' -Bind $Bind -DbPath $DbPath -TokenPath $TokenPath -ForceRestart:$ForceRestart -AllowActiveClientDrain
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }
    Stop-SynapseMcpProcesses -Reason 'install_binary' -Bind $Bind -DbPath $DbPath -TokenPath $TokenPath -ForceRestart:$ForceRestart -TimeoutSeconds 300
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ExePath) | Out-Null
if (Test-Path -LiteralPath $ExePath) {
    $backupPath = "$ExePath.bak"
    Copy-SynapseFileWithSharingRetry -Source $ExePath -Destination $backupPath
    $backupHash = Get-SynapseFileSha256 -Path $backupPath
    if ($backupHash -ne $oldInstalledHash) {
        Die "SYNAPSE_BINARY_BACKUP_HASH_MISMATCH installed=$ExePath backup=$backupPath installed_hash=$oldInstalledHash backup_hash=$backupHash remediation=backup bytes changed during copy; refusing to install candidate"
    }
    Info "Backed up old binary -> $backupPath sha256=$backupHash"
}
if ([System.IO.Path]::GetFullPath($installSourcePath) -ine [System.IO.Path]::GetFullPath($ExePath)) {
    try {
        $null = Install-SynapseBinaryOverHeldImage -Source $installSourcePath -Destination $ExePath
    } catch {
        $installCopyError = $_.Exception.Message
        if (-not $backupPath -or -not (Test-Path -LiteralPath $backupPath) -or -not $oldInstalledHash) {
            Die "SYNAPSE_BINARY_INSTALL_COPY_FAILED path=$ExePath error=$installCopyError backup=<none> remediation=the candidate was not installed and no prior binary was available; resolve the file lock and rerun setup"
        }
        $restoredHash = Restore-SynapseBinaryBackupAfterInstallFailure `
            -BackupPath $backupPath `
            -ExePath $ExePath `
            -ExpectedHash $oldInstalledHash `
            -TaskName $TaskName `
            -InstallError $installCopyError
        $restoreHealth = $null
        $restoreDeadline = (Get-Date).AddSeconds(60)
        while ((Get-Date) -lt $restoreDeadline) {
            Start-Sleep -Seconds 1
            try {
                $candidateHealth = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $token" } -TimeoutSec 4
                if (Test-SynapseInstalledHealthIdentity -Health $candidateHealth -ExePath $ExePath -DbPath $DbPath) {
                    $restoreHealth = $candidateHealth
                    break
                }
            } catch { }
        }
        if ($restoreHealth) {
            Die "SYNAPSE_BINARY_INSTALL_COPY_FAILED_ROLLED_BACK path=$ExePath rollback_sha256=$restoredHash rollback_pid=$($restoreHealth.pid) install_error=$installCopyError remediation=old daemon is serving again; inspect the transient lock before retrying setup"
        }
        Die "SYNAPSE_BINARY_INSTALL_COPY_FAILED_ROLLBACK_UNHEALTHY path=$ExePath rollback_sha256=$restoredHash install_error=$installCopyError remediation=the old binary was restored but its scheduled daemon did not become healthy; inspect launcher logs"
    }
} else {
    Info "Candidate already resides at install path=$ExePath; copy skipped"
}
} else {
    Step "Verified binary already installed -> $ExePath"
    Info "SYNAPSE_BINARY_INSTALL_NOOP path=$ExePath sha256=$oldInstalledHash reason=installed bytes already match the preflight candidate; daemon drain, backup, and copy skipped"
}
if (-not (Test-Path -LiteralPath $ExePath)) {
    Die "SYNAPSE_INSTALL_BINARY_MISSING path=$ExePath remediation=setup could not find the installed daemon binary after the copy step"
}
$installedHash = Get-SynapseFileSha256 -Path $ExePath
if ($installedHash -ne $installSourceHash) {
    Die "SYNAPSE_INSTALLED_BINARY_HASH_MISMATCH path=$ExePath expected_sha256=$installSourceHash actual_sha256=$installedHash remediation=installed daemon bytes do not match the candidate that passed health preflight"
}
$ver = (& $ExePath --version) 2>&1
Info "Installed binary reports: $ver"
Info "Installed binary verified path=$ExePath sha256=$installedHash previous_sha256=$oldInstalledHash"
Remove-SynapseSetupOwnedDirectories

if ($script:SynapsePostExitStartOnly) {
    Info "SYNAPSE_POST_EXIT_SKIP_CHROME_BRIDGE_VERIFY reason=$PostExitContinuationReason bind=$Bind remediation=post-exit continuation avoids creating bridge peers while the dead-owner bind is still draining; daemon /health after start verifies the active Chrome bridge."
} elseif ($SkipClientWiring) {
    Info "Skipped Chrome direct bridge verification because -SkipClientWiring was set."
} else {
    Step "Verifying Chrome direct localhost bridge"
    $chromeBridgeReadback = Invoke-SynapseChromeBridgeVerifier `
        -InstallerPath $chromeBridgeInstaller `
        -NativeHostExePath $ChromeNativeHostExePath
    Info ("Chrome direct bridge verifier completed transport={0} extension_id={1} native_host_registry_present={2} native_host_manifest_present={3} policy_cleanup={4} popup_shield={5} {6}" -f `
        $chromeBridgeReadback.daemon_bridge_transport, `
        $chromeBridgeReadback.extension_id, `
        $chromeBridgeReadback.native_host_registry_present, `
        $chromeBridgeReadback.native_host_manifest_present, `
        (($chromeBridgeReadback.chrome_policy_cleanup | ForEach-Object { "$($_.hive):$($_.reason)" }) -join ','), `
        (($chromeBridgeReadback.chrome_policy_popup_shield | ForEach-Object { "$($_.hive):$($_.reason)" }) -join ','), `
        (Format-SynapseChromeBridgeProfileInstallState -Readback $chromeBridgeReadback))
}

# ---------------------------------------------------------------------------
# 6. Deploy bundled profiles next to the exe (executable-relative lookup) +
#    keep an explicit --profile-dir for belt-and-suspenders.
# ---------------------------------------------------------------------------
Step "Deploying bundled profiles -> $ProfilesDir"
if ($srcProfiles -and (Test-Path $srcProfiles)) {
    New-Item -ItemType Directory -Force -Path $ProfilesDir | Out-Null
    Copy-Item "$srcProfiles\*" $ProfilesDir -Recurse -Force
    $n = (Get-ChildItem $ProfilesDir -Filter *.toml -File).Count
    if ($n -lt 1) { Die "SYNAPSE_PROFILES_DEPLOYED_EMPTY path=$ProfilesDir source=$srcProfiles remediation=copied profiles but found 0 .toml files in the deployed profile directory" }
    Info "Deployed $n profiles."
} elseif (-not (Test-Path $ProfilesDir)) {
    Die "SYNAPSE_PROFILES_MISSING source=$srcProfiles deployed=$ProfilesDir remediation=profile-dependent tools need bundled or deployed profiles before daemon start"
} else { Info "Reusing existing profiles at $ProfilesDir." }

# 6b. Deploy the browser rows next to the exe (synapse_browsers
#     BrowserRegistry::bundled_dirs reads <exe dir>\browsers). Without them the
#     daemon still serves the legacy chrome row compiled into the binary.
$srcBrowserRows = if ($SourceDir) { Join-Path $SourceDir 'crates\synapse-browsers\browsers' } else { $null }
$deployedBrowserRowsDir = Join-Path (Split-Path -Parent $ExePath) 'browsers'
Step "Deploying browser rows -> $deployedBrowserRowsDir"
if ($srcBrowserRows -and (Test-Path -LiteralPath $srcBrowserRows -PathType Container)) {
    New-Item -ItemType Directory -Force -Path $deployedBrowserRowsDir | Out-Null
    Copy-Item -Path (Join-Path $srcBrowserRows '*.json') -Destination $deployedBrowserRowsDir -Force
    $deployedBrowserRows = @(Get-SynapseBrowserRows -BrowsersDir $deployedBrowserRowsDir)
    if (-not ($deployedBrowserRows | Where-Object { [string]$_.id -eq $SynapseLegacyBrowserId })) {
        Die "SYNAPSE_BROWSER_ROWS_LEGACY_MISSING dir=$deployedBrowserRowsDir legacy=$SynapseLegacyBrowserId remediation=the deployed browser rows must include the mandatory legacy row"
    }
    Info ("Deployed {0} browser rows: {1}" -f $deployedBrowserRows.Count, (($deployedBrowserRows | ForEach-Object { $_.id }) -join ','))
} else {
    Info "SYNAPSE_BROWSER_ROWS_SOURCE_ABSENT source=$srcBrowserRows deployed=$deployedBrowserRowsDir reason=no SourceDir browser rows; the daemon uses its compiled-in legacy row"
}

if ($script:SynapseBindPostExitContinuationRequired) {
    $continuation = Start-SynapsePostExitSetupContinuation `
        -Reason 'install_binary' `
        -Bind $Bind `
        -SourceDir $SourceDir `
        -ExePath $ExePath `
        -ChromeNativeHostExePath $ChromeNativeHostExePath `
        -CargoTarget $CargoTarget `
        -DbPath $DbPath `
        -ProfilesDir $ProfilesDir `
        -LogDir $LogDir `
        -TokenPath $TokenPath `
        -CodexToolSurfaceSnapshotPath $CodexToolSurfaceSnapshotPath `
        -TaskName $TaskName `
        -MaintenanceLockPath $MaintenanceLockPath `
        -InstalledBinarySha256 $installedHash `
        -ActiveIssue $ActiveIssue `
        -DeadOwnerDetail $script:SynapseBindPostExitContinuationDetail
    Die ("SYNAPSE_BIND_POST_EXIT_CONTINUATION_STARTED reason=install_binary bind={0} child_pid={1} manifest={2} stdout={3} stderr={4} remediation=the verified daemon bytes and profiles were installed, but Windows kept the dead-owner listener unavailable until this setup process exits. A hidden continuation has been launched and will wait for parent_pid={5}, reacquire the maintenance lock, start the daemon through the normal setup path, and write its own stdout/stderr/readbacks. Inspect the continuation manifest/logs and final process/socket SoT before accepting repair." -f `
        $Bind,
        $continuation.ChildPid,
        $continuation.ManifestPath,
        $continuation.StdoutPath,
        $continuation.StderrPath,
        $PID)
}

# ---------------------------------------------------------------------------
# 7. Register + start the auto-start HTTP daemon (interactive desktop session)
# ---------------------------------------------------------------------------
Step "Registering auto-start daemon task '$TaskName'"
Wait-SynapseBindReleased -Reason 'pre_start' -Bind $Bind -TimeoutSeconds 300
$legacyLauncher = Join-Path $LogDir 'synapse-daemon-launch.cmd'
$hiddenLauncher = Join-Path $LogDir 'synapse-daemon-launch-hidden.vbs'
$launcherLog = Join-Path $LogDir 'daemon-launcher.log'
if (Test-Path $legacyLauncher) {
    Remove-Item -LiteralPath $legacyLauncher -Force
}
$wscriptExe = Join-Path $env:SystemRoot 'System32\wscript.exe'
if (-not (Test-Path $wscriptExe)) {
    Die "SYNAPSE_HIDDEN_LAUNCHER_MISSING path=$wscriptExe remediation=repair Windows Script Host or run the daemon manually with a hidden process supervisor"
}
New-HiddenDaemonLauncher -OutputPath $hiddenLauncher -ExePath $ExePath -Bind $Bind -DbPath $DbPath -ProfilesDir $ProfilesDir -LogDir $LogDir -TokenPath $TokenPath

$action  = New-ScheduledTaskAction -Execute $wscriptExe -Argument "//B //Nologo `"$hiddenLauncher`"" -WorkingDirectory $LogDir
$trigger = New-ScheduledTaskTrigger -AtLogOn -User "$env:USERDOMAIN\$env:USERNAME"
$princ   = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Limited
$set     = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
            -StartWhenAvailable -MultipleInstances IgnoreNew -RestartCount 3 `
            -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero)
$set.Hidden = $true
if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
}
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Principal $princ `
    -Settings $set -Description "Synapse MCP HTTP daemon (loopback) - the single body controlling Windows + WSL programs." | Out-Null
Start-ScheduledTask -TaskName $TaskName
Info "Task registered and started."

# ---------------------------------------------------------------------------
# 8. Health verify (source of truth: the live daemon)
# ---------------------------------------------------------------------------
Step "Verifying daemon health (http://$Bind/health)"
$ok = $false
$healthPid = $null
$healthDeadline = (Get-Date).AddMinutes(10)
while ((Get-Date) -lt $healthDeadline) {
    Start-Sleep -Seconds 2
    try {
        $h = Invoke-RestMethod -Uri "http://$Bind/health" -Headers @{ Authorization = "Bearer $token" } -TimeoutSec 4
        if (Test-SynapseInstalledHealthIdentity -Health $h -ExePath $ExePath -DbPath $DbPath) {
            Info ("Daemon OK: pid={0} version={1} db={2}" -f $h.pid, $h.version, $DbPath)
            $healthPid = [int]$h.pid
            $ok = $true; break
        }
    } catch { }
}
if (-not $ok) {
    $failureListeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
    $failureProcesses = @(Get-SynapseMcpProcessSnapshot)
    $failureDetail = ("SYNAPSE_INSTALL_HEALTH_FAILED bind={0} candidate_sha256={1} installed_sha256={2} backup={3}`nlisteners:`n{4}`nprocesses:`n{5}`nremediation=inspect {6} and synapse.log.* under {7} for launch / STORAGE_* / bind errors" -f `
        $Bind,
        $installSourceHash,
        $installedHash,
        ($(if ($backupPath) { $backupPath } else { '<none>' })),
        (Format-SynapseTcpBindListenerSnapshot -Snapshot $failureListeners),
        (Format-SynapseMcpProcessSnapshot -Snapshot $failureProcesses),
        $launcherLog,
        $LogDir)

    if ($backupPath -and (Test-Path -LiteralPath $backupPath) -and $oldInstalledHash) {
        Info "WARN: $failureDetail"
        Info "Attempting rollback to previous daemon binary backup=$backupPath sha256=$oldInstalledHash"
        $rollback = Invoke-SynapseInstallHealthRollback `
            -BackupPath $backupPath -ExePath $ExePath -ExpectedHash $oldInstalledHash `
            -TaskName $TaskName -FailureDetail $failureDetail -Bind $Bind -DbPath $DbPath -TokenPath $TokenPath -Token $token
        $rollbackHash = $rollback.RollbackHash
        if ($rollback.Ok) {
            Die ("SYNAPSE_INSTALL_HEALTH_FAILED_ROLLED_BACK candidate_sha256={0} rollback_sha256={1} rollback_pid={2} original_failure=[{3}] remediation=old daemon is serving again; inspect candidate startup logs before retrying" -f `
                $installSourceHash,
                $rollbackHash,
                $rollback.Pid,
                $failureDetail)
        }

        $rollbackListeners = @(Get-SynapseTcpBindListenerSnapshot -Bind $Bind)
        $rollbackProcesses = @(Get-SynapseMcpProcessSnapshot)
        Die ("SYNAPSE_INSTALL_HEALTH_FAILED_ROLLBACK_FAILED candidate_sha256={0} rollback_sha256={1} original_failure=[{2}]`nrollback_listeners:`n{3}`nrollback_processes:`n{4}`nremediation=rollback binary was restored but daemon did not become healthy; inspect {5} and synapse.log.* under {6}" -f `
            $installSourceHash,
            $rollbackHash,
            $failureDetail,
            (Format-SynapseTcpBindListenerSnapshot -Snapshot $rollbackListeners),
            (Format-SynapseMcpProcessSnapshot -Snapshot $rollbackProcesses),
            $launcherLog,
            $LogDir)
    }

    Die $failureDetail
}
if ($PostExitUseInstalledBinary) {
    # The running daemon image is write-locked by the OS, so hashing it now
    # binds the bytes the continuation actually launched: a replacement
    # between the preflight hash equality and the process start is caught
    # here instead of being blessed by a health check that verifies
    # pid/path/db but never the executable bytes. Runs immediately after
    # health, before Chrome bridge / client wiring treat the daemon as
    # accepted.
    $postLaunchHash = Get-SynapseFileSha256 -Path $ExePath
    if ($postLaunchHash -ne $PostExitInstalledSha256) {
        Die "SYNAPSE_POST_EXIT_LAUNCHED_BINARY_REPLACED path=$ExePath expected_sha256=$PostExitInstalledSha256 actual_sha256=$postLaunchHash daemon_pid=$healthPid remediation=the daemon was started from bytes that differ from the parent-verified hash; stop the scheduled daemon and rerun full setup without -SkipBuild to reinstall a verified candidate"
    }
    Info "Post-exit launched binary re-verified path=$ExePath sha256=$postLaunchHash daemon_pid=$healthPid"
}
if ($SkipClientWiring) {
    Info "Skipped Chrome bridge post-start assertion because -SkipClientWiring was set."
} else {
    $h = Assert-SynapseChromeBridgeLiveAfterSetup `
        -Bind $Bind `
        -Token $token `
        -Health $h `
        -ChromeBridgeInstallerPath $chromeBridgeInstaller `
        -ChromeNativeHostExePath $ChromeNativeHostExePath
    if ($null -eq $chromeBridgeReadback) {
        # The post-exit continuation skipped the pre-start verifier; its
        # deployment readback supplies the expected bridge identity.
        Step "Reading Chrome direct localhost bridge deployment for live identity verification"
        $chromeBridgeReadback = Invoke-SynapseChromeBridgeVerifier `
            -InstallerPath $chromeBridgeInstaller `
            -NativeHostExePath $ChromeNativeHostExePath
    }
    Step "Verifying live bridge identity through an authenticated browser_debugger MCP session"
    Assert-SynapseBridgeLiveIdentityAfterSetup `
        -Bind $Bind `
        -Token $token `
        -Health $h `
        -ChromeReadback $chromeBridgeReadback `
        -OptionalBridges @($optionalBrowserBridges)
}
$healthPid = [int]$h.pid
$daemonLineage = Get-ProcessLineage -StartPid $healthPid
$cmdAncestor = $daemonLineage | Where-Object { $_.Name -ieq 'cmd.exe' } | Select-Object -First 1
if ($cmdAncestor) {
    $lineageText = ($daemonLineage | ForEach-Object { "{0}:{1}" -f $_.ProcessId, $_.Name }) -join ' <- '
    Die "SYNAPSE_DAEMON_CMD_ANCESTOR_FORBIDDEN pid=$healthPid cmd_pid=$($cmdAncestor.ProcessId) lineage=$lineageText remediation=rerun setup after removing legacy daemon launchers; daemon must not be launched through cmd.exe."
}

$toolSurface = Read-SynapseDaemonToolSurface -Bind $Bind -Token $token -Health $h
Write-SynapseCodexToolSurfaceSnapshot -Path $CodexToolSurfaceSnapshotPath -Surface $toolSurface

# ---------------------------------------------------------------------------
# 9. Wire the Windows-side MCP clients
# ---------------------------------------------------------------------------
if (-not $SkipClientWiring) {
    Step "Wiring Windows-side MCP clients"

    # Claude Code (Windows) speaks Streamable HTTP natively -> point at the daemon.
    $claude = Get-Command claude -ErrorAction SilentlyContinue
    if ($claude) {
        try {
            & $claude.Source mcp remove synapse -s user 2>$null | Out-Null
            & $claude.Source mcp add --scope user --transport http synapse "http://$Bind/mcp" --header "Authorization: Bearer $token"
            Info "Claude Code (Windows) wired via HTTP transport."
        } catch { Info "WARN: 'claude mcp add' failed: $($_.Exception.Message). Wire it manually (transport http -> http://$Bind/mcp)." }
    } else { Info "claude CLI not found on Windows PATH; skipping Claude Code wiring." }

    # Codex speaks Streamable HTTP; Claude Desktop remains stdio-only -> connect bridge.
    $bridgeArgs = @('--mode','connect','--bind',$Bind)

    $codex = Get-Command codex -ErrorAction SilentlyContinue
    $codexCfg = "$env:USERPROFILE\.codex\config.toml"
    if ($codex) {
        if (Test-CodexSynapseHttpTransportConfig -ConfigPath $codexCfg -Bind $Bind) {
            Info "Codex MCP entry already uses the required Streamable HTTP transport."
        } else {
            & $codex.Source mcp remove synapse 2>$null | Out-Null
            & $codex.Source mcp add synapse --url "http://$Bind/mcp" --bearer-token-env-var SYNAPSE_BEARER_TOKEN
            $codexAddExit = $LASTEXITCODE
            if ($codexAddExit -ne 0 -and -not (Test-CodexSynapseHttpTransportConfig -ConfigPath $codexCfg -Bind $Bind)) {
                Die "codex mcp add failed (exit $codexAddExit). Codex must be wired to HTTP, not the connect bridge."
            }
            if (-not (Test-CodexSynapseHttpTransportConfig -ConfigPath $codexCfg -Bind $Bind)) {
                Die "codex mcp add completed but Codex config is not the required HTTP transport."
            }
            if ($codexAddExit -ne 0) {
                Info "WARN: codex mcp add exited $codexAddExit but Codex config now contains the required HTTP entry; continuing."
            }
        }
        Set-CodexSynapseClientPolicy -ConfigPath $codexCfg -Bind $Bind
        if (-not (Test-CodexSynapseHttpConfig -ConfigPath $codexCfg -Bind $Bind)) {
            Die "SYNAPSE_CODEX_MCP_CONFIG_INCOMPLETE path=$codexCfg remediation=repair [mcp_servers.synapse] so it contains url=http://$Bind/mcp, bearer_token_env_var=SYNAPSE_BEARER_TOKEN, required=false, and default_tools_approval_mode=approve."
        }
        Install-CodexSynapseTokenLoader -CodexCommandPath $codex.Source -TokenPath $TokenPath
        Info "Codex (Windows) wired via Streamable HTTP transport with required=false, startup_timeout_sec=20 and default_tools_approval_mode=approve; a Synapse outage stays visible without blocking Codex startup."
    } elseif (Test-Path $codexCfg) {
        $c = Get-Content -Raw $codexCfg
        if ($c -match '(?m)^\[mcp_servers\.synapse\]' -and
            -not (Test-CodexSynapseHttpConfig -ConfigPath $codexCfg -Bind $Bind)) {
            Die "Codex config exists at $codexCfg but codex CLI is not on PATH and the synapse entry is not the required HTTP transport/client policy. Install/repair Codex CLI, then re-run."
        }
        Info "Codex CLI not found; existing Codex config is already HTTP or has no synapse entry."
    } else { Info "codex CLI/config not found; skipping Codex wiring." }

    $desktopCfg = "$env:APPDATA\Claude\claude_desktop_config.json"
    if (Test-Path $desktopCfg) {
        try {
            $j = Get-Content -Raw $desktopCfg | ConvertFrom-Json
            if (-not $j.mcpServers) { $j | Add-Member -NotePropertyName mcpServers -NotePropertyValue (@{}) -Force }
            $desktopEntry = @{ command = $ExePath; args = $bridgeArgs; env = @{ SYNAPSE_MCP_DISABLE_OPERATOR_HOTKEY = '1' } }
            # $j.mcpServers is a hashtable when freshly created above, but a PSCustomObject when
            # parsed from an existing config. Dot-assigning a NEW property to a PSCustomObject throws
            # "The property 'synapse' cannot be found on this object" under Windows PowerShell 5.1,
            # so branch on type: index-assign dictionaries, Add-Member -Force PSCustomObjects (the
            # latter both adds-or-overwrites and works on PS 5.1 and 7+).
            if ($j.mcpServers -is [System.Collections.IDictionary]) {
                $j.mcpServers['synapse'] = $desktopEntry
            } else {
                $j.mcpServers | Add-Member -NotePropertyName synapse -NotePropertyValue $desktopEntry -Force
            }
            ($j | ConvertTo-Json -Depth 12) | Set-Content $desktopCfg -Encoding utf8
            Info "Claude Desktop wired -> connect bridge."
        } catch { Info "WARN: could not update $desktopCfg : $($_.Exception.Message)" }
    } else { Info "No Claude Desktop config at $desktopCfg; skipping." }
}

if (-not $SkipClientWiring) {
    $codexAncestor = Get-SynapseCurrentCodexAncestor
    if ($codexAncestor -and $processTokenAtStart -ne $token) {
        Info ("WARN: SYNAPSE_CODEX_CURRENT_PROCESS_ENV_STALE_NONFATAL codex_pid={0} token_at_process_start={1} token_file={2} remediation=do not assume the current Codex process is disconnected; first call real mcp__synapse.health from this same session and verify daemon PID/tool_surface readback. The patched launcher has been updated for future clients; if this already-running process has no authenticated MCP connection, use the existing live daemon and token file as diagnostics until the client can refresh its environment." -f $codexAncestor.ProcessId, ($(if ([string]::IsNullOrWhiteSpace($processTokenAtStart)) { 'missing' } else { 'mismatch' })), $TokenPath)
    }
    Assert-CodexCurrentProcessToolSurfaceFresh `
        -CodexAncestor $codexAncestor `
        -CurrentSurface $toolSurface `
        -ProcessHashAtStart $processToolSurfaceHashAtStart `
        -ProcessSnapshotAtStart $processToolSurfaceSnapshotAtStart `
        -SnapshotPath $CodexToolSurfaceSnapshotPath `
        -SourceDir $SourceDir `
        -Bind $Bind `
        -TokenPath $TokenPath `
        -ActiveIssue $ActiveIssue
} else {
    $codexAncestor = Get-SynapseCurrentCodexAncestor
    if ($codexAncestor) {
        Assert-CodexCurrentProcessToolSurfaceFresh `
            -CodexAncestor $codexAncestor `
            -CurrentSurface $toolSurface `
            -ProcessHashAtStart $processToolSurfaceHashAtStart `
            -ProcessSnapshotAtStart $processToolSurfaceSnapshotAtStart `
            -SnapshotPath $CodexToolSurfaceSnapshotPath `
            -SourceDir $SourceDir `
            -Bind $Bind `
            -TokenPath $TokenPath `
            -ActiveIssue $ActiveIssue `
            -NonFatal
        Info "Skipped client wiring because -SkipClientWiring was set; current-process freshness check still wrote any required current-daemon handoff in nonfatal mode."
    } else {
        Info "Skipped client wiring because -SkipClientWiring was set; no current Codex ancestor was found for a freshness handoff."
    }
}

if ($script:SynapsePostExitStartOnly) {
    $completionReadback = [ordered]@{
        daemon_pid = $healthPid
        bind = $Bind
        db_path = $DbPath
        daemon_run_current_path = (Join-Path $DbPath 'daemon-run-current.json')
        installed_binary_path = $ExePath
        installed_binary_sha256 = $installedHash
        codex_tool_surface_snapshot_path = $CodexToolSurfaceSnapshotPath
        tool_count = $toolSurface.tool_count
        tool_surface_sha256 = $toolSurface.tool_surface_sha256
        chrome_bridge_status = $h.subsystems.chrome_bridge.status
        chrome_bridge_detail = $h.subsystems.chrome_bridge.detail
    }
    Write-SynapsePostExitManifestState `
        -State 'completed' `
        -Message 'post-exit setup continuation completed after daemon, Chrome bridge, tool-surface, and client-config readbacks passed' `
        -ExitCode 0 `
        -Readback $completionReadback
    Info "SYNAPSE_POST_EXIT_CONTINUATION_COMPLETED manifest=$PostExitManifestPath daemon_pid=$healthPid tool_count=$($toolSurface.tool_count) tool_surface_sha256=$($toolSurface.tool_surface_sha256)"
}

$setupRepairCompletionReadback = [ordered]@{
    daemon_pid = $healthPid
    bind = $Bind
    db_path = $DbPath
    daemon_run_current_path = (Join-Path $DbPath 'daemon-run-current.json')
    installed_binary_path = $ExePath
    installed_binary_sha256 = $installedHash
    codex_tool_surface_snapshot_path = $CodexToolSurfaceSnapshotPath
    tool_count = $toolSurface.tool_count
    tool_surface_sha256 = $toolSurface.tool_surface_sha256
    chrome_bridge_status = $h.subsystems.chrome_bridge.status
    chrome_bridge_detail = $h.subsystems.chrome_bridge.detail
}
Write-SynapseSetupRepairManifestState `
    -State 'completed' `
    -Message 'setup repair completed after daemon, Chrome bridge, tool-surface, and client-config readbacks passed' `
    -ExitCode 0 `
    -Readback $setupRepairCompletionReadback

Step "Done"
Info "Synapse daemon is live on http://$Bind (MCP: http://$Bind/mcp)."
Info "Token: $TokenPath   DB: $DbPath   Profiles: $ProfilesDir"
Info "WSL clients: run scripts/synapse-install.sh from WSL to wire Claude Code + Codex there."
Release-SynapseSetupMaintenanceLock -State released
