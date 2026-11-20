# Shared private credential-file helpers. Dot-source this file.
# Used by provision-synapse-chrome-auth.ps1 (registration token and
# bridge-auth.json) and install-synapse-gecko-bridge.ps1 (managed storage
# manifest). A private file has a protected DACL granting FullControl to
# exactly the current user, LocalSystem and BUILTIN\Administrators.

function Get-SynapsePrivateFileSids {
    @(
        [System.Security.Principal.WindowsIdentity]::GetCurrent().User,
        [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18'),
        [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
    )
}

function New-PrivateFileSecurity {
    $security = [System.Security.AccessControl.FileSecurity]::new()
    $security.SetAccessRuleProtection($true, $false)
    foreach ($sid in @(Get-SynapsePrivateFileSids)) {
        $security.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
            $sid, [System.Security.AccessControl.FileSystemRights]::FullControl,
            [System.Security.AccessControl.AccessControlType]::Allow
        ))
    }
    return $security
}

function Assert-PrivateFilePermissions {
    param([string]$Path)

    $attributes = [System.IO.File]::GetAttributes($Path)
    if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        throw 'SYNAPSE_CHROME_BRIDGE_AUTH_UNSAFE_FILE credential files must be ordinary files'
    }
    $security = Get-Acl -LiteralPath $Path
    $rules = @($security.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
    $expectedSidValues = @(Get-SynapsePrivateFileSids | ForEach-Object { $_.Value })
    if (-not $security.AreAccessRulesProtected -or $rules.Count -ne $expectedSidValues.Count) {
        throw 'SYNAPSE_CHROME_BRIDGE_AUTH_PERMISSIONS_INVALID credential file DACL must be private'
    }
    foreach ($rule in $rules) {
        if ($rule.IsInherited -or $rule.IdentityReference.Value -notin $expectedSidValues -or
            $rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or
            $rule.FileSystemRights -ne [System.Security.AccessControl.FileSystemRights]::FullControl) {
            throw 'SYNAPSE_CHROME_BRIDGE_AUTH_PERMISSIONS_INVALID credential file DACL must be private'
        }
    }
    foreach ($sidValue in $expectedSidValues) {
        if (@($rules | Where-Object { $_.IdentityReference.Value -eq $sidValue }).Count -ne 1) {
            throw 'SYNAPSE_CHROME_BRIDGE_AUTH_PERMISSIONS_INVALID credential file DACL must include only the expected principals'
        }
    }
}

function Set-PrivateFilePermissions {
    param([string]$Path)

    $attributes = [System.IO.File]::GetAttributes($Path)
    if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        throw 'SYNAPSE_CHROME_BRIDGE_AUTH_UNSAFE_FILE credential files must be ordinary files'
    }
    $file = [System.IO.FileInfo]::new($Path)
    $security = New-PrivateFileSecurity
    # Persist the modified DACL only. PowerShell Set-Acl may also attempt SACL
    # updates, requiring SeSecurityPrivilege that normal installations lack.
    if ($PSEdition -eq 'Core') {
        [System.IO.FileSystemAclExtensions]::SetAccessControl($file, $security)
    } else {
        $file.SetAccessControl($security)
    }
    Assert-PrivateFilePermissions -Path $Path
}

function Write-PrivateAtomicFile {
    param([string]$Path, [string]$Content, [bool]$Replace)

    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $temporaryPath = Join-Path $parent ([System.IO.Path]::GetRandomFileName())
    try {
        $security = New-PrivateFileSecurity
        # The DACL is present at creation, before any credential bytes exist.
        if ($PSEdition -eq 'Core') {
            $stream = [System.IO.FileSystemAclExtensions]::Create(
                [System.IO.FileInfo]::new($temporaryPath), [System.IO.FileMode]::CreateNew,
                [System.Security.AccessControl.FileSystemRights]::Write,
                [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::None, $security
            )
        } else {
            $stream = [System.IO.FileStream]::new(
                $temporaryPath, [System.IO.FileMode]::CreateNew,
                [System.Security.AccessControl.FileSystemRights]::Write,
                [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::None, $security
            )
        }
        try {
            $contentBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($Content)
            $stream.Write($contentBytes, 0, $contentBytes.Length)
            $stream.Flush($true)
        } finally {
            $stream.Dispose()
        }
        Assert-PrivateFilePermissions -Path $temporaryPath
        if ($Replace -and [System.IO.File]::Exists($Path)) {
            # ReplaceFile preserves the destination DACL, not the temp DACL.
            Set-PrivateFilePermissions -Path $Path
            [System.IO.File]::Replace($temporaryPath, $Path, [NullString]::Value)
        } else {
            try {
                [System.IO.File]::Move($temporaryPath, $Path)
            } catch [System.IO.IOException] {
                if (-not $Replace -or -not [System.IO.File]::Exists($Path)) { throw }
                # A concurrent first-time provisioner created the config after
                # our existence check. Replace its complete file atomically.
                Set-PrivateFilePermissions -Path $Path
                [System.IO.File]::Replace($temporaryPath, $Path, [NullString]::Value)
            }
        }
        Assert-PrivateFilePermissions -Path $Path
    } finally {
        if (Test-Path -LiteralPath $temporaryPath) {
            Remove-Item -LiteralPath $temporaryPath -Force
        }
    }
}
