$ErrorActionPreference = 'Stop'

# #131 raid tests (Pester 3.4.0). Which bytes setup validates and installs is decided inline at
# top level (-SkipBuild / post-exit / built branches), so tests that compose helpers by hand stay
# green if that wiring is reverted. NEEDS-SEAM: the fixer extracts the branch into
#   Resolve-SynapseInstallSource -ExePath <string> -CargoTarget <string> -CargoTargetExplicit <bool>
#       -LogDir <string> [-SourceDir <string>] [-BuiltPath <string>] [-SkipBuild]
#       [-PostExitUseInstalledBinary] [-PostExitInstalledSha256 <string>]
#   -> [pscustomobject]@{ Path = [string]; Sha256 = [string]; CargoTarget = [string]; HeadIdentity = $obj-or-$null }
# and top level uses only its result. All files and git fixtures live in $TestDrive.
Describe 'Synapse install-source selection seam (#131)' {
    BeforeAll {
        $script:setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($script:setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $script:seamFound = $false
        foreach ($name in @('Get-SynapseFileSha256', 'Get-SynapseCheckoutCargoTarget', 'Get-SynapseCargoReleaseArtifactPath',
                'Resolve-SynapseSkipBuildCandidatePath', 'Get-SynapseSkipBuildHeadIdentity', 'New-SynapseSetupRunDirectory',
                'Register-SynapseSetupOwnedDirectory', 'New-SynapseStagedDaemonBinary', 'Resolve-SynapseInstallSource')) {
            $fn = $script:ast.Find({
                    param($node)
                    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
                }, $true)
            if ($null -ne $fn) { Invoke-Expression $fn.Extent.Text; if ($name -eq 'Resolve-SynapseInstallSource') { $script:seamFound = $true } }
        }
        function Info([string]$Message) { }
        function Die([string]$Message) { throw $Message }
        $script:SynapseSetupOwnedDirectories = @()
        $script:savedCeiling = $env:GIT_CEILING_DIRECTORIES
        $env:GIT_CEILING_DIRECTORIES = $TestDrive

        function Invoke-Git {
            $old = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try { $null = & git @args 2>&1; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $old }
            if ($code -ne 0) { throw "git $args failed ($code)" }
        }
        function New-Checkout([string]$Case) {
            $src = Join-Path $TestDrive "$Case\src"
            New-Item -ItemType Directory -Force -Path $src | Out-Null
            Invoke-Git -C $src init -q -b main
            Invoke-Git -C $src config user.email t@example.invalid
            Invoke-Git -C $src config user.name t
            [System.IO.File]::WriteAllText((Join-Path $src 'a.txt'), 'a')
            Invoke-Git -C $src add -A
            Invoke-Git -C $src commit -q -m init
            return $src
        }
        function New-Installed([string]$Case) {
            $exe = Join-Path $TestDrive "$Case\bin\synapse-mcp.exe"
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $exe) | Out-Null
            [System.IO.File]::WriteAllText($exe, 'installed-old')
            return $exe
        }
        function Get-ThrownMessage([scriptblock]$Block) {
            try { & $Block; return $null } catch { return $_.Exception.Message }
        }
        $script:savedBuildTarget = $env:CARGO_BUILD_TARGET
        $env:CARGO_BUILD_TARGET = $null
    }

    AfterAll {
        $env:GIT_CEILING_DIRECTORIES = $script:savedCeiling
        $env:CARGO_BUILD_TARGET = $script:savedBuildTarget
    }

    It 'must_pass -SkipBuild selects the CargoTarget release artifact, not the installed destination' {
        $script:seamFound | Should Be $true
        $src = New-Checkout 'skip'
        $exe = New-Installed 'skip'
        $target = Join-Path $TestDrive 'skip\target'
        $artifact = Join-Path $target 'release\synapse-mcp.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $artifact) | Out-Null
        [System.IO.File]::WriteAllText($artifact, 'fresh-build')

        $r = Resolve-SynapseInstallSource -ExePath $exe -CargoTarget $target -CargoTargetExplicit $true `
            -LogDir (Join-Path $TestDrive 'skip\logs') -SourceDir $src -SkipBuild

        $r.Path | Should Be ([System.IO.Path]::GetFullPath($artifact))
        $r.Sha256 | Should Be (Get-SynapseFileSha256 -Path $artifact)
        ($r.Sha256 -ne (Get-SynapseFileSha256 -Path $exe)) | Should Be $true
        [System.IO.File]::ReadAllText($exe) | Should Be 'installed-old'
    }

    It 'must_fail -SkipBuild with no release artifact refuses and never falls back to the destination' {
        $script:seamFound | Should Be $true
        $src = New-Checkout 'missing'
        $exe = New-Installed 'missing'
        $target = Join-Path $TestDrive 'missing\target'
        New-Item -ItemType Directory -Force -Path $target | Out-Null

        $msg = Get-ThrownMessage { Resolve-SynapseInstallSource -ExePath $exe -CargoTarget $target -CargoTargetExplicit $true `
                -LogDir (Join-Path $TestDrive 'missing\logs') -SourceDir $src -SkipBuild }

        $msg | Should Match '^SYNAPSE_SKIP_BUILD_BINARY_MISSING'
        [System.IO.File]::ReadAllText($exe) | Should Be 'installed-old'
        Test-Path -LiteralPath (Join-Path $TestDrive 'missing\logs\setup-staging') | Should Be $false
    }

    It 'must_pass a normal build stages a verified copy of the built binary' {
        $script:seamFound | Should Be $true
        $exe = New-Installed 'built'
        $built = Join-Path $TestDrive 'built\target\release\synapse-mcp.exe'
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $built) | Out-Null
        [System.IO.File]::WriteAllText($built, 'just-built')

        $r = Resolve-SynapseInstallSource -ExePath $exe -CargoTarget (Join-Path $TestDrive 'built\target') -CargoTargetExplicit $true `
            -LogDir (Join-Path $TestDrive 'built\logs') -BuiltPath $built

        $r.Sha256 | Should Be (Get-SynapseFileSha256 -Path $built)
        $r.Path | Should Match ([regex]::Escape((Join-Path $TestDrive 'built\logs\setup-staging\daemon-binary-')))
        [System.IO.File]::ReadAllText($r.Path) | Should Be 'just-built'
    }

    It 'must_fail post-exit continuation refuses installed bytes that changed after the parent verified them' {
        $script:seamFound | Should Be $true
        $exe = New-Installed 'postexit'

        $msg = Get-ThrownMessage { Resolve-SynapseInstallSource -ExePath $exe -CargoTarget (Join-Path $TestDrive 'postexit\target') `
                -CargoTargetExplicit $true -LogDir (Join-Path $TestDrive 'postexit\logs') -SkipBuild -PostExitUseInstalledBinary `
                -PostExitInstalledSha256 ('0' * 64) }

        $msg | Should Match '^SYNAPSE_POST_EXIT_INSTALLED_BINARY_REPLACED'
    }

    It 'must_pass top-level setup takes the install source only from the seam' {
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
        (& $topLevel 'Resolve-SynapseInstallSource').Count | Should Be 1
        (& $topLevel 'Resolve-SynapseSkipBuildCandidatePath').Count | Should Be 0
        (& $topLevel 'New-SynapseStagedDaemonBinary').Count | Should Be 0
    }
}
