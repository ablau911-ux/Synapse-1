$ErrorActionPreference = 'Stop'

# #63 raid tests (Pester 3.4.0).
# 1) REAL setup entry point (child process, all paths in $TestDrive, fake -SourceDir): a missing
#    libclang must stop setup with SYNAPSE_NATIVE_LIBCLANG_MISSING before Cargo is launched.
#    SAFETY INTERLOCK: the child only runs if the AST shows the native preflight precedes every
#    Cargo launch and the token/data-dir step; the fake Cargo.toml is not valid TOML.
# 2) A loadable libclang must load on BOTH supported hosts. setup supports Windows PowerShell
#    5.1 (see the PSVersion branch in New-SynapseSetupSpawnGuardMutex), where .NET Framework has
#    no System.Runtime.InteropServices.NativeLibrary -> every 5.1 build dies LOAD_FAILED (variant).
Describe 'Synapse setup native build preflight gate (#63)' {
    BeforeAll {
        $script:setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $script:hostExe = (Get-Process -Id $PID).Path
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $fn = $script:ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Initialize-SynapseNativeBuildEnvironment'
            }, $true)
        if ($null -eq $fn) { throw 'setup function is missing: Initialize-SynapseNativeBuildEnvironment' }
        Invoke-Expression $fn.Extent.Text
        function Die([string]$Message) { throw $Message }
        function Info([string]$Message) { }

        function Get-TopLevelCommandOffsets([string]$Name) {
            return @($script:ast.FindAll({
                        param($n)
                        $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq $Name
                    }, $true) | Where-Object {
                    $p = $_.Parent
                    while ($null -ne $p -and $p -isnot [System.Management.Automation.Language.FunctionDefinitionAst]) { $p = $p.Parent }
                    $null -eq $p
                } | ForEach-Object { $_.Extent.StartOffset })
        }
        function Get-RealLibclangDir {
            $candidates = @($env:LIBCLANG_PATH) + @(Get-ChildItem -Path "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\*\VC\Tools\Llvm\x64\bin" -Directory -ErrorAction SilentlyContinue | ForEach-Object FullName)
            return ($candidates | Where-Object { $_ -and (Test-Path -LiteralPath (Join-Path $_ 'libclang.dll')) } | Select-Object -First 1)
        }
        function Invoke-InitInHost([string]$HostExe, [string]$Dir) {
            $probe = Join-Path $TestDrive ('probe-' + [guid]::NewGuid().ToString('N') + '.ps1')
            @'
param([string]$SetupPath, [string]$Dir)
$ErrorActionPreference = 'Stop'
$t = $null; $e = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($SetupPath, [ref]$t, [ref]$e)
$fn = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Initialize-SynapseNativeBuildEnvironment' }, $true)
Invoke-Expression $fn.Extent.Text
function Die([string]$Message) { throw $Message }
function Info([string]$Message) { }
try { 'RESULT=' + (Initialize-SynapseNativeBuildEnvironment -LibclangPath $Dir) } catch { 'ERROR=' + $_.Exception.Message }
'@ | Set-Content -LiteralPath $probe -Encoding UTF8
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { return (& $HostExe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $probe -SetupPath $script:setupPath -Dir $Dir 2>&1 | Out-String) }
            finally { $ErrorActionPreference = $old }
        }
        $script:savedPath = $env:Path
        $script:savedLibclang = $env:LIBCLANG_PATH
    }

    AfterEach {
        $env:Path = $script:savedPath
        $env:LIBCLANG_PATH = $script:savedLibclang
    }

    It 'must_fail real setup stops with SYNAPSE_NATIVE_LIBCLANG_MISSING before launching Cargo' {
        $init = Get-TopLevelCommandOffsets 'Initialize-SynapseNativeBuildEnvironment'
        $cargo = Get-TopLevelCommandOffsets 'Invoke-SynapseProcessInKillOnCloseJob'
        $tokenStep = $script:ast.Extent.Text.IndexOf('Step "Bearer token + data dirs"')
        $init.Count | Should Be 1
        ($cargo.Count -ge 1) | Should Be $true
        ($init[0] -lt ($cargo | Measure-Object -Minimum).Minimum) | Should Be $true
        ($tokenStep -gt 0 -and $init[0] -lt $tokenStep) | Should Be $true
        if (-not (Test-Path -LiteralPath "$env:USERPROFILE\.cargo\bin\cargo.exe")) { Set-TestInconclusive 'cargo.exe absent: setup would die before the native preflight' }

        $base = Join-Path $TestDrive 'setup-run'
        New-Item -ItemType Directory -Force -Path (Join-Path $base 'src') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $base 'src\Cargo.toml'), 'this is [not toml')
        $savedManifest = $env:SYNAPSE_SETUP_REPAIR_MANIFEST
        $savedIssue = $env:SYNAPSE_ACTIVE_ISSUE
        $env:SYNAPSE_SETUP_REPAIR_MANIFEST = $null
        $env:SYNAPSE_ACTIVE_ISSUE = $null
        $env:LIBCLANG_PATH = Join-Path $base 'no-libclang-here'
        $old = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            $out = & $script:hostExe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $script:setupPath `
                -SourceDir (Join-Path $base 'src') -MaintenanceLockPath (Join-Path $base 'lock\setup.lock.json') `
                -LogDir (Join-Path $base 'logs') -ExePath (Join-Path $base 'bin\synapse-mcp.exe') `
                -ChromeNativeHostExePath (Join-Path $base 'bin\host.exe') -CargoTarget (Join-Path $base 'target') `
                -DbPath (Join-Path $base 'db') -ProfilesDir (Join-Path $base 'profiles') -TokenPath (Join-Path $base 'appdata\token.txt') `
                -CodexToolSurfaceSnapshotPath (Join-Path $base 'appdata\surface.json') -TaskName 'SynapseRaidTestNoSuchTask63' `
                -Bind '127.0.0.1:1' 2>&1 | Out-String
            $code = $LASTEXITCODE
        } finally {
            $ErrorActionPreference = $old
            $env:SYNAPSE_SETUP_REPAIR_MANIFEST = $savedManifest
            $env:SYNAPSE_ACTIVE_ISSUE = $savedIssue
        }

        $code | Should Not Be 0
        $out | Should Match 'SYNAPSE_NATIVE_LIBCLANG_MISSING'
        Test-Path -LiteralPath (Join-Path $base 'logs\setup-cargo-version.log') | Should Be $false
        Test-Path -LiteralPath (Join-Path $base 'target') | Should Be $false
        Test-Path -LiteralPath (Join-Path $base 'appdata') | Should Be $false
        Test-Path -LiteralPath (Join-Path $base 'db') | Should Be $false
        # Follow the advertised remediation without rerunning setup: a corrected
        # DLL path must work in the same supported Windows PowerShell host.
        $dir = Get-RealLibclangDir
        ($null -ne $dir) | Should Be $true
        $ps51 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $retry = Invoke-InitInHost -HostExe $ps51 -Dir $dir
        $retry | Should Not Match 'ERROR=SYNAPSE_NATIVE_LIBCLANG_LOAD_FAILED'
        $retry | Should Match ([regex]::Escape('RESULT=' + (Join-Path $dir 'libclang.dll')))
    }

    It 'must_fail an unloadable libclang.dll is refused with SYNAPSE_NATIVE_LIBCLANG_LOAD_FAILED' {
        $bad = Join-Path $TestDrive 'bad-llvm'
        New-Item -ItemType Directory -Force -Path $bad | Out-Null
        [System.IO.File]::WriteAllBytes((Join-Path $bad 'libclang.dll'), [byte[]](1..64))

        $message = $null
        try { $null = Initialize-SynapseNativeBuildEnvironment -LibclangPath $bad }
        catch { $message = $_.Exception.Message }
        $message | Should Match '^SYNAPSE_NATIVE_LIBCLANG_LOAD_FAILED'
        # #63: bad native bytes must reach the OS loader on PS 5.1 too. A missing
        # NativeLibrary type is a broken supported host, not evidence of a bad DLL.
        $message | Should Not Match 'Unable to find type|TypeNotFound|Cannot find type'
        # Also exercise the 5.1 compatibility branch when Pester itself runs in 7.
        $ps51 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $out = Invoke-InitInHost -HostExe $ps51 -Dir $bad
        $out | Should Match 'ERROR=SYNAPSE_NATIVE_LIBCLANG_LOAD_FAILED'
        $out | Should Not Match 'Unable to find type|TypeNotFound|Cannot find type'
    }

    It 'must_pass a real libclang loads under Windows PowerShell 5.1 (supported setup host)' {
        $dir = Get-RealLibclangDir
        if (-not $dir) { Set-TestInconclusive 'no libclang.dll on this host' }
        $ps51 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        if (-not (Test-Path -LiteralPath $ps51)) { Set-TestInconclusive 'Windows PowerShell 5.1 absent' }

        $out = Invoke-InitInHost -HostExe $ps51 -Dir $dir

        $out | Should Not Match 'ERROR='
        $out | Should Match ([regex]::Escape('RESULT=' + (Join-Path $dir 'libclang.dll')))
    }

    It 'must_pass a real libclang loads under PowerShell 7' {
        $dir = Get-RealLibclangDir
        if (-not $dir) { Set-TestInconclusive 'no libclang.dll on this host' }
        $pwsh = Get-Command pwsh.exe -ErrorAction SilentlyContinue
        if (-not $pwsh) { Set-TestInconclusive 'pwsh absent' }

        $out = Invoke-InitInHost -HostExe $pwsh.Source -Dir $dir

        $out | Should Match ([regex]::Escape('RESULT=' + (Join-Path $dir 'libclang.dll')))
    }
}
