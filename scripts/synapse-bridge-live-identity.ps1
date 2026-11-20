# Live bridge identity readback shared by install-synapse-chrome-debugger.ps1
# (standalone installer runs) and synapse-setup.ps1 (post-start verification).
# Dot-source only: defines functions, performs no I/O at load time.
#
# Contract (#251 / issue H): an installer outcome is "verified" only when the
# running daemon reports the deployed bridge identity for an active host.
# - Only the chrome_bridge row carries full identity, and only in a
#   browser_debugger-profile MCP session (#82 Q5: the normal profile and the
#   sessionless HTTP /health return a short allowlisted summary).
# - Every other Chromium row (edge, brave, opera, opera_gx, vivaldi) publishes
#   its own row contract (crates\synapse-mcp\src\browser_bridge_health.rs
#   active_host_health): bridge_host_id, extension_id, extension_browser_kind,
#   extension_build_id, extension_code_stamp, origin, transport.
# - No daemon listening yet is a deferred check, never a failure; a daemon that
#   IS listening but cannot prove the identity fails closed with a reason.

# Mirrors the extension manifest host_permissions (http://127.0.0.1:7700/*).
$script:SynapseBridgeDaemonDefaultBind = '127.0.0.1:7700'
# The one health row the daemon projects per session profile (server/health.rs).
$script:SynapseDetailedBridgeHealthKey = 'chrome_bridge'

function Test-SynapseBridgeDaemonListening {
    param([string]$Bind = $script:SynapseBridgeDaemonDefaultBind)
    $separator = $Bind.LastIndexOf(':')
    if ($separator -lt 1) { return $false }
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $connect = $client.ConnectAsync($Bind.Substring(0, $separator), [int]$Bind.Substring($separator + 1))
        if (-not $connect.Wait(2000)) { return $false }
        return $client.Connected
    } catch { return $false }
    finally { $client.Close() }
}

function New-SynapseBridgeExpectedIdentity {
    # Picks the identity contract the row's health entry actually publishes.
    # Fields are compared in their given order, so a caller lists the code
    # identity (#237 runtime stamp) first and its mismatch is the reported cause.
    # Advisory fields (load folder paths) never fail the check: a different
    # value is returned as a warning naming the actual value.
    param([object]$Row, [System.Collections.IDictionary]$DetailedValues, [System.Collections.IDictionary]$RowValues,
        [System.Collections.IDictionary]$DetailedAdvisory = @{})
    $detailed = [string]$Row.health_key -ceq $script:SynapseDetailedBridgeHealthKey
    [pscustomobject]@{
        browser = [string]$Row.id
        health_key = [string]$Row.health_key
        contract = if ($detailed) { 'browser_debugger_detailed_identity' } else { 'browser_row_identity' }
        host_id_key = if ($detailed) { 'active_host_id' } else { 'bridge_host_id' }
        fields = if ($detailed) { $DetailedValues } else { $RowValues }
        advisory = if ($detailed) { $DetailedAdvisory } else { @{} }
    }
}

function Get-SynapseChromeBridgeHealthDetailValue {
    param([string]$Detail, [string]$Name)
    $pattern = '(?:^|\s)' + [regex]::Escape($Name) + '=("(?:\\"|[^"])*"|[^\s]+)(?=\s|$)'
    $values = [regex]::Matches($Detail, $pattern)
    if ($values.Count -ne 1) { return $null }
    $value = $values[0].Groups[1].Value
    if ($value.StartsWith('"')) { return $value.Substring(1, $value.Length - 2).Replace('\"', '"') }
    return $value
}

function Get-SynapseBridgeIdentityFailureReason {
    # Only fixed bridge_identity_* tokens leave this module; anything else
    # (transport text, payload fragments) collapses to one opaque reason.
    param([string]$Message)
    if ($Message -cmatch '^bridge_identity_[a-z_]+(?::[a-z/]+)?$') { return $Message }
    return 'detailed_bridge_identity_unavailable_or_invalid'
}

