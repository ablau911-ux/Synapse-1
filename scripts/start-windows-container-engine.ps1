<#
.SYNOPSIS
Start a dedicated Windows Docker engine without switching Docker Desktop.
.DESCRIPTION
Run elevated. Uses a separate named pipe and data directory so the existing
Docker Desktop Linux engine and its containers can remain running.
#>
[CmdletBinding()]
param(
    [string]$EngineRoot = 'C:\ProgramData\SynapseWindowsDocker',
    [string]$DockerDaemon = 'C:\Program Files\Docker\Docker\resources\dockerd.exe'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$principal = [Security.Principal.WindowsPrincipal]::new(
    [Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Starting the Windows container engine requires elevation.'
}
if (-not (Test-Path -LiteralPath $DockerDaemon -PathType Leaf)) {
    throw "Docker daemon executable is missing: $DockerDaemon"
}
foreach ($featureName in @('Containers', 'Microsoft-Hyper-V-All')) {
    $feature = Get-WindowsOptionalFeature -Online -FeatureName $featureName
    if ($feature.State.ToString() -ne 'Enabled') {
        throw "Windows feature $featureName is $($feature.State); finish servicing and reboot first."
    }
}

$root = [IO.Path]::GetFullPath($EngineRoot)
$serviceName = 'synapse-windows-docker'
$endpoint = 'npipe:////./pipe/synapse_windows_engine'
$configPath = Join-Path $root 'daemon.json'
$config = [ordered]@{
    hosts = @($endpoint)
    'data-root' = Join-Path $root 'data'
    'exec-root' = Join-Path $root 'exec'
    pidfile = Join-Path $root 'dockerd.pid'
    group = 'docker-users'
}
New-Item -ItemType Directory -Path $root -Force | Out-Null
if (Test-Path -LiteralPath $configPath) {
    $existing = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    foreach ($key in $config.Keys) {
        if (-not ($existing.PSObject.Properties.Name -contains $key) -or
            (@($existing.$key) -join '|') -cne (@($config[$key]) -join '|')) {
            throw "Existing engine configuration differs at $key; refusing to overwrite $configPath"
        }
    }
    if (@($existing.PSObject.Properties).Count -ne $config.Count) {
        throw "Existing engine configuration contains additional settings: $configPath"
    }
} else {
    $config | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $configPath -Encoding Ascii
}
& $DockerDaemon --validate --config-file $configPath
if ($LASTEXITCODE -ne 0) { throw "Docker rejected configuration: $configPath" }

$service = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($null -eq $service) {
    & $DockerDaemon --register-service --service-name $serviceName --config-file $configPath
    if ($LASTEXITCODE -ne 0) { throw 'Windows Docker service registration failed.' }
} else {
    $serviceInfo = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
    if (-not $serviceInfo.PathName.Contains($configPath) -or
        -not $serviceInfo.PathName.Contains($DockerDaemon)) {
        throw "Existing service points to an unexpected executable/configuration: $serviceName"
    }
}
Start-Service -Name $serviceName
$serviceInfo = Get-CimInstance Win32_Service -Filter "Name='$serviceName'"
[ordered]@{
    recorded_at = [DateTimeOffset]::UtcNow.ToString('o')
    service = $serviceName
    state = $serviceInfo.State
    pid = $serviceInfo.ProcessId
    endpoint = $endpoint
    configuration = $configPath
    data_root = $config['data-root']
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'startup.json') -Encoding Ascii
Write-Output "Windows engine service started. Independently inspect: $endpoint"
