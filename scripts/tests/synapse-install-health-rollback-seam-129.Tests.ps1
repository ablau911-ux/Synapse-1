$ErrorActionPreference = 'Stop'

# #129 raid tests (Pester 3.4.0). The health-failure rollback (installed candidate never became
# healthy) lives inline in setup's top-level script, so only a source scan covered it.
# NEEDS-SEAM: the fixer extracts that branch into
#   Invoke-SynapseInstallHealthRollback -BackupPath <string> -ExePath <string> -ExpectedHash <string>
#       -TaskName <string> -FailureDetail <string> -Bind <string> -DbPath <string> -TokenPath <string>
#       -Token <string> [-HealthTimeoutSeconds <int> = 600]
#   -> [pscustomobject]@{ Ok = [bool]; RollbackHash = [string]; Pid = [int] or $null }
# and the top-level branch calls it. Tests drive it with a connector-held (mapped-like) image.
# Process, scheduler and HTTP dependencies are stubbed; files live in $TestDrive.
Describe 'Synapse install health-failure rollback through the held-image path (#129)' {
    BeforeAll {
        $script:setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $script:seamFound = $false
        foreach ($name in @('Get-SynapseFileSha256', 'Test-SynapseSharingViolation', 'Get-SynapseRetryDelayMs',
                'Copy-SynapseFileWithSharingRetry', 'Move-SynapseFileWithSharingRetry', 'Install-SynapseBinaryOverHeldImage',
                'Restore-SynapseBinaryBackupAfterInstallFailure', 'Invoke-SynapseInstallHealthRollback')) {
            $fn = $script:ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
                }, $true)
            if ($null -ne $fn) { Invoke-Expression $fn.Extent.Text; if ($name -eq 'Invoke-SynapseInstallHealthRollback') { $script:seamFound = $true } }
        }
        function Info([string]$Message) { }
        function Die([string]$Message) { throw $Message }
        function Start-Sleep { param([int]$Milliseconds, [int]$Seconds) }
        function Get-ScheduledTask { param($TaskName, $ErrorAction) [pscustomobject]@{ TaskName = $TaskName } }
        function Stop-ScheduledTask { param($TaskName, $ErrorAction) $script:stoppedTasks += $TaskName }
        function Start-ScheduledTask { param($TaskName) $script:startedTasks += $TaskName }
        function Stop-SynapseMcpProcesses { param($Reason, $Bind, $DbPath, $TokenPath, [switch]$ForceRestart, $TimeoutSeconds) $script:stopReasons += $Reason }
        function Invoke-RestMethod { param($Uri, $Headers, $TimeoutSec) [pscustomobject]@{ ok = $true; pid = 777 } }
        function Test-SynapseInstalledHealthIdentity { param($Health, $ExePath, $DbPath) $script:healthIdentityOk }
        function Hold-Image([string]$Path) {
            return [System.IO.File]::Open($Path, 'Open', 'Read', ([System.IO.FileShare]::Read -bor [System.IO.FileShare]::Delete))
        }
        function New-Install([string]$Case) {
            $exe = Join-Path $TestDrive "$Case-synapse-mcp.exe"
            [System.IO.File]::WriteAllText($exe, 'unhealthy-candidate')
            [System.IO.File]::WriteAllText("$exe.bak", 'old-verified-bytes')
            return $exe
        }
        function Invoke-Rollback([string]$Exe, [int]$Timeout) {
            return Invoke-SynapseInstallHealthRollback -BackupPath "$Exe.bak" -ExePath $Exe `
                -ExpectedHash (Get-SynapseFileSha256 -Path "$Exe.bak") -TaskName 'SynapseRaidTest129' `
                -FailureDetail 'SYNAPSE_INSTALL_HEALTH_FAILED test' -Bind '127.0.0.1:1' -DbPath (Join-Path $TestDrive 'db') `
                -TokenPath (Join-Path $TestDrive 'token.txt') -Token 'raid-token' -HealthTimeoutSeconds $Timeout
        }
    }

    BeforeEach {
        $script:startedTasks = @()
        $script:stoppedTasks = @()
        $script:stopReasons = @()
        $script:healthIdentityOk = $true
    }

    It 'must_pass restores the verified backup over a held image, restarts the task and reports the healthy pid' {
        $script:seamFound | Should Be $true
        $exe = New-Install 'pass'
        $expected = Get-SynapseFileSha256 -Path "$exe.bak"
        $held = Hold-Image $exe
        $failure = ''
        try { $result = Invoke-Rollback -Exe $exe -Timeout 5 } catch { $failure = $_.Exception.Message } finally { $held.Dispose() }
        ('#129 held-image rollback error: ' + $failure) | Should Be '#129 held-image rollback error: '

        $result.Ok | Should Be $true
        $result.RollbackHash | Should Be $expected
        $result.Pid | Should Be 777
        [System.IO.File]::ReadAllText($exe) | Should Be 'old-verified-bytes'
        ($script:stopReasons -join ',') | Should Be 'install_health_failed_rollback'
        ($script:startedTasks -join ',') | Should Be 'SynapseRaidTest129'
    }

    It 'must_fail rollback whose daemon never proves identity reports Ok=false but keeps the restored bytes' {
        $script:seamFound | Should Be $true
        $script:healthIdentityOk = $false
        $exe = New-Install 'unhealthy'
        $held = Hold-Image $exe
        $failure = ''
        try { $result = Invoke-Rollback -Exe $exe -Timeout 1 } catch { $failure = $_.Exception.Message } finally { $held.Dispose() }
        ('#129 unhealthy restart must restore held image first: ' + $failure) | Should Be '#129 unhealthy restart must restore held image first: '

        $result.Ok | Should Be $false
        $null -eq $result.Pid | Should Be $true
        [System.IO.File]::ReadAllText($exe) | Should Be 'old-verified-bytes'
        ($script:startedTasks -join ',') | Should Be 'SynapseRaidTest129'
    }

    It 'must_pass the top-level health-failure branch delegates to the seam' {
        $topLevel = {
            param([string]$Name)
            @($script:ast.FindAll({
                        param($n)
                        $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $Name
                    }, $true) | Where-Object {
                    $p = $_.Parent
                    while ($null -ne $p -and $p -isnot [System.Management.Automation.Language.FunctionDefinitionAst]) { $p = $p.Parent }
                    $null -eq $p
                })
        }
        # Health-failure branch -> seam; the install-copy catch keeps its single direct restore call.
        (& $topLevel 'Invoke-SynapseInstallHealthRollback').Count | Should Be 1
        (& $topLevel 'Restore-SynapseBinaryBackupAfterInstallFailure').Count | Should Be 1
    }
}
