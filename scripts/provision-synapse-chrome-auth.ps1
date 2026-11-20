param(
    # Required unless -TokenOnly.
    [string]$ExtensionDir,
    [string]$TokenPath,
    # Browser row whose registration_token_file names the per-kind token
    # (the daemon derives the browser kind from which token file matches).
    [string]$Browser = 'chrome',
    [string]$BrowsersDir,
    # Create or validate only the registration token file and return its path;
    # never reads an extension manifest or writes bridge-auth.json (the gecko
    # installer delivers the token through Firefox managed storage instead).
    [switch]$TokenOnly
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'synapse-private-file.ps1')
if (-not $TokenOnly -and [string]::IsNullOrWhiteSpace($ExtensionDir)) {
    throw 'SYNAPSE_CHROME_BRIDGE_AUTH_EXTENSION_DIR_MISSING pass -ExtensionDir (the unpacked bridge extension directory) or -TokenOnly'
}
if ([string]::IsNullOrWhiteSpace($TokenPath)) {
    if ([string]::IsNullOrWhiteSpace($env:APPDATA)) {
        throw 'SYNAPSE_CHROME_BRIDGE_AUTH_APPDATA_MISSING'
    }
    . (Join-Path $PSScriptRoot 'synapse-browser-rows.ps1')
    $browserRow = Get-SynapseBrowserRow -Browser $Browser -BrowsersDir $BrowsersDir
    $TokenPath = Join-Path $env:APPDATA ('synapse\' + [string]$browserRow.registration_token_file)
}
$TokenPath = [System.IO.Path]::GetFullPath($TokenPath)
if (-not $TokenOnly) {
    $ExtensionDir = [System.IO.Path]::GetFullPath($ExtensionDir)
    $manifestPath = Join-Path $ExtensionDir 'manifest.json'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if ($null -ne $manifest.web_accessible_resources -and @($manifest.web_accessible_resources).Count -ne 0) {
        # This private bridge injects scripts via chrome.scripting and exposes no
        # packaged resources. Refuse every exposure rule, including ambiguous globs.
        throw 'SYNAPSE_CHROME_BRIDGE_AUTH_WEB_ACCESSIBLE the private bridge must not declare web-accessible resources'
    }
}

if (-not (Test-Path -LiteralPath $TokenPath)) {
    $random = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $bytes = [byte[]]::new(32)
        $random.GetBytes($bytes)
        $candidate = -join ($bytes | ForEach-Object { $_.ToString('x2') })
    } finally {
        $random.Dispose()
    }
    try {
        Write-PrivateAtomicFile -Path $TokenPath -Content $candidate -Replace $false
    } catch [System.IO.IOException] {
        # A concurrent installer may have atomically created the credential.
        # Read and validate that winner below; all other I/O failures propagate.
        if (-not (Test-Path -LiteralPath $TokenPath -PathType Leaf)) { throw }
    }
}
Set-PrivateFilePermissions -Path $TokenPath
$registrationToken = [System.IO.File]::ReadAllText($TokenPath).Trim()
if ($registrationToken -cnotmatch '\A[0-9a-f]{64}\z') {
    throw 'SYNAPSE_CHROME_BRIDGE_AUTH_INVALID existing registration credential must be exactly 64 lowercase hexadecimal characters; it was not replaced'
}
if ($TokenOnly) {
    [pscustomobject]@{
        token_path = $TokenPath
        token_only = $true
        provisioned = $true
    }
    return
}
$configPath = Join-Path $ExtensionDir 'bridge-auth.json'
$config = ConvertTo-Json -Compress -InputObject @{ registration_token = $registrationToken }
Write-PrivateAtomicFile -Path $configPath -Content $config -Replace $true
Assert-PrivateFilePermissions -Path $configPath
try {
    $readback = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
} catch {
    throw 'SYNAPSE_CHROME_BRIDGE_AUTH_READBACK_FAILED private configuration could not be read as JSON'
}
if ($readback.registration_token -cne $registrationToken) {
    throw 'SYNAPSE_CHROME_BRIDGE_AUTH_READBACK_FAILED'
}
[pscustomobject]@{
    token_path = $TokenPath
    extension_config_path = $configPath
    provisioned = $true
    readback_matches = $true
}
