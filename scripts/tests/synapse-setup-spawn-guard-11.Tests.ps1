$ErrorActionPreference = 'Stop'

Describe 'Synapse setup maintenance lock spawn guard (#11)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $script:setupText = Get-Content -LiteralPath $setupPath -Raw
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $setupPath,
            [ref]$tokens,
            [ref]$errors
        )
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        foreach ($name in @('New-SynapseSetupSpawnGuardMutex', 'Enter-SynapseSetupSpawnGuard', 'Exit-SynapseSetupSpawnGuard', 'Get-SynapseProcessStartUnix')) {
            $functionAst = $ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                    $node.Name -eq $name
                }, $true)
            if ($null -eq $functionAst) { throw "setup function is missing: $name" }
            . ([scriptblock]::Create($functionAst.Extent.Text))
        }
        function Die($m) { throw $m }
        $script:SynapseSetupSpawnGuardName = "Local\SynapseSetupSpawnGuardTest-$PID"
    }

    It 'records the owner process start time as unix seconds' {
        $start = Get-SynapseProcessStartUnix -ProcessId $PID
        $expected = [DateTimeOffset]::new((Get-Process -Id $PID).StartTime).ToUnixTimeSeconds()
        [math]::Abs($start - $expected) | Should BeLessThan 3
    }

    It 'guard excludes a second holder until released' {
        $held = Enter-SynapseSetupSpawnGuard -TimeoutMs 1000
        try {
            $probe = New-Object System.Threading.Mutex($false, $script:SynapseSetupSpawnGuardName)
            # same thread re-enters a Mutex, so probe from another runspace
            $ps = [powershell]::Create()
            $null = $ps.AddScript({
                    param($name)
                    $m = New-Object System.Threading.Mutex($false, $name)
                    try { $m.WaitOne(200) } finally { $m.Dispose() }
                }).AddArgument($script:SynapseSetupSpawnGuardName)
            $acquiredElsewhere = $ps.Invoke()[0]
            $ps.Dispose()
            $probe.Dispose()
            $acquiredElsewhere | Should Be $false
        } finally {
            Exit-SynapseSetupSpawnGuard -Mutex $held
        }
        $ps2 = [powershell]::Create()
        $null = $ps2.AddScript({
                param($name)
                $m = New-Object System.Threading.Mutex($false, $name)
                try { $got = $m.WaitOne(1000); if ($got) { $m.ReleaseMutex() }; $got } finally { $m.Dispose() }
            }).AddArgument($script:SynapseSetupSpawnGuardName)
        $ps2.Invoke()[0] | Should Be $true
        $ps2.Dispose()
    }

    It 'lock acquire takes the guard before opening the lock file and writes the start time' {
        $acquire = [regex]::Match($script:setupText, '(?s)function Acquire-SynapseSetupMaintenanceLock \{.*?\r?\n\}\r?\n')
        $acquire.Success | Should Be $true
        $body = $acquire.Value
        $body.IndexOf('Enter-SynapseSetupSpawnGuard') | Should BeLessThan $body.IndexOf('[System.IO.File]::Open')
        $body | Should Match 'Exit-SynapseSetupSpawnGuard'
        $body | Should Match 'pid_start_unix'
    }
}
