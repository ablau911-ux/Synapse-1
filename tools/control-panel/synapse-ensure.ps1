# synapse-ensure.ps1 - SessionStart shim.
# All logic lives in synapse_core.py (single source of truth). This just invokes
# `ensure`: start the daemon unless the manual disable flag is set.
$py = if (Test-Path 'C:\Python313\python.exe') { 'C:\Python313\python.exe' } else { 'python' }
$core = Join-Path $PSScriptRoot 'synapse_core.py'
& $py $core ensure
exit $LASTEXITCODE
