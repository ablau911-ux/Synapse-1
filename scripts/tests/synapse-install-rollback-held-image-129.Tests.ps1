$ErrorActionPreference = 'Stop'

# #129 re-review: after a failed publication Install-SynapseBinaryOverHeldImage puts the
# connector-held image back at the canonical path; rollback must not try to overwrite
# that mapped image (sharing violation -> Die before the daemon restarts).
Describe 'Synapse install rollback with a connector-held image' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @(
                'Get-SynapseFileSha256', 'Test-SynapseSharingViolation', 'Get-SynapseRetryDelayMs',
                'Copy-SynapseFileWithSharingRetry', 'Move-SynapseFileWithSharingRetry',
                'Install-SynapseBinaryOverHeldImage', 'Restore-SynapseBinaryBackupAfterInstallFailure')) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "setup function is missing: $name" }
            Invoke-Expression $functionAst.Extent.Text
        }
        $script:messages = @()
        $script:startedTasks = @()
        function Info([string]$Message) { $script:messages += $Message }
        function Die($m) { throw $m }
        function Get-ScheduledTask { param($TaskName, $ErrorAction) return [pscustomobject]@{ TaskName = $TaskName } }
        function Start-ScheduledTask { param($TaskName) $script:startedTasks += $TaskName }

        # Open like a mapped image: readers allowed, writes denied, rename/delete allowed.
        function Hold-Image([string]$Path) {
            return [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read,
                ([System.IO.FileShare]::Read -bor [System.IO.FileShare]::Delete))
        }
    }

    BeforeEach {
        $script:messages = @()
        $script:startedTasks = @()
    }

    It 'skips the overwrite when the held destination already holds the verified backup bytes, and restarts the daemon' {
        $exe = Join-Path $TestDrive 'a-synapse-mcp.exe'
        $backup = "$exe.bak"
        [System.IO.File]::WriteAllText($exe, 'old-bytes')
        [System.IO.File]::WriteAllText($backup, 'old-bytes')
        $expected = Get-SynapseFileSha256 -Path $backup
        $held = Hold-Image $exe
        try {
            $hash = Restore-SynapseBinaryBackupAfterInstallFailure -BackupPath $backup -ExePath $exe `
                -ExpectedHash $expected -TaskName 'SynapseTest' -InstallError 'publish failed'
        } finally { $held.Dispose() }

        $hash | Should Be $expected
        ($script:startedTasks -join ',') | Should Be 'SynapseTest'
        ($script:messages -join "`n") | Should Match 'SYNAPSE_BINARY_ROLLBACK_ALREADY_RESTORED'
    }

    It 'restores a differing destination through rename when the image is held, then restarts the daemon' {
        $exe = Join-Path $TestDrive 'b-synapse-mcp.exe'
        $backup = "$exe.bak"
        [System.IO.File]::WriteAllText($exe, 'half-published-or-new')
        [System.IO.File]::WriteAllText($backup, 'old-bytes')
        $expected = Get-SynapseFileSha256 -Path $backup
        $held = Hold-Image $exe
        try {
            $hash = Restore-SynapseBinaryBackupAfterInstallFailure -BackupPath $backup -ExePath $exe `
                -ExpectedHash $expected -TaskName 'SynapseTest' -InstallError 'publish failed'
        } finally { $held.Dispose() }

        $hash | Should Be $expected
        [System.IO.File]::ReadAllText($exe) | Should Be 'old-bytes'
        ($script:startedTasks -join ',') | Should Be 'SynapseTest'
    }

    It 'restores when the destination is missing' {
        $exe = Join-Path $TestDrive 'c-synapse-mcp.exe'
        $backup = "$exe.bak"
        [System.IO.File]::WriteAllText($backup, 'old-bytes')
        $expected = Get-SynapseFileSha256 -Path $backup

        $hash = Restore-SynapseBinaryBackupAfterInstallFailure -BackupPath $backup -ExePath $exe `
            -ExpectedHash $expected -TaskName 'SynapseTest' -InstallError 'publish failed'

        $hash | Should Be $expected
        [System.IO.File]::ReadAllText($exe) | Should Be 'old-bytes'
    }
}
