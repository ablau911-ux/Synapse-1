# #379 (upstream 2040) frozen behaviour table: after a setup build job ends,
# setup proves zero job-owned processes remain, fails closed when any survive
# or the job cannot be read back, and never counts another agent's cargo/rustc
# as its own. Manager-owned, work method v3: Codex and Opus never edit this
# file. One case per L row.
# Run: pwsh -NoProfile -File scripts/tests/setup-process-job-cleanup-379_table.ps1
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'table-harness.ps1')
Import-SynapseScriptFunction -Path (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1') -Name 'Resolve-SynapseProcessJobCleanupVerdict'

function Test-JobCleanupRow {
    param($Arguments, $Expected)
    $verdict = Resolve-SynapseProcessJobCleanupVerdict @Arguments
    Join-SynapseTableProblems @(
        (Compare-SynapseTableValue 'JobClean' $Expected.JobClean $verdict.JobClean),
        (Compare-SynapseTableValue 'JobLeakedPids' $Expected.JobLeakedPids @($verdict.JobLeakedPids)),
        (Compare-SynapseTableValue 'UnrelatedBuildToolPids' $Expected.UnrelatedBuildToolPids @($verdict.UnrelatedBuildToolPids)),
        (Compare-SynapseTableValue 'FailClosed' $Expected.FailClosed $verdict.FailClosed),
        (Compare-SynapseTableValue 'Code' $Expected.Code $verdict.Code)
    )
}

$rows = @(
    @{ Name = '379a_neighbour_job_empty_no_other_builds_is_clean'; Check = {
        Test-JobCleanupRow @{ JobQueryOk = $true; JobActivePidsAfter = @(); BuildToolPidsAfter = @() } `
            @{ JobClean = $true; JobLeakedPids = @(); UnrelatedBuildToolPids = @(); FailClosed = $false; Code = '' }
    } },
    @{ Name = '379b_job_still_holds_cmd_and_cargo_is_leaked_fail_closed'; Check = {
        Test-JobCleanupRow @{ JobQueryOk = $true; JobActivePidsAfter = @(2544, 21608); BuildToolPidsAfter = @(21608) } `
            @{ JobClean = $false; JobLeakedPids = @(2544, 21608); UnrelatedBuildToolPids = @(); FailClosed = $true; Code = 'SYNAPSE_SETUP_PROCESS_JOB_LEAKED' }
    } },
    @{ Name = '379c_second_actor_other_agent_cargo_is_unrelated_never_leaked'; Check = {
        Test-JobCleanupRow @{ JobQueryOk = $true; JobActivePidsAfter = @(); BuildToolPidsAfter = @(7002, 7001) } `
            @{ JobClean = $true; JobLeakedPids = @(); UnrelatedBuildToolPids = @(7001, 7002); FailClosed = $false; Code = '' }
    } },
    @{ Name = '379d_mixed_job_survivor_and_other_agent_cargo'; Check = {
        Test-JobCleanupRow @{ JobQueryOk = $true; JobActivePidsAfter = @(100); BuildToolPidsAfter = @(100, 7001) } `
            @{ JobClean = $false; JobLeakedPids = @(100); UnrelatedBuildToolPids = @(7001); FailClosed = $true; Code = 'SYNAPSE_SETUP_PROCESS_JOB_LEAKED' }
    } },
    @{ Name = '379e_job_readback_failed_fails_closed'; Check = {
        Test-JobCleanupRow @{ JobQueryOk = $false; JobActivePidsAfter = @(); BuildToolPidsAfter = @(7001) } `
            @{ JobClean = $false; JobLeakedPids = @(); UnrelatedBuildToolPids = @(7001); FailClosed = $true; Code = 'SYNAPSE_SETUP_PROCESS_JOB_READBACK_FAILED' }
    } },
    @{ Name = '379f_hostile_duplicate_job_pids_reported_once_sorted'; Check = {
        Test-JobCleanupRow @{ JobQueryOk = $true; JobActivePidsAfter = @(300, 100, 300); BuildToolPidsAfter = @() } `
            @{ JobClean = $false; JobLeakedPids = @(100, 300); UnrelatedBuildToolPids = @(); FailClosed = $true; Code = 'SYNAPSE_SETUP_PROCESS_JOB_LEAKED' }
    } }
)

Invoke-SynapseTableRows -Table '#379 setup process job cleanup' -Rows $rows
