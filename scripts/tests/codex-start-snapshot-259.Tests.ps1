# Pester 3.4, Windows PowerShell 5.1 / PowerShell 7. No installer or real Codex.
# Extract the generated launcher here-strings (cmd, ps1, sh) through the setup AST, never
# dot-source setup. The only substituted boundary is the Node/Codex invocation itself: a
# probe reads the actual launcher-created snapshot, so every name, copy, prune, export and
# self-delete step still comes from the real launcher text. All files live in TestDrive and
# every child process runs hidden (UseShellExecute=false, CreateNoWindow=true).
# Round 2 (#259 review): deterministic race via a PSModulePath-shadowed CimCmdlets module that
# pauses the process-table read, CIM failure injection, fresh legacy names, self-delete on exit.
param(
    # Negative control affects only the extracted TestDrive fixture, never setup.
    [ValidateSet('None', 'DeleteCurrentSnapshot')]
    [string]$NegativeControl = 'None'
)
$ErrorActionPreference = 'Stop'
$bash259 = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
$noBash259 = -not (Test-Path -LiteralPath $bash259)

Describe 'Generated Codex launcher start snapshots #259' {
    BeforeAll {
        $setup = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null; $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setup, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw ($errors | Out-String) }
        function Get-Literal259([string]$Prefix) {
            $found = @($ast.FindAll({ param($n)
                $n -is [System.Management.Automation.Language.StringConstantExpressionAst] -and
                $n.Value.StartsWith($Prefix) -and $n.Value.Contains('codex-start-snapshots')
            }, $true))
            if ($found.Count -ne 1) { throw "Expected exactly one generated Codex launcher starting with $Prefix" }
            return $found[0].Value
        }
        function Set-Invocation259([string]$Text, [string]$Old, [string]$New, [int]$Expected) {
            $count = ($Text.Length - $Text.Replace($Old, '').Length) / $Old.Length
            if ($count -ne $Expected) { throw "Codex invocation '$Old' found $count times, expected $Expected" }
            return $Text.Replace($Old, $New)
        }
        $probeCall = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File'
        $cmd = Set-Invocation259 (Get-Literal259 '@ECHO off') '"%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %*' ('"%ENGINE259%" ' + $probeCall + ' "%CASE259%\probe.ps1"') 1
        if ($NegativeControl -eq 'DeleteCurrentSnapshot') {
            $export = 'SET "SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START=!_synapse_start_surface!"'
            if (-not $cmd.Contains($export)) { throw 'Negative-control insertion point missing' }
            $cmd = $cmd.Replace($export, ('DEL /Q "!_synapse_start_surface!"' + "`r`n  " + $export))
        }
        $script:launcher259 = @{ cmd = $cmd }
        $ps1 = Get-Literal259 '#!/usr/bin/env pwsh'
        $ps1Probe = '& $env:ENGINE259 ' + $probeCall + ' (Join-Path $env:CASE259 ''probe.ps1'')'
        $ps1 = Set-Invocation259 $ps1 '& "$basedir/node$exe"  "$basedir/node_modules/@openai/codex/bin/codex.js" $args' $ps1Probe 2
        $script:launcher259.ps1 = Set-Invocation259 $ps1 '& "node$exe"  "$basedir/node_modules/@openai/codex/bin/codex.js" $args' $ps1Probe 2
        $sh = Get-Literal259 '#!/bin/sh'
        $shProbe = '"$ENGINE259" ' + $probeCall + ' "$CASE259/probe.ps1"'
        $sh = Set-Invocation259 $sh '"$basedir/node"  "$basedir/node_modules/@openai/codex/bin/codex.js" "$@"' $shProbe 1
        $sh = Set-Invocation259 $sh 'node  "$basedir/node_modules/@openai/codex/bin/codex.js" "$@"' $shProbe 1
        $script:launcher259.sh = $sh -replace "`r?`n", "`n"
        $script:engine259 = (Get-Process -Id $PID).Path

        function New-Case259([string]$Kind = 'cmd') {
            $root = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
            foreach ($dir in @('home\.codex', 'roaming\synapse', 'local\synapse\codex-start-snapshots')) {
                [void][IO.Directory]::CreateDirectory((Join-Path $root $dir))
            }
            [IO.File]::WriteAllText((Join-Path $root 'home\.codex\config.toml'), "[mcp_servers.synapse]`r`n")
            [IO.File]::WriteAllText((Join-Path $root 'roaming\synapse\token.txt'), 'synthetic-token-259')
            $surface = "{`r`n  `"tool_surface_sha256`": `"$('a' * 64)`",`r`n  `"tool_count`": 42`r`n}`r`n"
            [IO.File]::WriteAllText((Join-Path $root 'roaming\synapse\codex-tool-surface.json'), $surface)
            $probe = @'
$ErrorActionPreference = 'Stop'
$path = $env:SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START
$report = Join-Path $env:CASE259 ($env:START259 + '.json')
$stream = $null
try {
    if (-not $path -or -not [IO.File]::Exists($path)) { throw 'Current start snapshot deleted or missing before consumer' }
    $bytes = [IO.File]::ReadAllText($path)
    if ($env:LOCK259 -eq '1') { $stream = [IO.File]::Open($path, 'Open', 'Read', 'Read') }
    [IO.File]::WriteAllText($report, (@{path=$path; bytes=$bytes; pid=$PID} | ConvertTo-Json -Compress))
    $timer = [Diagnostics.Stopwatch]::StartNew()
    while (-not [IO.File]::Exists((Join-Path $env:CASE259 'release'))) {
        if ($timer.Elapsed.TotalSeconds -gt 40) { throw 'Probe release timeout' }
        Start-Sleep -Milliseconds 20
    }
    if (-not [IO.File]::Exists($path)) { throw 'Running consumer snapshot deleted' }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 73
} finally { if ($stream) { $stream.Dispose() } }
exit ([int]('0' + $env:EXIT259))
'@
            [IO.File]::WriteAllText((Join-Path $root 'probe.ps1'), $probe)
            $file = @{ cmd = 'codex.cmd'; ps1 = 'codex.ps1'; sh = 'codex' }[$Kind]
            $encoding = if ($Kind -eq 'cmd') { [Text.Encoding]::ASCII } else { New-Object Text.UTF8Encoding($false) }
            [IO.File]::WriteAllText((Join-Path $root $file), $script:launcher259[$Kind], $encoding)
            return $root
        }

        # Environment-level CIM stand-in for the launcher's PS 5.1 helper: proxies the real
        # cmdlet, or fails (CIM259=down), or pauses after the unfiltered process-table read
        # (CIM259=pause) until the test creates 'cim-go'. Production text is not modified.
        function New-CimShadow259([string]$Root) {
            $dir = Join-Path $Root 'mods\CimCmdlets'
            [void][IO.Directory]::CreateDirectory($dir)
            $module = @'
Import-Module (Join-Path $PSHOME 'Modules\CimCmdlets\CimCmdlets.psd1') -Prefix Real259 -Global
function Get-CimInstance {
    [CmdletBinding()] param([Parameter(Position=0)][string]$ClassName, [string]$Filter, [string[]]$Property)
    if ($env:CIM259 -eq 'down') { throw 'SYNTHETIC_CIM_DOWN_259' }
    $rows = @(Get-Real259CimInstance @PSBoundParameters)
    if ($env:CIM259 -eq 'pause' -and -not $Filter) {
        [IO.File]::WriteAllText((Join-Path $env:CASE259 'cim-table-taken'), 'taken')
        $timer = [Diagnostics.Stopwatch]::StartNew()
        while (-not [IO.File]::Exists((Join-Path $env:CASE259 'cim-go')) -and $timer.Elapsed.TotalSeconds -lt 30) { Start-Sleep -Milliseconds 20 }
    }
    $rows
}
'@
            [IO.File]::WriteAllText((Join-Path $dir 'CimCmdlets.psm1'), $module)
            return ((Join-Path $Root 'mods') + ';' + (Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\Modules'))
        }

        function Start-Wrapper259([string]$Root, [string]$Id, [string]$Kind = 'cmd', [switch]$LockSnapshot, [hashtable]$Env = @{}) {
            $psi = New-Object Diagnostics.ProcessStartInfo
            $engine = $script:engine259
            switch ($Kind) {
                'cmd' { $psi.FileName = $env:ComSpec; $psi.Arguments = '/d /c ""' + (Join-Path $Root 'codex.cmd') + '""' }
                'ps1' { $psi.FileName = $engine; $psi.Arguments = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + (Join-Path $Root 'codex.ps1') + '"' }
                'sh' { $psi.FileName = $bash259; $psi.Arguments = '"' + ((Join-Path $Root 'codex') -replace '\\', '/') + '"'; $engine = $engine -replace '\\', '/' }
            }
            $psi.WorkingDirectory = $Root
            $psi.UseShellExecute = $false
            $psi.CreateNoWindow = $true
            $psi.RedirectStandardOutput = $true
            $psi.RedirectStandardError = $true
            $psi.EnvironmentVariables['USERPROFILE'] = Join-Path $Root 'home'
            $psi.EnvironmentVariables['APPDATA'] = Join-Path $Root 'roaming'
            $psi.EnvironmentVariables['LOCALAPPDATA'] = Join-Path $Root 'local'
            $psi.EnvironmentVariables['TEMP'] = $Root
            $psi.EnvironmentVariables['TMP'] = $Root
            $psi.EnvironmentVariables['CASE259'] = $Root
            $psi.EnvironmentVariables['START259'] = $Id
            $psi.EnvironmentVariables['ENGINE259'] = $engine
            $psi.EnvironmentVariables['LOCK259'] = [string][int]$LockSnapshot.IsPresent
            # Deterministic same-clock-seed surrogate: CMD allows RANDOM to be shadowed.
            $psi.EnvironmentVariables['RANDOM'] = '259'
            foreach ($key in @('SYNAPSE_BEARER_TOKEN', 'SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START',
                'SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START', 'SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START',
                'SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS', 'SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES',
                'CIM259', 'EXIT259')) {
                $psi.EnvironmentVariables.Remove($key)
            }
            foreach ($key in $Env.Keys) { $psi.EnvironmentVariables[$key] = [string]$Env[$key] }
            $p = New-Object Diagnostics.Process
            $p.StartInfo = $psi
            [void]$p.Start()
            return $p
        }

        function Wait-Probe259([string]$Root, [string]$Id, $Process) {
            $report = Join-Path $Root ($Id + '.json')
            $timer = [Diagnostics.Stopwatch]::StartNew()
            while (-not [IO.File]::Exists($report) -and -not $Process.HasExited -and $timer.Elapsed.TotalSeconds -lt 30) {
                Start-Sleep -Milliseconds 20
            }
            if ([IO.File]::Exists($report)) { return (Get-Content -Raw -LiteralPath $report | ConvertFrom-Json) }
            return $null
        }

        function Wait-File259([string]$Path) {
            $timer = [Diagnostics.Stopwatch]::StartNew()
            while (-not [IO.File]::Exists($Path) -and $timer.Elapsed.TotalSeconds -lt 30) { Start-Sleep -Milliseconds 20 }
            return [IO.File]::Exists($Path)
        }

        function Complete-Starts259([string]$Root, [array]$Processes) {
            [IO.File]::WriteAllText((Join-Path $Root 'release'), 'release')
            foreach ($p in $Processes) {
                if (-not $p.WaitForExit(45000)) { $p.Kill(); $p.WaitForExit(); throw 'Launcher timeout' }
                $stdout = $p.StandardOutput.ReadToEnd()
                $stderr = $p.StandardError.ReadToEnd()
                [pscustomobject]@{ Code = $p.ExitCode; Out = $stdout; Error = $stderr }
                $p.Dispose()
            }
        }

        function New-Snapshot259([string]$Root, [string]$Name, [double]$AgeMinutes) {
            $path = Join-Path $Root ('local\synapse\codex-start-snapshots\' + $Name)
            [IO.File]::WriteAllText($path, '{"old":true}')
            [IO.File]::SetLastWriteTimeUtc($path, [DateTime]::UtcNow.AddMinutes(-$AgeMinutes))
            return $path
        }

        $script:limits259 = @{ SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES = '10'; SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS = '168' }
        $script:guid259 = { [guid]::NewGuid().ToString('N') }

        # Shared prune matrix for every launcher kind. A PID above Windows' range is never live;
        # the Pester host's own PID is a live owner whose snapshot is older than every limit.
        function Test-PruneMatrix259([string]$Kind) {
            $root = New-Case259 $Kind
            $seed = [ordered]@{
                deadOld     = New-Snapshot259 $root ('codex-tool-surface-o2147483647-' + (& $script:guid259) + '.json') 60
                deadFresh   = New-Snapshot259 $root ('codex-tool-surface-o2147483646-' + (& $script:guid259) + '.json') 0
                liveOld     = New-Snapshot259 $root ('codex-tool-surface-o' + $PID + '-' + (& $script:guid259) + '.json') (8 * 24 * 60)
                legacyFresh = New-Snapshot259 $root 'codex-tool-surface-2147483645-5012.json' 0
                legacyOld   = New-Snapshot259 $root 'codex-tool-surface-31071-5012.json' (90 * 24 * 60)
                unknownOld  = New-Snapshot259 $root ('codex-tool-surface-u-' + (& $script:guid259) + '.json') (90 * 24 * 60)
            }
            $p = Start-Wrapper259 $root "prune-$Kind" $Kind -Env $script:limits259
            $report = Wait-Probe259 $root "prune-$Kind" $p
            $state = ($seed.Keys | ForEach-Object { '{0}={1}' -f $_, [IO.File]::Exists($seed[$_]) }) -join ','
            $results = @(Complete-Starts259 $root @($p))
            $report | Should Not Be $null
            $report.path | Should Match ('codex-tool-surface-o\d+-[0-9a-f]+\.json$')
            $state | Should Be 'deadOld=False,deadFresh=True,liveOld=True,legacyFresh=True,legacyOld=False,unknownOld=True'
            $results[0].Code | Should Be 0
        }

        function Test-SelfDelete259([string]$Kind) {
            $root = New-Case259 $Kind
            $fail = Start-Wrapper259 $root "fail-$Kind" $Kind -Env @{ EXIT259 = '7' }
            $ok = Start-Wrapper259 $root "ok-$Kind" $Kind
            $a = Wait-Probe259 $root "fail-$Kind" $fail
            $b = Wait-Probe259 $root "ok-$Kind" $ok
            $during = @($a, $b | ForEach-Object { $null -ne $_ -and [IO.File]::Exists($_.path) }) -join ','
            $results = @(Complete-Starts259 $root @($fail, $ok))
            $during | Should Be 'True,True'
            (($results | ForEach-Object { $_.Code }) -join ',') | Should Be '7,0'
            (@($a.path, $b.path | ForEach-Object { [IO.File]::Exists($_) }) -join ',') | Should Be 'False,False'
        }
    }

    It 'must_fail_same_second_seed_collision_is_rejected_without_failing_the_second_start' {
        $root = New-Case259
        $first = Start-Wrapper259 $root 'first' -LockSnapshot
        $a = Wait-Probe259 $root 'first' $first
        $second = Start-Wrapper259 $root 'second'
        $b = Wait-Probe259 $root 'second' $second
        $results = @(Complete-Starts259 $root @($first, $second))
        Write-Host ('#259 collision: exits=' + (($results | ForEach-Object { $_.Code }) -join ',') +
            '; stderr=' + (($results | ForEach-Object { $_.Error.Trim() }) -join ' | '))
        ($results | ForEach-Object { $_.Code }) -join ',' | Should Be '0,0'
        $b | Should Not Be $null
        $b.path | Should Not Be $a.path
    }

    It 'must_pass_parallel_starts_have_N_unique_readable_snapshots_and_all_succeed' {
        $root = New-Case259
        $n = 6
        $processes = @(); $reports = @()
        for ($i = 0; $i -lt $n; $i++) { $processes += Start-Wrapper259 $root "parallel-$i" }
        for ($i = 0; $i -lt $n; $i++) { $reports += Wait-Probe259 $root "parallel-$i" $processes[$i] }
        $results = @(Complete-Starts259 $root $processes)
        @($reports | Where-Object { $null -ne $_ }).Count | Should Be $n
        @($reports | ForEach-Object { $_.path } | Select-Object -Unique).Count | Should Be $n
        @($results | Where-Object { $_.Code -ne 0 }).Count | Should Be 0
        $expected = [IO.File]::ReadAllText((Join-Path $root 'roaming\synapse\codex-tool-surface.json'))
        foreach ($r in $reports) { $r.bytes | Should BeExactly $expected }
    }

    It 'must_pass_cmd_prunes_only_proven_orphans_and_keeps_live_unknown_and_fresh_snapshots' {
        Test-PruneMatrix259 'cmd'
    }

    It 'must_pass_cmd_running_start_keeps_its_snapshot_while_another_start_prunes' {
        $root = New-Case259
        $first = Start-Wrapper259 $root 'live' -Env $script:limits259
        # Deliberately unlocked: OS sharing must not mask a wrong deletion of a live start.
        $live = Wait-Probe259 $root 'live' $first
        # Age the live start's file past every limit: only its live owner may protect it.
        if ($null -ne $live) { [IO.File]::SetLastWriteTimeUtc($live.path, [DateTime]::UtcNow.AddDays(-30)) }
        $second = Start-Wrapper259 $root 'cleanup' -Env $script:limits259
        $current = Wait-Probe259 $root 'cleanup' $second
        $liveExists = $null -ne $live -and [IO.File]::Exists($live.path)
        $results = @(Complete-Starts259 $root @($first, $second))
        $liveExists | Should Be $true
        $current | Should Not Be $null
        @($results | Where-Object { $_.Code -ne 0 }).Count | Should Be 0
    }

    It 'must_fail_cleanup_cannot_delete_the_snapshot_the_current_start_just_wrote' {
        $root = New-Case259
        [void](New-Snapshot259 $root 'codex-tool-surface-31071-5012.json' (90 * 24 * 60))
        $p = Start-Wrapper259 $root 'current' -Env $script:limits259
        $report = Wait-Probe259 $root 'current' $p
        $existsBeforeExit = $null -ne $report -and [IO.File]::Exists($report.path)
        $results = @(Complete-Starts259 $root @($p))
        $results[0].Code | Should Be 0
        $report | Should Not Be $null
        $existsBeforeExit | Should Be $true
        $report.bytes | Should BeExactly ([IO.File]::ReadAllText((Join-Path $root 'roaming\synapse\codex-tool-surface.json')))
    }

    It 'must_fail_start_B_written_after_start_A_read_its_process_table_is_not_deleted_by_A' {
        $root = New-Case259
        $shadow = New-CimShadow259 $root
        $envA = @{ PSModulePath = $shadow; CIM259 = 'pause' } + $script:limits259
        $a = Start-Wrapper259 $root 'A' -Env $envA
        $paused = Wait-File259 (Join-Path $root 'cim-table-taken')
        # A is now frozen right after its process-table read. B starts and writes its snapshot.
        $b = Start-Wrapper259 $root 'B' -Env $script:limits259
        $rb = Wait-Probe259 $root 'B' $b
        # Age B past the grace window: only A's enumerate-then-liveness order can protect it now.
        if ($null -ne $rb) { [IO.File]::SetLastWriteTimeUtc($rb.path, [DateTime]::UtcNow.AddMinutes(-60)) }
        [IO.File]::WriteAllText((Join-Path $root 'cim-go'), 'go')
        $ra = Wait-Probe259 $root 'A' $a
        $bAfterA = $null -ne $rb -and [IO.File]::Exists($rb.path)
        $results = @(Complete-Starts259 $root @($a, $b))
        $paused | Should Be $true
        $rb | Should Not Be $null
        $ra | Should Not Be $null
        $bAfterA | Should Be $true
        (($results | ForEach-Object { $_.Code }) -join ',') | Should Be '0,0'
    }

    It 'must_fail_cim_failure_keeps_old_snapshots_and_still_starts' {
        $root = New-Case259
        $shadow = New-CimShadow259 $root
        $deadOld = New-Snapshot259 $root ('codex-tool-surface-o2147483647-' + (& $script:guid259) + '.json') (8 * 24 * 60)
        $liveOld = New-Snapshot259 $root ('codex-tool-surface-o' + $PID + '-' + (& $script:guid259) + '.json') (8 * 24 * 60)
        $p = Start-Wrapper259 $root 'cimdown' -Env (@{ PSModulePath = $shadow; CIM259 = 'down' } + $script:limits259)
        $report = Wait-Probe259 $root 'cimdown' $p
        $state = (@($deadOld, $liveOld) | ForEach-Object { [IO.File]::Exists($_) }) -join ','
        $results = @(Complete-Starts259 $root @($p))
        $report | Should Not Be $null
        $report.path | Should Match 'codex-tool-surface-u-[0-9a-f]{32}\.json$'
        $state | Should Be 'True,True'
        $results[0].Code | Should Be 0
    }

    It 'must_fail_fresh_legacy_random_name_is_not_read_as_a_dead_owner' {
        $root = New-Case259
        $legacy = New-Snapshot259 $root 'codex-tool-surface-2147483645-5012.json' 0
        $legacyPs = New-Snapshot259 $root ('codex-tool-surface-2147483644-' + (& $script:guid259) + '.json') 0
        $p = Start-Wrapper259 $root 'legacy' -Env $script:limits259
        $report = Wait-Probe259 $root 'legacy' $p
        $state = (@($legacy, $legacyPs) | ForEach-Object { [IO.File]::Exists($_) }) -join ','
        $results = @(Complete-Starts259 $root @($p))
        $report | Should Not Be $null
        $state | Should Be 'True,True'
        $results[0].Code | Should Be 0
    }

    It 'must_pass_cmd_deletes_its_own_snapshot_on_exit_and_keeps_the_exit_code' {
        Test-SelfDelete259 'cmd'
    }

    It 'must_pass_ps1_deletes_its_own_snapshot_on_exit_and_keeps_the_exit_code' {
        Test-SelfDelete259 'ps1'
    }

    It 'must_pass_ps1_prunes_only_proven_orphans' {
        Test-PruneMatrix259 'ps1'
    }

    It 'must_pass_sh_deletes_its_own_snapshot_on_exit_and_keeps_the_exit_code' -Skip:$noBash259 {
        Test-SelfDelete259 'sh'
    }

    It 'must_pass_sh_prunes_only_proven_orphans' -Skip:$noBash259 {
        Test-PruneMatrix259 'sh'
    }
}

