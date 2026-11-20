# Shared throwaway-git-repo fixtures for the repo-maintenance.ps1 child-process Pester tests
# (#263, #264). Dot-source from BeforeAll:  . (Join-Path $PSScriptRoot 'fixtures\repo-maintenance-fixture.ps1')
# Added 2026-10-16 19:10 +03:00. Not a *.Tests.ps1 file, so Pester does not run it.
# The caller must set GIT_CEILING_DIRECTORIES to $TestDrive so git never discovers a real repository.
# Every repo gets core.autocrlf=false (a global autocrlf=true makes git warn on stderr).
# These helpers never assert how the script suppresses git stderr: they only expose outcomes.

$script:FxMaint = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'repo-maintenance.ps1'
$script:FxPs51 = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$FxPwshCmd = Get-Command pwsh -ErrorAction SilentlyContinue
$script:FxPwsh = if ($FxPwshCmd) { $FxPwshCmd.Source } else { $null }

function Fx-GitOut {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $out = & git @args 2>&1; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $old }
    if ($code -ne 0) { throw "fixture git $args failed ($code): $out" }
    return (($out | ForEach-Object { "$_" }) -join "`n").Trim()
}
function Fx-Git { $null = Fx-GitOut @args }
function Fx-BranchExists([string]$Repo, [string]$Name) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $null = & git -C $Repo show-ref --verify --quiet "refs/heads/$Name" 2>&1; return ($LASTEXITCODE -eq 0) } finally { $ErrorActionPreference = $old }
}
function Fx-Commit([string]$Repo, [string]$File, [string]$Message = $File) {
    [System.IO.File]::WriteAllText((Join-Path $Repo $File), $File)
    Fx-Git -C $Repo add -A
    Fx-Git -C $Repo commit -q -m $Message
}
# git's real exit code for a call that is expected to fail (never throws).
function Fx-GitCode {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { $null = & git @args 2>&1; return $LASTEXITCODE } finally { $ErrorActionPreference = $old }
}
# Lines of $Text matching $Pattern (script output is parsed per line so one message cannot satisfy another).
function Fx-Lines([string]$Text, [string]$Pattern) { return @(($Text -split '\r?\n') | Where-Object { $_ -match $Pattern }) }
function Fx-ExeTag([string]$Exe) { return [System.IO.Path]::GetFileNameWithoutExtension($Exe) }
function Fx-NewRepo([string]$Case, [switch]$WithOrigin) {
    $root = Join-Path $TestDrive "$Case\root"
    $repo = Join-Path $root 'repo'
    New-Item -ItemType Directory -Force -Path $repo | Out-Null
    Fx-Git -C $repo init -q -b main
    Fx-Git -C $repo config user.email t@example.invalid
    Fx-Git -C $repo config user.name t
    Fx-Git -C $repo config core.autocrlf false
    Fx-Commit $repo 'init.txt'
    $fx = @{ Root = $root; Repo = $repo; Backup = (Join-Path $TestDrive "$Case\backups"); Wts = (Join-Path $TestDrive "$Case\wts"); Origin = $null }
    if ($WithOrigin) {
        $fx.Origin = Join-Path $TestDrive "$Case\origin.git"
        Fx-Git init -q --bare -b main $fx.Origin
        Fx-Git -C $repo remote add origin $fx.Origin
        Fx-Git -C $repo push -q origin main
    }
    return $fx
}
function Fx-NewWorktree($Fx, [string]$Name) {
    $wt = Join-Path $Fx.Wts $Name
    Fx-Git -C $Fx.Repo worktree add -q -b "br-$Name" $wt
    return $wt
}
# Runs the REAL repo-maintenance.ps1 in a child process (Windows PowerShell 5.1 or pwsh 7).
function Fx-RunMaint($Fx, [string]$Exe, [string[]]$Extra, [hashtable]$ChildEnv = @{}) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($k in $ChildEnv.Keys) { $psi.EnvironmentVariables[$k] = [string]$ChildEnv[$k] }
    $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$($script:FxMaint)`"", '-Root', "`"$($Fx.Root)`"", '-BackupDir', "`"$($Fx.Backup)`"", '-MinFreeGB', '0') + $Extra
    $psi.Arguments = $argv -join ' '
    $proc = [System.Diagnostics.Process]::Start($psi)
    $so = $proc.StandardOutput.ReadToEndAsync()
    $se = $proc.StandardError.ReadToEndAsync()
    if (-not $proc.WaitForExit(240000)) { $proc.Kill(); throw 'repo-maintenance child run timed out' }
    $proc.WaitForExit()
    return [pscustomobject]@{ Code = $proc.ExitCode; Text = ($so.Result + "`n" + $se.Result) }
}
# Exit 0 expected; on failure the child's own text is put in the failure message so the cause is visible.
function Fx-AssertCompleted($Run) {
    if ($Run.Code -ne 0) { throw "repo-maintenance exited $($Run.Code), expected 0. Output: $($Run.Text.Trim())" }
}
# A gone branch that is checked out somewhere is KEPT with the script's own message. git refuses
# `branch -D` on a checked-out branch by itself, so the branch surviving proves nothing: the script must
# say it skipped it and must never report it as deleted.
function Fx-AssertKeptNotDeleted([string]$Text, [string]$Branch) {
    $name = [regex]::Escape($Branch)
    $Text | Should Not Match "\[APPLY\] delete gone branch: $name(\s|$)"
    (Fx-Lines $Text "\[maint\]\[WARN\].*\b$name\b.*(checked out|worktree)").Count | Should Not Be 0
}

# ---- gone-branch / backup-tag helpers shared by the #266 and #267 tests ----
# Branch $Branch with one commit of its own, pushed with upstream, then deleted on the bare origin:
# `fetch --prune` makes it [gone]. -Merged also fast-forwards main (and origin/main) to it first.
function Fx-NewGoneBranch([string]$Case, [string]$Branch, [switch]$Merged) {
    $fx = Fx-NewRepo $Case -WithOrigin
    Fx-Git -C $fx.Repo checkout -q -b $Branch
    Fx-Commit $fx.Repo (($Branch -replace '/', '_') + '.txt')
    $fx.Head = Fx-GitOut -C $fx.Repo rev-parse HEAD
    Fx-Git -C $fx.Repo push -q -u origin $Branch
    Fx-Git -C $fx.Repo checkout -q main
    if ($Merged) {
        Fx-Git -C $fx.Repo merge -q --ff-only $Branch
        Fx-Git -C $fx.Repo push -q origin main
    }
    Fx-Git -C $fx.Origin branch -D $Branch
    return $fx
}
function Fx-GoneTags($Fx) {
    return @((Fx-GitOut -C $Fx.Repo tag --list 'maint-backup/*') -split '\r?\n' | Where-Object { $_ -match '/gone-' })
}
function Fx-TagTarget($Fx, [string]$Tag) { return (Fx-GitOut -C $Fx.Repo rev-parse "${Tag}^{commit}") }
function Fx-HeaderText {
    $t = [System.IO.File]::ReadAllText($script:FxMaint)
    return $t.Substring(0, $t.IndexOf('#>'))
}
