$ErrorActionPreference = 'Stop'

# Pester 3.4. Real entry points with environment roots confined to TestDrive.
Describe 'npm and Codex cleanup busy guards (#116 variants)' {
    BeforeAll {
        $script:scripts = Split-Path -Parent $PSScriptRoot
        $script:savedLocal = $env:LOCALAPPDATA
        $script:savedProfile = $env:USERPROFILE
    }
    AfterAll {
        $env:LOCALAPPDATA = $script:savedLocal
        $env:USERPROFILE = $script:savedProfile
    }
    BeforeEach {
        $root = Join-Path $TestDrive ([guid]::NewGuid().ToString('N'))
        $env:LOCALAPPDATA = Join-Path $root 'local'
        $env:USERPROFILE = Join-Path $root 'user'
        $script:files = @()
        foreach ($rel in @('local/npm-cache', 'user/npm-cache', 'user/.codex/.tmp')) {
            $dir = Join-Path $root $rel
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            $file = Join-Path $dir 'sentinel.bin'
            [IO.File]::WriteAllText($file, 'preserve while busy')
            (Get-Item -LiteralPath $file).LastWriteTime = (Get-Date).AddDays(-2)
            [IO.File]::ReadAllText($file) | Should Be 'preserve while busy'
            $script:files += $file
        }
    }
    It 'must_pass_npm_idle_apply_deletes_both_cache_contents' {
        Mock Get-Process { @() }
        $result = (& (Join-Path $script:scripts 'cleanup-npm-caches.ps1') -Apply | Out-String) | ConvertFrom-Json
        $result.Mode | Should Be 'Contents'
        @($result.Deleted).Count | Should Be 2
        Test-Path -LiteralPath $script:files[0] | Should Be $false
        Test-Path -LiteralPath $script:files[1] | Should Be $false
        [IO.File]::ReadAllText($script:files[2]) | Should Be 'preserve while busy'
    }
    It 'must_fail_npm_busy_node_before_any_target_is_deleted' {
        Mock Get-Process { @() }
        Mock Get-Process { [pscustomobject]@{ ProcessName = 'node'; Id = 116 } } -ParameterFilter { $Name -contains 'node' }
        $message = $null
        try { & (Join-Path $script:scripts 'cleanup-npm-caches.ps1') -Apply | Out-Null } catch { $message = $_.Exception.Message }
        $message | Should Be 'Refusing cleanup while process is running: node'
        foreach ($file in $script:files) { [IO.File]::ReadAllText($file) | Should Be 'preserve while busy' }
    }
    It 'must_pass_codex_idle_apply_deletes_only_old_files' {
        Mock Get-Process { @() }
        $fresh = Join-Path (Split-Path -Parent $script:files[2]) 'fresh.bin'
        [IO.File]::WriteAllText($fresh, 'fresh')
        $result = (& (Join-Path $script:scripts 'cleanup-codex-tmp.ps1') -Apply | Out-String) | ConvertFrom-Json
        $result.Mode | Should Be 'OldFiles'
        @($result.Deleted).Count | Should Be 1
        Test-Path -LiteralPath $script:files[2] | Should Be $false
        [IO.File]::ReadAllText($fresh) | Should Be 'fresh'
    }
    It 'must_fail_codex_busy_before_old_file_is_deleted' {
        Mock Get-Process { @() }
        Mock Get-Process { [pscustomobject]@{ ProcessName = 'codex'; Id = 117 } } -ParameterFilter { $Name -contains 'codex' }
        $message = $null
        try { & (Join-Path $script:scripts 'cleanup-codex-tmp.ps1') -Apply | Out-Null } catch { $message = $_.Exception.Message }
        $message | Should Be 'Refusing cleanup while process is running: codex'
        foreach ($file in $script:files) { [IO.File]::ReadAllText($file) | Should Be 'preserve while busy' }
    }
    It 'must_pass_busy_dry_runs_preserve_all_targets' {
        Mock Get-Process { [pscustomobject]@{ ProcessName = 'node'; Id = 116 } }
        foreach ($cleaner in @('cleanup-npm-caches.ps1', 'cleanup-codex-tmp.ps1')) {
            $result = (& (Join-Path $script:scripts $cleaner) | Out-String) | ConvertFrom-Json
            $result.Apply | Should Be $false
            @($result.Deleted).Count | Should Be 0
        }
        foreach ($file in $script:files) { [IO.File]::ReadAllText($file) | Should Be 'preserve while busy' }
    }
}
