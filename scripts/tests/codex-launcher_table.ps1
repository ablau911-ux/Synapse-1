# Codex launcher fixes (upstream 0e5df451 #2269, 4518fcff) frozen behaviour
# table. (1) The [mcp_servers.synapse] section is written with required=false
# and startup_timeout_sec=20 so a Synapse outage never stops Codex from
# opening; other MCP server sections stay byte-identical. (2) Every Codex
# launcher setup patches calls the real package entry point by absolute path,
# including a copied shim that wins on PATH without its own node_modules;
# unsafe paths and a drifted template are refused (fail closed).
# Manager-owned, work method v3: Codex and Opus never edit this file.
# Run: pwsh -NoProfile -File scripts/tests/codex-launcher_table.ps1
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'table-harness.ps1')
Import-SynapseScriptFunction -Path (Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1') -Name @(
    'Update-CodexSynapseSectionText',
    'Test-CodexSynapseHttpConfigBody',
    'Resolve-CodexLauncherPlan',
    'Set-CodexLauncherEntrypoint'
)

$bind = '127.0.0.1:7700'
$serena = "[mcp_servers.serena]`r`ncommand = `"serena`"`r`nrequired = true`r`nstartup_timeout_sec = 90`r`n"

function Get-TomlSection {
    param([string]$Content, [string]$Header)
    $match = [regex]::Match($Content, '(?ms)^\[' + [regex]::Escape($Header) + '\]\s*(?<body>.*?)(?=^\[|\z)')
    if (-not $match.Success) { return $null }
    # Trailing blank lines belong to the separator before the next section.
    return $match.Value.TrimEnd()
}

function Test-SectionRow {
    param([string]$Content, [string[]]$MustContainLines, [string[]]$MustNotContain)
    $updated = Update-CodexSynapseSectionText -Content $Content -Bind $bind
    $synapse = Get-TomlSection $updated 'mcp_servers.synapse'
    if ($null -eq $synapse) { return 'synapse section missing after update' }
    $problems = @()
    foreach ($line in $MustContainLines) {
        $count = [regex]::Matches($synapse, '(?m)^' + [regex]::Escape($line) + '\s*$').Count
        if ($count -ne 1) { $problems += "line '$line' expected once in synapse section, observed $count" }
    }
    foreach ($text in $MustNotContain) {
        if ($synapse.Contains($text)) { $problems += "synapse section still contains '$text'" }
    }
    $serenaBefore = Get-TomlSection $Content 'mcp_servers.serena'
    if ($null -ne $serenaBefore) {
        $serenaAfter = Get-TomlSection $updated 'mcp_servers.serena'
        if ($serenaAfter -cne $serenaBefore) { $problems += 'serena section changed' }
    }
    Join-SynapseTableProblems $problems
}

$policyLines = @(
    'url = "http://127.0.0.1:7700/mcp"',
    'bearer_token_env_var = "SYNAPSE_BEARER_TOKEN"',
    'required = false',
    'startup_timeout_sec = 20',
    'default_tools_approval_mode = "approve"'
)

$npm = 'C:\Users\A\AppData\Roaming\npm'
$copied = 'C:\Users\A\.codex\bin'
$pkg = 'node_modules\@openai\codex\bin\codex.js'

function Test-PlanRow {
    param([string]$LauncherDir, [string[]]$ExistingPaths, $ExpectedCodexJs, [string[]]$ExpectedDirs)
    $plan = Resolve-CodexLauncherPlan -LauncherDir $LauncherDir -NpmGlobalDir $npm -ExistingPaths $ExistingPaths
    Join-SynapseTableProblems @(
        (Compare-SynapseTableValue 'CodexJs' $ExpectedCodexJs $plan.CodexJs),
        (Compare-SynapseTableValue 'TargetDirs' $ExpectedDirs @($plan.TargetDirs))
    )
}

$absJs = "$npm\$pkg"
$ps1Template = "# #259 snapshot loader stays`r`n  & `"`$basedir/node`$exe`"  `"`$basedir/node_modules/@openai/codex/bin/codex.js`" `$args`r`n  & `"node`$exe`"  `"`$basedir/node_modules/@openai/codex/bin/codex.js`" `$args`r`n"
$cmdTemplate = "goto #_undefined_# 2>NUL || title %COMSPEC% & `"%_prog%`"  `"%dp0%\node_modules\@openai\codex\bin\codex.js`" %* & CALL %COMSPEC% /d /c `"EXIT`"`r`n"
$shTemplate = "# #259: run node as a child`n  `"`$basedir/node`"  `"`$basedir/node_modules/@openai/codex/bin/codex.js`" `"`$@`"`n  node  `"`$basedir/node_modules/@openai/codex/bin/codex.js`" `"`$@`"`n"

function Test-EntrypointRow {
    param([string]$Template, [string]$Kind, [string]$CodexJs, [string]$ExpectedQuoted, [int]$ExpectedCount, [string]$KeptLine)
    $patched = Set-CodexLauncherEntrypoint -Text $Template -Kind $Kind -CodexJsPath $CodexJs
    $problems = @()
    foreach ($relative in @('$basedir/node_modules/@openai/codex/bin/codex.js', '%dp0%\node_modules\@openai\codex\bin\codex.js')) {
        if ($patched.Contains($relative)) { $problems += "relative entry point '$relative' still present" }
    }
    $count = ([regex]::Matches($patched, [regex]::Escape($ExpectedQuoted))).Count
    if ($count -ne $ExpectedCount) { $problems += "'$ExpectedQuoted' expected $ExpectedCount times, observed $count" }
    if (-not $patched.Contains($KeptLine)) { $problems += "line '$KeptLine' was not kept" }
    Join-SynapseTableProblems $problems
}

function Test-EntrypointRefusedRow {
    param([string]$Template, [string]$Kind, [string]$CodexJs, [string]$ExpectedCode)
    Compare-SynapseTableValue 'thrown code' $ExpectedCode (Get-SynapseTableThrownCode {
        Set-CodexLauncherEntrypoint -Text $Template -Kind $Kind -CodexJsPath $CodexJs
    })
}

$rows = @(
    # (1) config section policy
    @{ Name = 'C1a_existing_section_required_false_timeout_20_comment_kept_serena_untouched'; Check = {
        $content = "model = `"gpt`"`r`n`r`n[mcp_servers.synapse]`r`nurl = `"http://old:1/mcp`"`r`nrequired = true`r`nstartup_timeout_sec = 5400`r`n# keep me`r`n`r`n$serena"
        Join-SynapseTableProblems @(
            (Test-SectionRow $content ($policyLines + @('# keep me')) @('required = true', '5400', 'http://old:1/mcp'))
        )
    } },
    @{ Name = 'C1b_missing_section_is_appended_serena_untouched'; Check = {
        Test-SectionRow $serena $policyLines @('required = true')
    } },
    @{ Name = 'C1c_empty_config_gets_section'; Check = { Test-SectionRow '' $policyLines @('required = true') } },
    @{ Name = 'C1d_startup_timeout_ms_replaced_by_sec_20'; Check = {
        Test-SectionRow "[mcp_servers.synapse]`r`nstartup_timeout_ms = 900000`r`nrequired = false`r`n" $policyLines @('startup_timeout_ms')
    } },
    @{ Name = 'C1e_validator_accepts_required_false'; Check = {
        Compare-SynapseTableValue 'valid' $true (Test-CodexSynapseHttpConfigBody -Body (($policyLines -join "`r`n") + "`r`n") -Bind $bind)
    } },
    @{ Name = 'C1f_validator_rejects_required_true'; Check = {
        $body = ($policyLines -join "`r`n").Replace('required = false', 'required = true') + "`r`n"
        Compare-SynapseTableValue 'valid' $false (Test-CodexSynapseHttpConfigBody -Body $body -Bind $bind)
    } },
    @{ Name = 'C1g_validator_rejects_other_bind'; Check = {
        Compare-SynapseTableValue 'valid' $false (Test-CodexSynapseHttpConfigBody -Body (($policyLines -join "`r`n") + "`r`n") -Bind '127.0.0.1:7701')
    } },
    # (2) launcher plan: which codex.js, which launcher dirs
    @{ Name = 'C2a_npm_dir_wins_on_path_uses_its_package'; Check = {
        Test-PlanRow $npm @("$npm\codex.ps1", "$npm\codex.cmd", "$npm\codex", "$npm\$pkg") "$npm\$pkg" @($npm)
    } },
    @{ Name = 'C2b_second_actor_copied_shim_first_on_path_gets_npm_package_both_dirs_patched'; Check = {
        Test-PlanRow $copied @("$copied\codex.ps1", "$npm\codex.ps1", "$npm\codex.cmd", "$npm\$pkg") "$npm\$pkg" @($copied, $npm)
    } },
    @{ Name = 'C2c_empty_package_missing_everywhere_has_no_entry_point'; Check = {
        Test-PlanRow $copied @("$copied\codex.ps1", "$npm\codex.cmd") $null @($copied, $npm)
    } },
    @{ Name = 'C2d_launcher_local_package_preferred'; Check = {
        Test-PlanRow $copied @("$copied\codex.ps1", "$copied\$pkg", "$npm\codex.ps1", "$npm\$pkg") "$copied\$pkg" @($copied, $npm)
    } },
    @{ Name = 'C2e_boundary_same_dir_different_case_patched_once'; Check = {
        Test-PlanRow $npm.ToLowerInvariant() @("$npm\codex.ps1", "$npm\$pkg") "$($npm.ToLowerInvariant())\$pkg" @($npm.ToLowerInvariant())
    } },
    @{ Name = 'C2f_dir_without_any_codex_shim_not_patched'; Check = {
        Test-PlanRow $copied @("$copied\codex.cmd", "$npm\$pkg") "$npm\$pkg" @($copied)
    } },
    # (2) launcher text: absolute entry point
    @{ Name = 'C3a_ps1_both_calls_absolute_snapshot_loader_kept'; Check = {
        Test-EntrypointRow $ps1Template 'ps1' $absJs "`"$absJs`"" 2 '# #259 snapshot loader stays'
    } },
    @{ Name = 'C3b_cmd_call_absolute'; Check = {
        Test-EntrypointRow $cmdTemplate 'cmd' $absJs "`"$absJs`"" 1 'CALL %COMSPEC% /d /c'
    } },
    @{ Name = 'C3c_sh_calls_absolute_with_forward_slashes'; Check = {
        Test-EntrypointRow $shTemplate 'sh' $absJs ('"' + ($absJs -replace '\\', '/') + '"') 2 '# #259: run node as a child'
    } },
    @{ Name = 'C3d_boundary_path_with_space_and_apostrophe_ok'; Check = {
        $js = "C:\Users\O'Brien Smith\AppData\Roaming\npm\$pkg"
        Test-EntrypointRow $ps1Template 'ps1' $js "`"$js`"" 2 '# #259 snapshot loader stays'
    } },
    @{ Name = 'C3e_hostile_dollar_in_path_refused'; Check = {
        Test-EntrypointRefusedRow $ps1Template 'ps1' "C:\Users\a`$b\$pkg" 'SYNAPSE_CODEX_PACKAGE_PATH_UNSAFE'
    } },
    @{ Name = 'C3f_hostile_percent_in_path_refused'; Check = {
        Test-EntrypointRefusedRow $cmdTemplate 'cmd' "C:\Users\a%PATH%b\$pkg" 'SYNAPSE_CODEX_PACKAGE_PATH_UNSAFE'
    } },
    @{ Name = 'C3g_empty_path_refused'; Check = {
        Test-EntrypointRefusedRow $shTemplate 'sh' '' 'SYNAPSE_CODEX_PACKAGE_PATH_UNSAFE'
    } },
    @{ Name = 'C3h_drifted_template_without_relative_entry_point_refused'; Check = {
        Test-EntrypointRefusedRow "exec node `"`$basedir/other.js`"`n" 'sh' $absJs 'SYNAPSE_CODEX_LAUNCHER_TEMPLATE_DRIFT'
    } }
)

Invoke-SynapseTableRows -Table 'codex launcher' -Rows $rows
