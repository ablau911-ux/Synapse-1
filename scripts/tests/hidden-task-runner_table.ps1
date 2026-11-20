# Hidden scheduled tasks (upstream b3a546c4) frozen behaviour table: a Synapse
# scheduled task never starts a console program directly; its action is
# wscript.exe running run-hidden.vbs with the program and its arguments.
# WScript.Arguments cannot carry an embedded double quote and treats quotes as
# plain toggles, so such arguments are refused instead of being mangled.
# Manager-owned, work method v3: Codex and Opus never edit this file.
# Run: pwsh -NoProfile -File scripts/tests/hidden-task-runner_table.ps1
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'table-harness.ps1')
Import-SynapseScriptFunction -Path (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-hidden-runner.ps1') -Name 'Format-SynapseHiddenRunnerAction'

$wscript = 'C:\Windows\System32\wscript.exe'
$runner = 'C:\Users\Adam\Synapse\scripts\run-hidden.vbs'
$pwsh = 'C:\Program Files\PowerShell\7\pwsh.exe'

function Test-ActionRow {
    param([string]$Runner, [string[]]$ArgumentList, [string]$ExpectedArgument)
    $action = Format-SynapseHiddenRunnerAction -WscriptPath $wscript -RunnerPath $Runner -ArgumentList $ArgumentList
    Join-SynapseTableProblems @(
        (Compare-SynapseTableValue 'Execute' $wscript $action.Execute),
        (Compare-SynapseTableValue 'Argument' $ExpectedArgument $action.Argument)
    )
}

function Test-RefusedRow {
    param([string]$Runner, [string[]]$ArgumentList)
    Compare-SynapseTableValue 'thrown code' 'SYNAPSE_HIDDEN_RUNNER_ARGUMENT_UNSUPPORTED' (Get-SynapseTableThrownCode {
        Format-SynapseHiddenRunnerAction -WscriptPath $wscript -RunnerPath $Runner -ArgumentList $ArgumentList
    })
}

$rows = @(
    @{ Name = 'H1a_maintenance_task_runs_pwsh_through_wscript_runner'; Check = {
        Test-ActionRow $runner @($pwsh, '-NoProfile', '-File', 'C:\Users\Adam\Synapse\scripts\repo-maintenance.ps1', '-Apply', '-Root', 'C:\Users\Adam') `
            ('//B //Nologo C:\Users\Adam\Synapse\scripts\run-hidden.vbs "C:\Program Files\PowerShell\7\pwsh.exe" -NoProfile -File C:\Users\Adam\Synapse\scripts\repo-maintenance.ps1 -Apply -Root C:\Users\Adam')
    } },
    @{ Name = 'H1b_spaces_and_empty_argument_are_quoted'; Check = {
        Test-ActionRow $runner @('C:\x\pwsh.exe', 'two words', '') `
            ('//B //Nologo C:\Users\Adam\Synapse\scripts\run-hidden.vbs C:\x\pwsh.exe "two words" ""')
    } },
    @{ Name = 'H1c_boundary_unquoted_trailing_backslash_kept_verbatim'; Check = {
        Test-ActionRow $runner @('C:\x\pwsh.exe', '-Root', 'C:\code\') `
            ('//B //Nologo C:\Users\Adam\Synapse\scripts\run-hidden.vbs C:\x\pwsh.exe -Root C:\code\')
    } },
    @{ Name = 'H1d_runner_path_with_spaces_is_quoted'; Check = {
        Test-ActionRow 'C:\Program Files\Syn\run-hidden.vbs' @('C:\x\pwsh.exe') `
            ('//B //Nologo "C:\Program Files\Syn\run-hidden.vbs" C:\x\pwsh.exe')
    } },
    @{ Name = 'H1e_hostile_quoted_argument_ending_in_backslash_refused'; Check = { Test-RefusedRow $runner @('C:\x\pwsh.exe', 'C:\my code\') } },
    @{ Name = 'H1f_hostile_embedded_double_quote_refused'; Check = { Test-RefusedRow $runner @('C:\x\pwsh.exe', 'a"b') } },
    @{ Name = 'H1g_empty_no_program_refused'; Check = { Test-RefusedRow $runner @() } },
    @{ Name = 'H1h_empty_program_name_refused'; Check = { Test-RefusedRow $runner @('') } },
    @{ Name = 'H1i_empty_runner_path_refused'; Check = { Test-RefusedRow '' @('C:\x\pwsh.exe') } }
)

Invoke-SynapseTableRows -Table 'hidden task runner' -Rows $rows
