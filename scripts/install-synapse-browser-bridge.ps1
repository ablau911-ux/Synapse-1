param(
    # Browser row id (file stem of crates\synapse-browsers\browsers\<id>.json).
    [string]$Browser = 'chrome',
    # Directory holding the browser rows; default: the checkout's
    # crates\synapse-browsers\browsers.
    [string]$BrowsersDir,
    # Read-only: resolve the row, family installer and executable, print the
    # plan, and exit without installing anything.
    [switch]$ResolveOnly,
    # Named parameters forwarded to the family installer, e.g.
    # @{ SkipAutoInstall = $true; AutoInstallTimeoutSeconds = 120 }.
    [hashtable]$InstallerParameters = @{}
)

# Multi-browser bridge dispatcher: -Browser <id> -> the row's family installer.
# A browser that is not installed is reported as a skip with readback (never an
# error), so setup can loop every row best-effort.

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'synapse-browser-rows.ps1')

$row = Get-SynapseBrowserRow -Browser $Browser -BrowsersDir $BrowsersDir
$installer = Get-SynapseBrowserFamilyInstaller -Row $row
$executable = Resolve-SynapseBrowserExecutable -Row $row
$plan = [pscustomobject]@{
    browser = [string]$row.id
    label = [string]$row.label
    family = [string]$row.family
    health_key = [string]$row.health_key
    installer = $installer
    installer_present = (Test-Path -LiteralPath $installer -PathType Leaf)
    executable = if ($executable) { $executable.path } else { $null }
    executable_source_of_truth = if ($executable) { $executable.source_of_truth } else { $null }
    rows_dir = Get-SynapseBrowserRowsDir -BrowsersDir $BrowsersDir
}

if ($null -eq $executable) {
    [pscustomobject]@{
        ok = $true
        browser = $plan.browser
        status = 'not_installed'
        reason = 'browser_executable_not_found'
        plan = $plan
        remediation = if ($row.winget_id) { "winget install $($row.winget_id)" } else { 'install the browser' }
    }
    return
}
if ($ResolveOnly) {
    [pscustomobject]@{
        ok = $true
        browser = $plan.browser
        status = 'resolved'
        plan = $plan
    }
    return
}
if (-not $plan.installer_present) {
    throw "SYNAPSE_BROWSER_BRIDGE_FAMILY_INSTALLER_MISSING browser=$($row.id) family=$($row.family) installer=$installer remediation=the $($row.family) package ships this installer"
}
& $installer -Browser $row.id -BrowsersDir $plan.rows_dir @InstallerParameters
