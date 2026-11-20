$ErrorActionPreference = 'Stop'

# #262 variant (Pester 3.4.0, child processes: Windows PowerShell 5.1, plus pwsh 7 when installed).
# Added 2026-10-16 19:40 +03:00.
# Step 4 runs `& cargo-sweep sweep ... 2>&1 | ForEach-Object { Info ... }` under $ErrorActionPreference='Stop'.
# On Windows PowerShell 5.1 the FIRST stderr line of that native command becomes a terminating error and aborts
# the whole run; pwsh 7 logs the line as Info and carries on. A fake cargo-sweep.cmd (first on PATH) writes one
# stdout line and one stderr line and exits with the given code. Outcome-based: the run completes (exit 0, own
# summary line), the fake was really invoked with the documented arguments, and on pwsh 7 the stderr line is
# still logged (unchanged behaviour). How 5.1 surfaces the stderr text is left to the fix.
Describe 'repo-maintenance cargo-sweep stderr under Stop (#262 variant)' {
    BeforeAll {
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive
        . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')

        function Fx-ScenarioSweep([string]$Exe, [int]$SweepExit) {
            $case = "sweep-$(Fx-ExeTag $Exe)-exit$SweepExit"
            $fx = Fx-NewRepo $case
            Fx-Commit $fx.Repo 'Cargo.toml'   # step 4 only runs for a repo that has Cargo.toml
            $bin = Join-Path $TestDrive "$case\fakebin"
            New-Item -ItemType Directory -Force -Path $bin | Out-Null
            $log = Join-Path $bin 'sweep-args.log'
            $lines = @('@echo off', 'echo sweep-stdout-line', 'echo sweep-stderr-line 1>&2', "echo %* > `"$log`"", "exit /b $SweepExit")
            [System.IO.File]::WriteAllText((Join-Path $bin 'cargo-sweep.cmd'), (($lines -join "`r`n") + "`r`n"), [System.Text.Encoding]::ASCII)
            $r = Fx-RunMaint $fx $Exe @('-Apply') @{ PATH = "$bin;$env:PATH" }
            Fx-AssertCompleted $r
            $r.Text | Should Match 'free after'
            Test-Path -LiteralPath $log | Should Be $true
            [System.IO.File]::ReadAllText($log) | Should Match 'sweep --time \d+ --recursive'
            return $r
        }
        $script:noPwsh = [bool](-not $script:FxPwsh)
    }

    AfterAll { $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling }

    It 'must_fail_cargo_sweep_stderr_line_does_not_abort_the_run_exit_0' {
        $null = Fx-ScenarioSweep $script:FxPs51 0
    }
    It 'must_fail_cargo_sweep_stderr_line_with_nonzero_exit_does_not_abort_the_run' {
        $null = Fx-ScenarioSweep $script:FxPs51 1
    }
    It 'must_pass_pwsh7_cargo_sweep_stderr_line_is_still_logged_as_info' -Skip:$script:noPwsh {
        $r = Fx-ScenarioSweep $script:FxPwsh 0
        $r.Text | Should Match 'sweep: sweep-stdout-line'
        $r.Text | Should Match 'sweep: sweep-stderr-line'
    }
    It 'must_pass_pwsh7_cargo_sweep_nonzero_exit_does_not_abort_the_run' -Skip:$script:noPwsh {
        $null = Fx-ScenarioSweep $script:FxPwsh 1
    }
}
