$ErrorActionPreference = 'Stop'
# Pester 3.x suite (legacy `Should Be` syntax), like the other scripts/tests suites.
#   Import-Module Pester -RequiredVersion 3.4.0; Invoke-Pester <this file>
# MY TDD contract for the Edge title match in the shipped Chromium app profile
# (crates\synapse-profiles\profiles\chrome.toml, msedge.exe [[matches]]).
# Edge deliberately puts U+200B ZERO WIDTH SPACE between "Microsoft" and the
# space before "Edge" in its window titles (Eric Lawrence, Edge team,
# https://textslashplain.com/2022/01/14/recognizing-edge-windows); the team
# intends to remove it later, so both spellings must match. browsers/edge.json
# window_title_suffixes already lists both.
# The title_regex is read raw from the TOML: its only escape, `​`, has the
# same meaning in a TOML basic string (decoded before Rust's regex sees it) and
# in a .NET regex, so this test exercises the same pattern the daemon compiles.
$activePester = (Get-Command Should -ErrorAction SilentlyContinue | Select-Object -First 1).Module
$pesterMajor = if ($activePester) { $activePester.Version.Major } else { 0 }
if ($pesterMajor -ge 4) {
    throw "synapse-browser-bridge-shared-edge-title.Tests.ps1 targets Pester 3.x; loaded Pester major=$pesterMajor. Import-Module Pester -RequiredVersion 3.4.0 first."
}

Describe 'Synapse Chromium app profile Edge title match' {
    BeforeAll {
        $repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
        $toml = Get-Content -Raw -Encoding UTF8 -LiteralPath (Join-Path $repoRoot 'crates\synapse-profiles\profiles\chrome.toml')
        $match = [regex]::Match($toml, 'exe = "msedge\.exe"\r?\n(?:#[^\n]*\n)*title_regex = "(?<pattern>[^"]*)"')
        if (-not $match.Success) { throw 'chrome.toml has no msedge.exe [[matches]] entry with a title_regex' }
        $script:edgeTitleRegex = $match.Groups['pattern'].Value
        $script:zwsp = [string][char]0x200B
    }

    It 'must_pass_edge_title_with_and_without_zero_width_space_matches' {
        ("Inbox - Microsoft$($script:zwsp) Edge" -cmatch $script:edgeTitleRegex) | Should Be $true
        ('Inbox - Microsoft Edge' -cmatch $script:edgeTitleRegex) | Should Be $true
        ("New tab - Microsoft$($script:zwsp) Edge" -cmatch $script:edgeTitleRegex) | Should Be $true
    }

    It 'must_fail_non_edge_titles_never_match' {
        ('Inbox - Google Chrome' -cmatch $script:edgeTitleRegex) | Should Be $false
        ("Inbox - Microsoft$($script:zwsp)$($script:zwsp) Edge" -cmatch $script:edgeTitleRegex) | Should Be $false
        ("Inbox - Microsoft $($script:zwsp)Edge" -cmatch $script:edgeTitleRegex) | Should Be $false
        ('Inbox - MicrosoftX Edge' -cmatch $script:edgeTitleRegex) | Should Be $false
        ("Inbox - Microsoft$($script:zwsp) Edge - extra" -cmatch $script:edgeTitleRegex) | Should Be $false
        ('' -cmatch $script:edgeTitleRegex) | Should Be $false
    }
}
