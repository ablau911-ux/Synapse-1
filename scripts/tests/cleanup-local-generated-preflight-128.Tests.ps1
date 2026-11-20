$ErrorActionPreference = 'Stop'

# #128 raid tests (Pester 3.4.0). A cleanup script must preflight EVERY target (exact path,
# no reparse ancestor/descendant) before deleting ANY target. The shared helper
# cleanup-cache-common.ps1 does; cleanup-synapse-local-generated.ps1 validated and deleted
# one target at a time, so a junction in the second target let the first be deleted.
# NEEDS-SEAM: the local-generated cleaner hardcodes C:\Users\Adam paths; tests use the
# specified -LocalDataRoot parameter and an interlock refuses to run the script without it.
Describe 'cleanup scripts preflight all targets before deleting any (#128)' {
    BeforeAll {
        $scriptsDir = Split-Path -Parent $PSScriptRoot
        $script:localCleaner = Join-Path $scriptsDir 'cleanup-synapse-local-generated.ps1'
        . (Join-Path $scriptsDir 'cleanup-cache-common.ps1')

        function New-Tree([string]$Path) {
            New-Item -ItemType Directory -Force -Path $Path | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $Path 'sentinel.bin'), 'sentinel')
            return $Path
        }
        # SAFETY INTERLOCK (see #116 file): never invoke the cleaner unless it declares
        # -LocalDataRoot and holds no absolute drive-path literal.
        function Assert-LocalCleanerSeam {
            $tokens = $null; $errors = $null
            $ast = [System.Management.Automation.Language.Parser]::ParseFile($script:localCleaner, [ref]$tokens, [ref]$errors)
            $errors.Count | Should Be 0
            $declared = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
            ($declared -contains 'LocalDataRoot') | Should Be $true
            $absolute = @($ast.FindAll({
                        param($n)
                        $n -is [System.Management.Automation.Language.StringConstantExpressionAst] -and $n.Value -match '^[A-Za-z]:\\'
                    }, $true))
            $absolute.Count | Should Be 0
        }
        function New-LocalDataRoot([string]$Case, [switch]$JunctionInSecond) {
            $root = Join-Path $TestDrive "$Case\localdata\synapse"
            New-Tree (Join-Path $root 'build-target\debug') | Out-Null
            New-Tree (Join-Path $root 'build-target\release') | Out-Null
            $second = Join-Path $root 'codex-target-syn31-20260913\debug'
            New-Tree $second | Out-Null
            if ($JunctionInSecond) {
                $outside = New-Tree (Join-Path $TestDrive "$Case\outside")
                New-Item -ItemType Junction -Path (Join-Path $second 'linked') -Target $outside | Out-Null
            }
            return $root
        }
    }

    It 'must_fail local-generated cleaner: junction in the second target refuses before the first is deleted' {
        Assert-LocalCleanerSeam
        Mock Get-Process { @() }
        Mock Get-CimInstance { @() }
        $root = New-LocalDataRoot 'junction' -JunctionInSecond
        $link = Join-Path $root 'codex-target-syn31-20260913\debug\linked'
        ((Get-Item -LiteralPath $link -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) | Should Not Be 0

        $message = $null
        try { $null = & $script:localCleaner -Apply -LocalDataRoot $root } catch { $message = $_.Exception.Message }

        $message | Should Match 'Refusing reparse'
        # Assert existence before reading bytes: a sequential delete must fail here,
        # not escape as FileNotFoundException before reaching a bug assertion.
        ('#128 first target intact: ' + (Test-Path -LiteralPath (Join-Path $root 'build-target\debug\sentinel.bin'))) | Should Be '#128 first target intact: True'
        [System.IO.File]::ReadAllText((Join-Path $root 'build-target\debug\sentinel.bin')) | Should Be 'sentinel'
        [System.IO.File]::ReadAllText((Join-Path $TestDrive 'junction\outside\sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_fail local-generated cleaner without -Apply deletes nothing and reports Apply=false' {
        Assert-LocalCleanerSeam
        $root = New-LocalDataRoot 'dry'

        $result = (& $script:localCleaner -LocalDataRoot $root | Out-String) | ConvertFrom-Json

        $result.Apply | Should Be $false
        @($result.Deleted).Count | Should Be 0
        [System.IO.File]::ReadAllText((Join-Path $root 'build-target\debug\sentinel.bin')) | Should Be 'sentinel'
        # A preview still preflights the whole plan. Validate a second fixture
        # containing a junction without applying any deletion.
        $invalid = New-LocalDataRoot 'dry-invalid' -JunctionInSecond
        $message = ''
        try { $null = & $script:localCleaner -LocalDataRoot $invalid } catch { $message = $_.Exception.Message }
        ('#128 dry-run preflight: ' + $message) | Should Match '^#128 dry-run preflight: Refusing reparse'
        Test-Path -LiteralPath (Join-Path $invalid 'build-target\debug\sentinel.bin') | Should Be $true
    }

    It 'must_pass local-generated cleaner deletes exactly the two debug trees and reports them' {
        Assert-LocalCleanerSeam
        Mock Get-Process { @() }
        Mock Get-CimInstance { @() }
        $root = New-LocalDataRoot 'apply'

        $result = (& $script:localCleaner -Apply -LocalDataRoot $root | Out-String) | ConvertFrom-Json

        $deleted = @($result.Deleted | ForEach-Object { [System.IO.Path]::GetFullPath($_).TrimEnd('\') }) | Sort-Object
        $expected = @((Join-Path $root 'build-target\debug'), (Join-Path $root 'codex-target-syn31-20260913\debug')) |
            ForEach-Object { [System.IO.Path]::GetFullPath($_).TrimEnd('\') } | Sort-Object
        ($deleted -join '|') | Should Be ($expected -join '|')
        [System.IO.File]::ReadAllText((Join-Path $root 'build-target\release\sentinel.bin')) | Should Be 'sentinel'
    }

    It 'must_fail shared helper: junction in the second Tree target refuses before the first is deleted' {
        $first = New-Tree (Join-Path $TestDrive 'common\t1')
        $second = New-Tree (Join-Path $TestDrive 'common\t2')
        $outside = New-Tree (Join-Path $TestDrive 'common\outside')
        New-Item -ItemType Junction -Path (Join-Path $second 'linked') -Target $outside | Out-Null

        { Invoke-SynapseCacheCleanup -Targets @($first, $second) -Mode Tree -Apply } | Should Throw 'Refusing reparse descendant'

        [System.IO.File]::ReadAllText((Join-Path $first 'sentinel.bin')) | Should Be 'sentinel'
        [System.IO.File]::ReadAllText((Join-Path $outside 'sentinel.bin')) | Should Be 'sentinel'
        # The entry script must provide the same all-target safety as the shared
        # helper; guarding only the helper leaves the actual local cleaner unsafe.
        Assert-LocalCleanerSeam
        Mock Get-Process { @() }
        $root = New-LocalDataRoot 'common-parity' -JunctionInSecond
        $message = ''
        try { $null = & $script:localCleaner -Apply -LocalDataRoot $root } catch { $message = $_.Exception.Message }
        $message | Should Match 'Refusing reparse'
        ('#128 entrypoint matches shared all-target preflight: ' + (Test-Path -LiteralPath (Join-Path $root 'build-target\debug\sentinel.bin'))) | Should Be '#128 entrypoint matches shared all-target preflight: True'
    }

    It 'must_pass shared helper: Tree mode deletes every validated target' {
        $first = New-Tree (Join-Path $TestDrive 'commonok\t1')
        $second = New-Tree (Join-Path $TestDrive 'commonok\t2')
        $keep = New-Tree (Join-Path $TestDrive 'commonok\keep')

        $result = Invoke-SynapseCacheCleanup -Targets @($first, $second) -Mode Tree -Apply

        @($result.Deleted).Count | Should Be 2
        Test-Path -LiteralPath $first | Should Be $false
        Test-Path -LiteralPath $second | Should Be $false
        [System.IO.File]::ReadAllText((Join-Path $keep 'sentinel.bin')) | Should Be 'sentinel'
    }
}
