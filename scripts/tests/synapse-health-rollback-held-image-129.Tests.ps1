$ErrorActionPreference = 'Stop'

# Pester 3.4; definitions only, never execute the setup entry point.
Describe 'Synapse health-failure rollback drives held-image restore (#129)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null; $errors = $null
        $ast = [Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count) { throw ($errors | Out-String) }
        foreach ($name in @('Get-SynapseFileSha256', 'Test-SynapseSharingViolation',
                'Get-SynapseRetryDelayMs', 'Copy-SynapseFileWithSharingRetry',
                'Move-SynapseFileWithSharingRetry', 'Install-SynapseBinaryOverHeldImage',
                'Restore-SynapseBinaryBackupAfterInstallFailure', 'Invoke-SynapseInstallHealthRollback')) {
            $fn = $ast.Find({ param($n)
                $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name
            }, $true)
            if ($null -eq $fn) { throw "Missing production function: $name" }
            Invoke-Expression $fn.Extent.Text
        }
        function Info { param($Message) }
        function Die { param($Message) throw $Message }
        function Get-ScheduledTask { param($TaskName, $ErrorAction) @{ TaskName = $TaskName } }
        function Stop-ScheduledTask { param($TaskName, $ErrorAction) }
        function Start-ScheduledTask { param($TaskName) $script:starts += $TaskName }
        function Stop-SynapseMcpProcesses {
            param($Reason, $Bind, $DbPath, $TokenPath, [switch]$ForceRestart, $TimeoutSeconds)
            $script:reasons += $Reason
        }
        function Start-Sleep { param($Seconds, $Milliseconds) }
        function Get-Date { $script:tick++; ([datetime]'2026-10-15T12:00:00Z').AddSeconds($script:tick) }
        function Invoke-RestMethod { param($Uri, $Headers, $TimeoutSec) @{ pid = 129; ok = $true } }
        function Test-SynapseInstalledHealthIdentity { param($Health, $ExePath, $DbPath) $script:healthy }
        function Invoke-TestRollback($Exe) {
            Invoke-SynapseInstallHealthRollback -BackupPath "$Exe.bak" -ExePath $Exe `
                -ExpectedHash (Get-SynapseFileSha256 -Path "$Exe.bak") -TaskName 'FU4-129' `
                -FailureDetail 'synthetic unhealthy candidate' -Bind '127.0.0.1:1' `
                -DbPath (Join-Path $TestDrive 'db') -TokenPath (Join-Path $TestDrive 'token') `
                -Token 'synthetic' -HealthTimeoutSeconds 4
        }
    }
    BeforeEach {
        $script:tick = 0; $script:starts = @(); $script:reasons = @(); $script:healthy = $true
        $script:exe = Join-Path $TestDrive 'synapse-mcp.exe'
        [IO.File]::WriteAllText($script:exe, 'unhealthy candidate')
        [IO.File]::WriteAllText("$script:exe.bak", 'verified backup')
        [IO.File]::ReadAllText($script:exe) | Should Be 'unhealthy candidate'
        [IO.File]::ReadAllText("$script:exe.bak") | Should Be 'verified backup'
    }
    It 'must_pass_held_image_restored_and_healthy_pid_returned' {
        $hash = Get-SynapseFileSha256 -Path "$script:exe.bak"
        $held = [IO.File]::Open($script:exe, 'Open', 'Read', ([IO.FileShare]::Read -bor [IO.FileShare]::Delete))
        $failure = ''
        try { $result = Invoke-TestRollback $script:exe } catch { $failure = $_.Exception.Message } finally { $held.Dispose() }
        ('#129 held-image rollback error: ' + $failure) | Should Be '#129 held-image rollback error: '
        $result.Ok | Should Be $true
        $result.Pid | Should Be 129
        $result.RollbackHash | Should Be $hash
        [IO.File]::ReadAllText($script:exe) | Should Be 'verified backup'
        ($script:starts -join ',') | Should Be 'FU4-129'
        ($script:reasons -join ',') | Should Be 'install_health_failed_rollback'
    }
    It 'must_fail_unhealthy_restart_without_losing_restored_bytes' {
        $script:healthy = $false
        $held = [IO.File]::Open($script:exe, 'Open', 'Read', ([IO.FileShare]::Read -bor [IO.FileShare]::Delete))
        $failure = ''
        try { $result = Invoke-TestRollback $script:exe } catch { $failure = $_.Exception.Message } finally { $held.Dispose() }
        ('#129 unhealthy restart must restore held image first: ' + $failure) | Should Be '#129 unhealthy restart must restore held image first: '
        $result.Ok | Should Be $false
        ($null -eq $result.Pid) | Should Be $true
        $result.RollbackHash | Should Be (Get-SynapseFileSha256 -Path "$script:exe.bak")
        [IO.File]::ReadAllText($script:exe) | Should Be 'verified backup'
        ($script:starts -join ',') | Should Be 'FU4-129'
    }
}
