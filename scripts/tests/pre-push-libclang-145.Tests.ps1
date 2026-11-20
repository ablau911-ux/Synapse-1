$ErrorActionPreference = 'Stop'

# #145 raid tests (Pester 3.4.0). The REAL .githooks/pre-push runs under Git Bash in a $TestDrive
# fixture repo, fed the same stdin git gives a hook. A fake `cargo` first on PATH records the
# environment the clippy gate would get (no real build). The hook must expose a valid libclang
# directory (LIBCLANG_PATH + PATH head) to cargo, accept a .dll path, ignore a directory without
# the DLL, and skip cargo entirely for pushes without Rust/Cargo changes.
Describe 'pre-push hook exposes libclang to the clippy gate (#145)' {
    BeforeAll {
        $script:repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        $script:hook = Join-Path $script:repoRoot '.githooks\pre-push'
        $script:bash = Join-Path $env:ProgramFiles 'Git\bin\bash.exe'
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $script:savedLibclang = $env:LIBCLANG_PATH
        $env:GIT_CEILING_DIRECTORIES = $TestDrive

        function Invoke-Git {
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $out = & git @args 2>&1; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $old }
            if ($code -ne 0) { throw "git $args failed ($code)" }
            return $out
        }
        $script:repo = Join-Path $TestDrive 'hookrepo'
        New-Item -ItemType Directory -Force -Path $script:repo | Out-Null
        Invoke-Git -C $script:repo init -q -b main | Out-Null
        Invoke-Git -C $script:repo config user.email t@example.invalid | Out-Null
        Invoke-Git -C $script:repo config user.name t | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $script:repo 'README.md'), 'r')
        Invoke-Git -C $script:repo add -A | Out-Null
        Invoke-Git -C $script:repo commit -q -m docs | Out-Null
        $script:shaDocs = ([string](Invoke-Git -C $script:repo rev-parse HEAD)).Trim()
        [System.IO.File]::WriteAllText((Join-Path $script:repo 'README.md'), 'r2')
        Invoke-Git -C $script:repo commit -q -am docs2 | Out-Null
        $script:shaDocs2 = ([string](Invoke-Git -C $script:repo rev-parse HEAD)).Trim()
        [System.IO.File]::WriteAllText((Join-Path $script:repo 'a.rs'), 'fn main() {}')
        Invoke-Git -C $script:repo add -A | Out-Null
        Invoke-Git -C $script:repo commit -q -m rust | Out-Null
        $script:shaRust = ([string](Invoke-Git -C $script:repo rev-parse HEAD)).Trim()

        $script:fakeBin = Join-Path $TestDrive 'fakebin'
        New-Item -ItemType Directory -Force -Path $script:fakeBin | Out-Null
        $cargo = "#!/bin/sh`nout=`$(cygpath -u `"`$RAID_CARGO_PROBE`")`nprintf 'LIBCLANG_PATH=%s\n' `"`$LIBCLANG_PATH`" > `"`$out`"`nprintf 'PATH_HEAD=%s\n' `"`${PATH%%:*}`" >> `"`$out`"`nexit 0`n"
        [System.IO.File]::WriteAllText((Join-Path $script:fakeBin 'cargo'), $cargo)

        # Runner script avoids passing embedded quotes through PowerShell's native-argument quoting.
        $script:runner = (Join-Path $TestDrive 'run-hook.sh') -replace '\\', '/'
        $runnerText = "cd `"`$(cygpath -u `"`$1`")`" || exit 90`nPATH=`"`$(cygpath -u `"`$2`"):`$PATH`"`nexport PATH`nexec sh `"`$(cygpath -u `"`$3`")`"`n"
        [System.IO.File]::WriteAllText((Join-Path $TestDrive 'run-hook.sh'), $runnerText)

        $script:llvm = Join-Path $TestDrive 'llvm\bin'
        New-Item -ItemType Directory -Force -Path $script:llvm | Out-Null
        [System.IO.File]::WriteAllBytes((Join-Path $script:llvm 'libclang.dll'), [byte[]](0, 1, 2))
        $script:emptyDir = Join-Path $TestDrive 'no-dll'
        New-Item -ItemType Directory -Force -Path $script:emptyDir | Out-Null

        function Invoke-Hook([string]$Case, [string]$Libclang, [string]$From, [string]$To) {
            $probe = Join-Path $TestDrive "$Case-cargo-probe.txt"
            $env:RAID_CARGO_PROBE = $probe
            $env:LIBCLANG_PATH = $Libclang
            $stdin = "refs/heads/main $To refs/heads/main $From`n"
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $out = $stdin | & $script:bash $script:runner ($script:repo -replace '\\', '/') ($script:fakeBin -replace '\\', '/') ($script:hook -replace '\\', '/') 2>&1 | Out-String; $code = $LASTEXITCODE }
            finally { $ErrorActionPreference = $old }
            $probeText = if (Test-Path -LiteralPath $probe) { [System.IO.File]::ReadAllText($probe) } else { $null }
            return [pscustomobject]@{ Out = $out; Code = $code; Probe = $probeText }
        }
        function Get-ProbeValue([string]$Probe, [string]$Key) {
            return ([regex]::Match($Probe, "(?m)^$Key=(.*)$").Groups[1].Value.Trim())
        }
    }

    AfterAll {
        $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling
        $env:LIBCLANG_PATH = $script:savedLibclang
        $env:RAID_CARGO_PROBE = $null
    }

    It 'must_pass a configured libclang directory reaches cargo as LIBCLANG_PATH and PATH head' {
        if (-not (Test-Path -LiteralPath $script:bash)) { Set-TestInconclusive 'Git Bash absent' }
        $r = Invoke-Hook -Case 'dir' -Libclang $script:llvm -From $script:shaDocs2 -To $script:shaRust

        $r.Code | Should Be 0
        $null -eq $r.Probe | Should Be $false
        (Get-ProbeValue $r.Probe 'LIBCLANG_PATH').TrimEnd('\') | Should Be $script:llvm.TrimEnd('\')
        (Get-ProbeValue $r.Probe 'PATH_HEAD') | Should Match '/llvm/bin$'
    }

    It 'must_pass a LIBCLANG_PATH naming the .dll itself resolves to its directory' {
        if (-not (Test-Path -LiteralPath $script:bash)) { Set-TestInconclusive 'Git Bash absent' }
        $r = Invoke-Hook -Case 'dll' -Libclang (Join-Path $script:llvm 'libclang.dll') -From $script:shaDocs2 -To $script:shaRust

        $r.Code | Should Be 0
        (Get-ProbeValue $r.Probe 'LIBCLANG_PATH').TrimEnd('\') | Should Be $script:llvm.TrimEnd('\')
    }

    It 'must_fail a LIBCLANG_PATH without libclang.dll is warned about and never exported' {
        if (-not (Test-Path -LiteralPath $script:bash)) { Set-TestInconclusive 'Git Bash absent' }
        $r = Invoke-Hook -Case 'nodll' -Libclang $script:emptyDir -From $script:shaDocs2 -To $script:shaRust

        $r.Out | Should Match "LIBCLANG_PATH='.*' has no libclang\.dll"
        (Get-ProbeValue $r.Probe 'LIBCLANG_PATH').TrimEnd('\') | Should Not Be $script:emptyDir.TrimEnd('\')
    }

    It 'must_fail a push without Rust or Cargo changes never launches cargo' {
        if (-not (Test-Path -LiteralPath $script:bash)) { Set-TestInconclusive 'Git Bash absent' }
        $r = Invoke-Hook -Case 'docs' -Libclang $script:llvm -From $script:shaDocs -To $script:shaDocs2

        $r.Code | Should Be 0
        $r.Out | Should Match 'no Rust/Cargo changes in this push'
        $null -eq $r.Probe | Should Be $true
    }
}
