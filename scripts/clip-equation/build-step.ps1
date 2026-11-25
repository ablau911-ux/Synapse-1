param(
    [ValidateSet('check', 'probe', 'clippy', 'release', 'fmt')]
    [string]$Kind,
    [string]$Label
)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
Set-Location -LiteralPath $repo
$arguments = switch ($Kind) {
    'probe' { @('build', '--release', '-j', '4', '--manifest-path', 'scripts/clip-equation/probe/Cargo.toml') }
    'release' { @('build', '--release', '-j', '4') }
    'clippy' { @('clippy', '-j', '4', '--', '-D', 'warnings') }
    'fmt' { @('fmt', '--all') }
    default { @('check', '-j', '4') }
}
$prefix = Join-Path $PSScriptRoot "results/$Label"
$p = Start-Process cargo -ArgumentList $arguments -NoNewWindow -PassThru -RedirectStandardOutput "$prefix.stdout.txt" -RedirectStandardError "$prefix.stderr.txt"
$processHandle = $p.Handle
try { $p.PriorityClass = 'Idle' } catch { if (-not $p.HasExited) { throw } }
$p.WaitForExit()
"exit=$($p.ExitCode)" | Set-Content -LiteralPath "$prefix.exit.txt"
exit $p.ExitCode
