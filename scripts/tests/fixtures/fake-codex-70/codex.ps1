# Test double for `codex app-server --listen ws://127.0.0.1:<port>` used by
# codex-app-server-runner-config-merge-70.Tests.ps1. It serves /healthz and a minimal
# WebSocket JSON-RPC endpoint:
#   initialize  -> result {}
#   config/read -> result { config = $env:FAKE_CODEX_CONFIG } (or -32601 when
#                  $env:FAKE_CODEX_MODE = 'no_config_read')
#   thread/start -> writes its params to $env:FAKE_CODEX_RECORD, then answers with a
#                  JSON-RPC error so the runner stops before any model turn.
# Exits after the WebSocket closes or after 60 s.
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest)
$ErrorActionPreference = 'Stop'

$listen = [Uri]$Rest[[Array]::IndexOf($Rest, '--listen') + 1]
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse('127.0.0.1'), $listen.Port)
$listener.Start()
$deadline = [DateTime]::UtcNow.AddSeconds(60)

function Read-Exact($Stream, [int]$Count) {
    $buffer = [byte[]]::new($Count)
    $read = 0
    while ($read -lt $Count) {
        $n = $Stream.Read($buffer, $read, $Count - $read)
        if ($n -le 0) { throw 'peer closed' }
        $read += $n
    }
    return , $buffer
}

function Read-RequestHead($Stream) {
    $bytes = [System.Collections.Generic.List[byte]]::new()
    while ($true) {
        $b = $Stream.ReadByte()
        if ($b -lt 0) { break }
        $bytes.Add([byte]$b)
        $n = $bytes.Count
        if ($n -ge 4 -and $bytes[$n - 4] -eq 13 -and $bytes[$n - 3] -eq 10 -and $bytes[$n - 2] -eq 13 -and $bytes[$n - 1] -eq 10) { break }
    }
    return [System.Text.Encoding]::ASCII.GetString($bytes.ToArray())
}

function Write-Ascii($Stream, [string]$Text) {
    $bytes = [System.Text.Encoding]::ASCII.GetBytes($Text)
    $Stream.Write($bytes, 0, $bytes.Length)
    $Stream.Flush()
}

function Send-Text($Stream, [string]$Text) {
    $payload = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $head = [System.Collections.Generic.List[byte]]::new()
    $head.Add(0x81)
    if ($payload.Length -lt 126) {
        $head.Add([byte]$payload.Length)
    } elseif ($payload.Length -lt 65536) {
        $head.Add(126); $head.Add([byte]($payload.Length -shr 8)); $head.Add([byte]($payload.Length -band 0xFF))
    } else {
        $head.Add(127)
        for ($shift = 56; $shift -ge 0; $shift -= 8) { $head.Add([byte](([long]$payload.Length -shr $shift) -band 0xFF)) }
    }
    $Stream.Write($head.ToArray(), 0, $head.Count)
    $Stream.Write($payload, 0, $payload.Length)
    $Stream.Flush()
}

function Read-Text($Stream) {
    $h = Read-Exact $Stream 2
    $opcode = $h[0] -band 0x0F
    $length = [long]($h[1] -band 0x7F)
    if ($length -eq 126) { $e = Read-Exact $Stream 2; $length = ([long]$e[0] -shl 8) -bor $e[1] }
    elseif ($length -eq 127) { $e = Read-Exact $Stream 8; $length = 0; foreach ($x in $e) { $length = ($length -shl 8) -bor $x } }
    $mask = if ($h[1] -band 0x80) { Read-Exact $Stream 4 } else { $null }
    $payload = if ($length -gt 0) { Read-Exact $Stream ([int]$length) } else { [byte[]]::new(0) }
    if ($null -ne $mask) { for ($i = 0; $i -lt $payload.Length; $i++) { $payload[$i] = $payload[$i] -bxor $mask[$i % 4] } }
    if ($opcode -eq 8) { return $null }
    return [System.Text.Encoding]::UTF8.GetString($payload)
}

function Get-Reply($Message) {
    switch ([string]$Message.method) {
        'initialize' { return @{ id = $Message.id; result = @{} } }
        'config/read' {
            if ($env:FAKE_CODEX_MODE -eq 'no_config_read') {
                return @{ id = $Message.id; error = @{ code = -32601; message = 'method not found' } }
            }
            return @{ id = $Message.id; result = @{ config = ($env:FAKE_CODEX_CONFIG | ConvertFrom-Json) } }
        }
        'thread/start' {
            [System.IO.File]::WriteAllText($env:FAKE_CODEX_RECORD, ($Message.params | ConvertTo-Json -Depth 20))
            return @{ id = $Message.id; error = @{ code = -32000; message = 'fake codex stops after thread/start' } }
        }
        default { return @{ id = $Message.id; error = @{ code = -32601; message = 'unexpected method' } } }
    }
}

function Serve-WebSocket($Stream, [string]$Head) {
    if ($Head -notmatch 'Sec-WebSocket-Key:\s*(\S+)') { return }
    $sha1 = [System.Security.Cryptography.SHA1]::Create()
    $hash = $sha1.ComputeHash([System.Text.Encoding]::ASCII.GetBytes($Matches[1] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'))
    Write-Ascii $Stream ("HTTP/1.1 101 Switching Protocols`r`nUpgrade: websocket`r`nConnection: Upgrade`r`nSec-WebSocket-Accept: " + [Convert]::ToBase64String($hash) + "`r`n`r`n")
    while ($true) {
        $text = Read-Text $Stream
        if ($null -eq $text) { return }
        Send-Text $Stream ((Get-Reply ($text | ConvertFrom-Json)) | ConvertTo-Json -Compress -Depth 20)
    }
}

try {
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not $listener.Pending()) { Start-Sleep -Milliseconds 20; continue }
        $client = $listener.AcceptTcpClient()
        try {
            $stream = $client.GetStream()
            $head = Read-RequestHead $stream
            if ($head -match '^GET /healthz') {
                Write-Ascii $stream "HTTP/1.1 200 OK`r`nContent-Length: 2`r`nConnection: close`r`n`r`nok"
                continue
            }
            Serve-WebSocket $stream $head
            exit 0
        } catch {
            exit 0
        } finally {
            $client.Close()
        }
    }
} finally {
    $listener.Stop()
}
