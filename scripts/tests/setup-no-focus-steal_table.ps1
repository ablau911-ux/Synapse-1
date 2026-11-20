# Setup never steals focus (upstream 4efe2baf, #2271/#2272) frozen behaviour
# table. (1) While setup's own bridge maintenance pause is active, setup keeps
# waiting for the worker instead of jumping to foreground chrome://extensions
# UI repair. (2) Foreground UI repair runs only when the human notification
# state says the human is not in a fullscreen/busy/presentation/locked state;
# an unreadable or unknown state refuses (fail closed).
# Manager-owned, work method v3: Codex and Opus never edit this file.
# Run: pwsh -NoProfile -File scripts/tests/setup-no-focus-steal_table.ps1
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'table-harness.ps1')
$scripts = Split-Path -Parent $PSScriptRoot
Import-SynapseScriptFunction -Path (Join-Path $scripts 'synapse-setup.ps1') -Name 'Get-SynapseChromeBridgeHostWaitStep'
Import-SynapseScriptFunction -Path (Join-Path $scripts 'install-synapse-chrome-debugger.ps1') -Name 'Resolve-SynapseUiRepairHumanStateGate'

$absent = 'status=degraded no_active_chrome_bridge_host extension_stale=unknown'
$present = 'status=ok extension_stale=false extension_capabilities=pageScreenshot'

function Test-WaitStepRow {
    param([string]$Detail, [int64]$PauseRemainingMs, [string]$Expected)
    Compare-SynapseTableValue 'step' $Expected (Get-SynapseChromeBridgeHostWaitStep -Detail $Detail -PauseRemainingMs $PauseRemainingMs)
}

function Test-GateRow {
    param([int]$HResult, [int]$State, [bool]$Allowed, [string]$Code)
    $gate = Resolve-SynapseUiRepairHumanStateGate -HResult $HResult -State $State
    Join-SynapseTableProblems @(
        (Compare-SynapseTableValue 'Allowed' $Allowed $gate.Allowed),
        (Compare-SynapseTableValue 'Code' $Code $gate.Code)
    )
}

$fullscreen = 'SYNAPSE_CHROME_BRIDGE_UI_REPAIR_REFUSED_HUMAN_FULLSCREEN'
$notPresent = 'SYNAPSE_CHROME_BRIDGE_UI_REPAIR_REFUSED_HUMAN_STATE'
$unreadable = 'SYNAPSE_CHROME_BRIDGE_UI_REPAIR_HUMAN_STATE_UNREADABLE'

$rows = @(
    # (1) wait step: 'wait' | 'stop_waiting' | 'ui_repair_now'
    @{ Name = 'F1a_host_absent_own_pause_active_keeps_waiting'; Check = { Test-WaitStepRow $absent 40000 'wait' } },
    @{ Name = 'F1b_boundary_host_absent_pause_1ms_left_keeps_waiting'; Check = { Test-WaitStepRow $absent 1 'wait' } },
    @{ Name = 'F1c_neighbour_host_absent_no_pause_waits'; Check = { Test-WaitStepRow $absent 0 'wait' } },
    @{ Name = 'F1d_neighbour_host_present_stops_waiting'; Check = { Test-WaitStepRow $present 40000 'stop_waiting' } },
    @{ Name = 'F1e_empty_detail_stops_waiting'; Check = { Test-WaitStepRow '' 0 'stop_waiting' } },
    # (2) human notification state gate (SHQueryUserNotificationState)
    @{ Name = 'F2a_neighbour_accepts_notifications_allows'; Check = { Test-GateRow 0 5 $true '' } },
    @{ Name = 'F2b_neighbour_quiet_time_allows'; Check = { Test-GateRow 0 6 $true '' } },
    @{ Name = 'F2c_busy_fullscreen_app_refuses'; Check = { Test-GateRow 0 2 $false $fullscreen } },
    @{ Name = 'F2d_d3d_fullscreen_game_refuses'; Check = { Test-GateRow 0 3 $false $fullscreen } },
    @{ Name = 'F2e_presentation_mode_refuses'; Check = { Test-GateRow 0 4 $false $fullscreen } },
    @{ Name = 'F2f_store_app_fullscreen_refuses'; Check = { Test-GateRow 0 7 $false $fullscreen } },
    @{ Name = 'F2g_locked_or_screensaver_refuses'; Check = { Test-GateRow 0 1 $false $notPresent } },
    @{ Name = 'F2h_hostile_unknown_state_99_refuses'; Check = { Test-GateRow 0 99 $false $notPresent } },
    @{ Name = 'F2i_hostile_zero_state_refuses'; Check = { Test-GateRow 0 0 $false $notPresent } },
    @{ Name = 'F2j_query_failed_refuses_even_if_state_looks_free'; Check = { Test-GateRow (-2147467259) 5 $false $unreadable } }
)

Invoke-SynapseTableRows -Table 'setup no focus steal' -Rows $rows
