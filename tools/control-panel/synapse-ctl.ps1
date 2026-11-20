# synapse-ctl.ps1 - command-line control shim.
# All daemon logic lives in synapse_core.py (single source of truth). This
# forwards commands to it. The interactive dashboard is now the Python GUI
# (synapse_panel.py); this remains for scripting / quick CLI use.
#
#   synapse-ctl.ps1 status|start|stop|reclaim|enable|disable|ensure
#   synapse-ctl.ps1            # no args -> status
param([Parameter(ValueFromRemainingArguments = $true)] [string[]] $Args)

$py = if (Test-Path 'C:\Python313\python.exe') { 'C:\Python313\python.exe' } else { 'python' }
$core = Join-Path $PSScriptRoot 'synapse_core.py'
if (-not $Args -or $Args.Count -eq 0) { $Args = @('status') }
& $py $core @Args
exit $LASTEXITCODE
