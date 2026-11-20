# Shared harness for the frozen PowerShell behaviour tables (work method v3).
# A table script dot-sources this file, imports the pure functions it tests
# straight from the production script (AST extraction: the script body never
# runs), runs one case per row and exits non-zero listing every failed row.
# No Pester dependency.

function Import-SynapseScriptFunction {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string[]]$Name
    )
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    if ($errors.Count -gt 0) {
        throw "TABLE_SETUP_PARSE_FAILED path=$Path errors=$($errors | Out-String)"
    }
    foreach ($functionName in $Name) {
        $definition = $ast.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq $functionName
        }, $true)
        if ($null -eq $definition) {
            throw "TABLE_SETUP_FUNCTION_MISSING path=$Path function=$functionName"
        }
        # Define the function in the caller's script scope.
        . ([scriptblock]::Create($definition.Extent.Text.Replace("function $functionName", "function script:$functionName")))
    }
}

function Invoke-SynapseTableRows {
    # $Rows: array of @{ Name = '<row id>'; Check = { <returns $null when the row
    # passes, or a string describing expected vs observed> } }.
    param(
        [Parameter(Mandatory = $true)][string]$Table,
        [Parameter(Mandatory = $true)][object[]]$Rows
    )
    $failures = @()
    foreach ($row in $Rows) {
        try {
            $problem = & $row.Check
        } catch {
            $problem = "threw: $($_.Exception.Message)"
        }
        if (-not [string]::IsNullOrEmpty([string]$problem)) {
            $failures += "row $($row.Name): $problem"
        }
    }
    if ($failures.Count -gt 0) {
        [Console]::Error.WriteLine("$Table table rows failed ($($failures.Count)/$($Rows.Count)):")
        foreach ($failure in $failures) { [Console]::Error.WriteLine("  $failure") }
        exit 1
    }
    Write-Host "$Table table: all $($Rows.Count) rows pass"
    exit 0
}

function Compare-SynapseTableValue {
    # Returns $null when equal, else "<label> expected <e> observed <o>".
    param([string]$Label, $Expected, $Observed)
    # -InputObject keeps arrays as arrays (a pipeline would unroll them).
    $e = ConvertTo-Json -InputObject $Expected -Compress -Depth 6
    $o = ConvertTo-Json -InputObject $Observed -Compress -Depth 6
    if ($e -ceq $o) { return $null }
    return "$Label expected $e observed $o"
}

function Join-SynapseTableProblems {
    param([object[]]$Problems)
    $real = @($Problems | Where-Object { -not [string]::IsNullOrEmpty([string]$_) })
    if ($real.Count -eq 0) { return $null }
    return ($real -join '; ')
}

function Get-SynapseTableThrownCode {
    # Runs $Action; returns the leading SYNAPSE_* code of the thrown message,
    # '<no throw>' when it returned, or '<other> <message>' for any other error.
    param([Parameter(Mandatory = $true)][scriptblock]$Action)
    try {
        & $Action | Out-Null
        return '<no throw>'
    } catch {
        $message = [string]$_.Exception.Message
        if ($message -match '^(SYNAPSE_[A-Z0-9_]+)') { return $Matches[1] }
        return "<other> $message"
    }
}
