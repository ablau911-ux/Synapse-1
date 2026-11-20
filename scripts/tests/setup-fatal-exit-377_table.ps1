# #377 (upstream 1932) frozen behaviour table: a setup FATAL always ends the
# process with a non-zero exit code and records the failure. Manager-owned,
# work method v3: Codex and Opus never edit this file. One case per L row.
# Run: pwsh -NoProfile -File scripts/tests/setup-fatal-exit-377_table.ps1
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'table-harness.ps1')
Import-SynapseScriptFunction -Path (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1') -Name 'Resolve-SynapseSetupFatalTrapPlan'

function Test-FatalPlanRow {
    param($Arguments, $Expected)
    $plan = Resolve-SynapseSetupFatalTrapPlan @Arguments
    Join-SynapseTableProblems @(
        (Compare-SynapseTableValue 'ExitCode' $Expected.ExitCode $plan.ExitCode),
        (Compare-SynapseTableValue 'WriteRepairFailed' $Expected.WriteRepairFailed $plan.WriteRepairFailed),
        (Compare-SynapseTableValue 'WritePostExitFailed' $Expected.WritePostExitFailed $plan.WritePostExitFailed),
        (Compare-SynapseTableValue 'LockState' $Expected.LockState $plan.LockState)
    )
}

$rows = @(
    @{ Name = '377a_fatal_after_native_exit_0_exits_1'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = ''; PostExitStartOnly = $false; PostExitManifestState = ''; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $false; LockState = 'failed' }
    } },
    @{ Name = '377b_boundary_fatal_after_native_exit_5_still_exits_1'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = ''; PostExitStartOnly = $false; PostExitManifestState = ''; LastExitCode = 5 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $false; LockState = 'failed' }
    } },
    @{ Name = '377c_post_exit_raw_throw_records_failed_and_exits_1'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = ''; PostExitStartOnly = $true; PostExitManifestState = 'started'; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $true; LockState = 'failed' }
    } },
    @{ Name = '377d_post_exit_die_already_recorded_failed_is_not_rewritten'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = ''; PostExitStartOnly = $true; PostExitManifestState = 'failed'; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $false; LockState = 'failed' }
    } },
    @{ Name = '377e_post_exit_completed_then_throw_records_failed'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = ''; PostExitStartOnly = $true; PostExitManifestState = 'completed'; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $true; LockState = 'failed' }
    } },
    @{ Name = '377f_post_exit_manifest_unreadable_empty_state_records_failed'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = ''; PostExitStartOnly = $true; PostExitManifestState = ''; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $true; LockState = 'failed' }
    } },
    @{ Name = '377g_neighbour_repair_handoff_started_is_preserved_and_exits_1'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = 'handoff_started'; PostExitStartOnly = $false; PostExitManifestState = ''; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $false; WritePostExitFailed = $false; LockState = 'failed' }
    } },
    @{ Name = '377h_hostile_unknown_repair_state_is_overwritten_failed'; Check = {
        Test-FatalPlanRow @{ RepairManifestState = 'HANDOFF_STARTED '; PostExitStartOnly = $false; PostExitManifestState = ''; LastExitCode = 0 } `
            @{ ExitCode = 1; WriteRepairFailed = $true; WritePostExitFailed = $false; LockState = 'failed' }
    } }
)

Invoke-SynapseTableRows -Table '#377 setup fatal exit' -Rows $rows
