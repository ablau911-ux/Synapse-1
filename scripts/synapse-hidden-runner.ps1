# Scheduled-task action builder shared by synapse-setup.ps1 and
# install-maintenance-task.ps1 (dot-sourced; defines functions only).

function Format-SynapseHiddenRunnerAction {
    # A scheduled task must never start a console program directly: Task Scheduler
    # gives it a visible console before -WindowStyle Hidden applies. The action is
    # wscript.exe running run-hidden.vbs, which starts the program with SW_HIDE and
    # returns its exit code. WScript.Arguments treats double quotes as plain toggles
    # and cannot carry an embedded quote, so an argument with a quote, or a quoted
    # argument ending in a backslash, is refused instead of arriving mangled.
    param(
        [string]$WscriptPath,
        [string]$RunnerPath,
        [AllowEmptyCollection()][AllowEmptyString()][string[]]$ArgumentList = @()
    )
    if ([string]::IsNullOrWhiteSpace($WscriptPath) -or
        [string]::IsNullOrWhiteSpace($RunnerPath) -or
        $ArgumentList.Count -eq 0 -or
        [string]::IsNullOrWhiteSpace($ArgumentList[0])) {
        throw "SYNAPSE_HIDDEN_RUNNER_ARGUMENT_UNSUPPORTED reason=missing_wscript_runner_or_program remediation=pass wscript.exe, run-hidden.vbs and the program path"
    }
    $quoted = foreach ($part in @($RunnerPath) + @($ArgumentList)) {
        if ($part.Contains('"')) {
            throw "SYNAPSE_HIDDEN_RUNNER_ARGUMENT_UNSUPPORTED reason=embedded_double_quote argument=$part remediation=WScript.Arguments cannot carry a double quote; give this command its own launcher"
        }
        if ($part.Length -gt 0 -and $part -notmatch '[ \t]') {
            $part
            continue
        }
        if ($part.EndsWith('\')) {
            throw "SYNAPSE_HIDDEN_RUNNER_ARGUMENT_UNSUPPORTED reason=quoted_trailing_backslash argument=$part remediation=drop the trailing backslash from the path"
        }
        '"' + $part + '"'
    }
    return [pscustomobject]@{
        Execute = $WscriptPath
        Argument = '//B //Nologo ' + (@($quoted) -join ' ')
    }
}
