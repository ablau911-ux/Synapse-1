$ErrorActionPreference = 'Stop'

# #125 re-review: `git status --porcelain` hides ignored files, so a merged worktree that
# holds an ignored .env looked clean and `git worktree remove` deleted it. The script must
# inventory ignored files and treat anything outside a disposable-artifact allowlist as
# data to preserve.
Describe 'repo-maintenance ignored-file inventory (#125)' {
    BeforeAll {
        $scriptPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'repo-maintenance.ps1'
        $tokens = $null
        $errors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        $functionAst = $ast.Find({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $node.Name -eq 'Get-WorktreeNonDisposableIgnored'
            }, $true)
        if ($null -eq $functionAst) { throw 'repo-maintenance function is missing: Get-WorktreeNonDisposableIgnored' }
        Invoke-Expression $functionAst.Extent.Text

        $paramAst = $ast.ParamBlock.Parameters | Where-Object { $_.Name.VariablePath.UserPath -eq 'DisposableIgnoredPatterns' }
        if ($null -eq $paramAst) { throw 'repo-maintenance parameter is missing: DisposableIgnoredPatterns' }
        $script:patterns = @(Invoke-Expression $paramAst.DefaultValue.Extent.Text)

        function Invoke-Git { & git @args 2>&1 | Out-Null; if ($LASTEXITCODE -ne 0) { throw "git $args failed" } }
        $script:repo = Join-Path $TestDrive 'primary'
        New-Item -ItemType Directory -Path $script:repo | Out-Null
        Invoke-Git -C $script:repo init -q
        Invoke-Git -C $script:repo config user.email t@example.invalid
        Invoke-Git -C $script:repo config user.name t
        # A global core.autocrlf=true makes git warn on stderr, which PS 5.1 + Stop turns into a throw.
        Invoke-Git -C $script:repo config core.autocrlf false
        [System.IO.File]::WriteAllText((Join-Path $script:repo '.gitignore'), ".env`n/target/`nnode_modules/`n")
        [System.IO.File]::WriteAllText((Join-Path $script:repo 'a.txt'), 'a')
        Invoke-Git -C $script:repo add -A
        Invoke-Git -C $script:repo commit -q -m init
    }

    function New-TestWorktree([string]$Name) {
        $wt = Join-Path $TestDrive $Name
        Invoke-Git -C $script:repo worktree add -q -b "br-$Name" $wt
        return $wt
    }

    It 'reports an ignored .env as data to preserve' {
        $wt = New-TestWorktree 'with-env'
        [System.IO.File]::WriteAllText((Join-Path $wt '.env'), 'SECRET=1')
        $found = @(Get-WorktreeNonDisposableIgnored -Path $wt -DisposablePatterns $script:patterns)
        $found.Count | Should Be 1
        $found[0] | Should Match '\.env'
    }

    It 'does not report disposable build directories (target, nested node_modules)' {
        $wt = New-TestWorktree 'build-only'
        New-Item -ItemType Directory -Path (Join-Path $wt 'target\debug') -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $wt 'target\debug\x.bin'), 'x')
        New-Item -ItemType Directory -Path (Join-Path $wt 'dashboard\node_modules\pkg') -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $wt 'dashboard\node_modules\pkg\i.js'), 'x')
        $found = @(Get-WorktreeNonDisposableIgnored -Path $wt -DisposablePatterns $script:patterns)
        $found.Count | Should Be 0
    }

    It 'reports an ignored file even when disposable directories sit beside it' {
        $wt = New-TestWorktree 'mixed'
        New-Item -ItemType Directory -Path (Join-Path $wt 'target') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $wt 'target\x.bin'), 'x')
        [System.IO.File]::WriteAllText((Join-Path $wt '.env'), 'SECRET=1')
        $found = @(Get-WorktreeNonDisposableIgnored -Path $wt -DisposablePatterns $script:patterns)
        $found.Count | Should Be 1
    }

    It 'throws (fail closed) when git cannot inventory the path' {
        $notRepo = Join-Path $TestDrive 'not-a-repo'
        New-Item -ItemType Directory -Path $notRepo | Out-Null
        $thrown = $null
        try { $null = @(Get-WorktreeNonDisposableIgnored -Path $notRepo -DisposablePatterns $script:patterns) } catch { $thrown = $_.Exception.Message }
        $thrown | Should Match 'git status --ignored failed'
    }
}