function Invoke-SynapseChromeBridgeMcpRequest {
    param([hashtable]$Headers, [string]$Bind, [string]$Method, [hashtable]$Params, [int]$Id = 0)
    $request = @{jsonrpc='2.0';method=$Method;params=$Params}
    if ($Id -ne 0) { $request.id = $Id }
    try {
        $response = Invoke-WebRequest -Uri "http://$Bind/mcp" -Method Post -Headers $Headers -ContentType 'application/json' -Body ($request | ConvertTo-Json -Depth 20 -Compress) -MaximumRedirection 0 -TimeoutSec 8 -UseBasicParsing -ErrorAction Stop
        if ($Method -eq 'initialize') {
            $Headers['Mcp-Session-Id'] = [string](@($response.Headers['Mcp-Session-Id'])[0])
            if ([string]::IsNullOrWhiteSpace($Headers['Mcp-Session-Id'])) { throw 'missing_session' }
        }
        if ($Id -eq 0) { return }
        $content = if ($response.Content -is [byte[]]) { [Text.Encoding]::UTF8.GetString($response.Content) } else { [string]$response.Content }
        $payloads = if ($content.TrimStart().StartsWith('{')) { @($content) } else {
            # A profile change can precede its response with tools/list_changed.
            foreach ($frame in @(($content -replace "`r`n", "`n" -replace "`r", "`n") -split "`n`n")) {
                $data = @($frame -split "`n" | Where-Object { $_.StartsWith('data:') } | ForEach-Object { $_.Substring(5).TrimStart() }) -join "`n"
                if (-not [string]::IsNullOrWhiteSpace($data)) { $data }
            }
        }
        foreach ($payload in $payloads) {
            $message = $payload | ConvertFrom-Json -ErrorAction Stop
            if ($message.id -ne $Id) { continue }
            if ($message.jsonrpc -cne '2.0' -or $null -ne $message.error -or $null -eq $message.result) { throw 'invalid_response' }
            return $message.result
        }
        throw 'response_missing'
    } catch {
        # Keep the protocol cause; never forward transport/payload text.
        $cause = if ($_.Exception.Message -cmatch '^(missing_session|invalid_response|response_missing)$') { $Matches[1] } else { 'request_failed' }
        throw "bridge_identity_mcp_${cause}:$Method"
    }
}

function Read-SynapseChromeBridgeMcpToolPayload {
    param([object]$Result, [string]$ToolName)
    if ($Result.isError -eq $true) { throw "bridge_identity_tool_failed:$ToolName" }
    if ($null -ne $Result.structuredContent) { return $Result.structuredContent }
    $texts = @($Result.content | Where-Object { $_.type -ceq 'text' })
    if ($texts.Count -ne 1) { throw "bridge_identity_tool_readback_missing:$ToolName" }
    try { return $texts[0].text | ConvertFrom-Json -ErrorAction Stop }
    catch { throw "bridge_identity_tool_readback_invalid:$ToolName" }
}

function Set-SynapseChromeBridgeHealthProfile {
    param([hashtable]$Headers, [string]$Bind)
    $tools = Invoke-SynapseChromeBridgeMcpRequest -Headers $Headers -Bind $Bind -Method 'tools/list' -Params @{} -Id 2
    $profileTools = @($tools.tools | Where-Object { $_.name -ceq 'profile' })
    if ($profileTools.Count -ne 1 -or $profileTools[0].inputSchema.type -cne 'object') { throw 'bridge_identity_profile_tool_unavailable' }
    $result = Invoke-SynapseChromeBridgeMcpRequest -Headers $Headers -Bind $Bind -Method 'tools/call' -Id 3 -Params @{
        name='profile';arguments=@{operation='set';profile='browser_debugger';confirm_break_glass=$true;reason='Verify installed bridge identity against the active daemon host'}
    }
    $profile = Read-SynapseChromeBridgeMcpToolPayload -Result $result -ToolName 'profile'
    if ($profile.operation -cne 'set' -or $profile.set.after.profile -cne 'browser_debugger') {
        throw 'bridge_identity_profile_readback_mismatch'
    }
}

