$ErrorActionPreference = 'Stop'

# #15: -ActiveIssue / SYNAPSE_ACTIVE_ISSUE is pure input. Invalid syntax must fail
# before the maintenance lock, build, staging, or candidate preflight.
Describe 'Synapse setup ActiveIssue early validation (#15)' {
    BeforeAll {
        $setupPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'synapse-setup.ps1'
        $tokens = $null
        $errors = $null
        $script:ast = [System.Management.Automation.Language.Parser]::ParseFile($setupPath, [ref]$tokens, [ref]$errors)
        if ($errors.Count -gt 0) { throw ($errors | Out-String) }
        function Get-FunctionText([string]$Name) {
            $fn = $script:ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $Name }, $true)
            if ($null -eq $fn) { throw "function $Name is missing" }
            return $fn.Extent.Text
        }
        Invoke-Expression (Get-FunctionText 'Get-SynapseNormalizedIssueRef')
        function Die([string]$Message) { throw $Message }

        # Top-level (script scope) command invocations by source offset.
        $script:topLevel = @($script:ast.EndBlock.Statements)
        function Get-TopLevelOffset([scriptblock]$Predicate) {
            foreach ($statement in $script:topLevel) {
                $commands = @($statement.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] }, $false))
                foreach ($command in $commands) {
                    if (& $Predicate $command) { return $statement.Extent.StartOffset }
                }
            }
            return $null
        }
    }

    It 'accepts number, hash-number and github url forms' {
        Get-SynapseNormalizedIssueRef -Issue '15' | Should Be '#15'
        Get-SynapseNormalizedIssueRef -Issue '#15' | Should Be '#15'
        Get-SynapseNormalizedIssueRef -Issue 'https://github.com/ChrisRoyse/Synapse/issues/15' | Should Be '#15'
    }

    It 'treats omitted and empty values as no active issue' {
        Get-SynapseNormalizedIssueRef -Issue $null | Should BeNullOrEmpty
        Get-SynapseNormalizedIssueRef -Issue '   ' | Should BeNullOrEmpty
    }

    It 'rejects structurally invalid values' {
        { Get-SynapseNormalizedIssueRef -Issue 'abc' } | Should Throw 'SYNAPSE_ACTIVE_ISSUE_INVALID'
        { Get-SynapseNormalizedIssueRef -Issue '#12x' } | Should Throw 'SYNAPSE_ACTIVE_ISSUE_INVALID'
    }

    It 'validates ActiveIssue at script scope before the maintenance lock' {
        $validate = Get-TopLevelOffset {
            param($c)
            $c.GetCommandName() -eq 'Get-SynapseNormalizedIssueRef' -and $c.Extent.Text -match 'ActiveIssue'
        }
        $lock = Get-TopLevelOffset { param($c) $c.GetCommandName() -eq 'Acquire-SynapseSetupMaintenanceLock' }
        $lock | Should Not BeNullOrEmpty
        $validate | Should Not BeNullOrEmpty
        ($validate -lt $lock) | Should Be $true
    }

    It 'validates ActiveIssue before preflight, build and staging steps' {
        $validate = Get-TopLevelOffset {
            param($c)
            $c.GetCommandName() -eq 'Get-SynapseNormalizedIssueRef' -and $c.Extent.Text -match 'ActiveIssue'
        }
        # Staging runs inside Resolve-SynapseInstallSource; its top-level call is the staging step.
        $stage = Get-TopLevelOffset { param($c) $c.GetCommandName() -in 'New-SynapseStagedDaemonBinary', 'Resolve-SynapseInstallSource' }
        $stage | Should Not BeNullOrEmpty
        $candidate = Get-TopLevelOffset { param($c) $c.GetCommandName() -eq 'Test-SynapseCandidateDaemon' }
        $validate | Should Not BeNullOrEmpty
        ($validate -lt $stage) | Should Be $true
        ($validate -lt $candidate) | Should Be $true
    }
}