function Read-SynapseChromeBridgeDetailedHealth {
    param([string]$Token, [string]$Bind = $script:SynapseBridgeDaemonDefaultBind)
    $headers = @{Authorization="Bearer $Token";Accept='application/json, text/event-stream'}
    try {
        $init = Invoke-SynapseChromeBridgeMcpRequest -Headers $headers -Bind $Bind -Method 'initialize' -Id 1 -Params @{
            protocolVersion='2025-06-18';capabilities=@{};clientInfo=@{name='synapse-bridge-installer';version='1'}
        }
        if ([string]::IsNullOrWhiteSpace([string]$init.protocolVersion) -or $null -eq $init.capabilities) {
            throw 'bridge_identity_mcp_initialize_invalid'
        }
        $headers['MCP-Protocol-Version'] = [string]$init.protocolVersion
        Invoke-SynapseChromeBridgeMcpRequest -Headers $headers -Bind $Bind -Method 'notifications/initialized' -Params @{}
        Set-SynapseChromeBridgeHealthProfile -Headers $headers -Bind $Bind
        # Refetch the visible surface after profile selection. HTTP /health is
        # always sessionless; only the MCP health tool reads this session's profile.
        $tools = Invoke-SynapseChromeBridgeMcpRequest -Headers $headers -Bind $Bind -Method 'tools/list' -Params @{} -Id 4
        $healthTools = @($tools.tools | Where-Object { $_.name -ceq 'health' })
        if ($healthTools.Count -ne 1 -or $healthTools[0].inputSchema.type -cne 'object') { throw 'bridge_identity_health_tool_unavailable' }
        $result = Invoke-SynapseChromeBridgeMcpRequest -Headers $headers -Bind $Bind -Method 'tools/call' -Id 5 -Params @{name='health';arguments=@{}}
        $health = Read-SynapseChromeBridgeMcpToolPayload -Result $result -ToolName 'health'
        if ($null -eq $health.subsystems) { throw 'bridge_identity_detailed_health_invalid' }
        return $health
    } finally {
        try {
            if (-not [string]::IsNullOrWhiteSpace([string]$headers['Mcp-Session-Id'])) {
                Invoke-WebRequest -Uri "http://$Bind/mcp" -Method Delete -Headers $headers -MaximumRedirection 0 -TimeoutSec 4 -UseBasicParsing -ErrorAction Stop | Out-Null
            }
        } catch { throw 'bridge_identity_mcp_session_cleanup_failed' }
        finally { $headers.Clear(); $Token = $null }
    }
}

function Test-SynapseBridgeLiveIdentity {
    # Pure comparison of one detailed health payload against one expected
    # identity from New-SynapseBridgeExpectedIdentity.
    param([object]$Health, [object]$Expected)
    $healthKey = [string]$Expected.health_key
    $bridge = $Health.subsystems.$healthKey
    if ($null -eq $bridge) { return [pscustomobject]@{ok=$false;reason="bridge_health_row_missing:$healthKey"} }
    if ([string]$bridge.status -cne 'ok') {
        $status = if ([string]$bridge.status -cmatch '^[a-z_]{1,32}$') { ":$($bridge.status)" } else { '' }
        return [pscustomobject]@{ok=$false;reason="active_bridge_not_ready$status"}
    }
    $detail = [string]$bridge.detail
    foreach ($key in @($Expected.fields.Keys)) {
        $actual = Get-SynapseChromeBridgeHealthDetailValue -Detail $detail -Name $key
        if ($null -eq $actual) { return [pscustomobject]@{ok=$false;reason="detailed_bridge_identity_missing:$key"} }
        if ([string]$actual -cne [string]$Expected.fields[$key]) { return [pscustomobject]@{ok=$false;reason="active_bridge_identity_mismatch:$key"} }
    }
    $hostId = Get-SynapseChromeBridgeHealthDetailValue -Detail $detail -Name ([string]$Expected.host_id_key)
    if ([string]::IsNullOrWhiteSpace($hostId) -or $hostId -eq '<none>') { return [pscustomobject]@{ok=$false;reason='no_active_bridge_host'} }
    $warnings = @()
    foreach ($key in @($Expected.advisory.Keys)) {
        $actual = Get-SynapseChromeBridgeHealthDetailValue -Detail $detail -Name $key
        # Windows paths compare case-insensitively.
        if ([string]$actual -ine [string]$Expected.advisory[$key]) {
            $shown = if ($null -eq $actual) { '<missing>' } else { $actual }
            $warnings += "nonstandard_extension_folder:$key=$shown expected=$($Expected.advisory[$key])"
        }
    }
    [pscustomobject]@{ok=$true;reason='live_runtime_verified';source="authenticated_browser_debugger_mcp_health_active_host:$healthKey";warnings=$warnings}
}

function Read-SynapseBridgeLiveOutcome {
    param([object]$Expected, [string]$Token, [string]$Bind = $script:SynapseBridgeDaemonDefaultBind)
    try {
        if (-not (Test-SynapseBridgeDaemonListening -Bind $Bind)) {
            return [pscustomobject]@{ok=$false;deferred=$true;reason='live_runtime_verification_deferred_daemon_not_running'}
        }
        if ([string]::IsNullOrWhiteSpace($Token)) { return [pscustomobject]@{ok=$false;reason='daemon_health_credential_unavailable'} }
        $health = Read-SynapseChromeBridgeDetailedHealth -Bind $Bind -Token $Token
        return Test-SynapseBridgeLiveIdentity -Health $health -Expected $Expected
    } catch {
        [pscustomobject]@{ok=$false;reason=(Get-SynapseBridgeIdentityFailureReason -Message $_.Exception.Message)}
    }
    finally { $Token = $null }
}
