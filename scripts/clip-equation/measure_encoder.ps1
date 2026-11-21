# Processor time and output size of the Windows Media Foundation H.264 encoder (through ffmpeg h264_mf)
# on recorded screen clips. No window is opened. Prints one line per run.
$ErrorActionPreference = 'Stop'
$src = 'C:\Users\Adam\AppData\Local\Temp\claude\126c6878b97b40c9\cc94cc18-39e3-4e39-9f87-4a8c4a68d71b\scratchpad\clipbench\out'
$out = 'C:\Users\Adam\AppData\Local\Temp\claude\126c6878b97b40c9\cc94cc18-39e3-4e39-9f87-4a8c4a68d71b\scratchpad\clipmath\enc'
New-Item -ItemType Directory -Force $out | Out-Null
$ffmpeg = (Get-Command ffmpeg).Source

function Invoke-Run([string]$name, [string[]]$ffArgs, [string]$file) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ffmpeg
    $psi.Arguments = ($ffArgs -join ' ')
    $psi.UseShellExecute = $false
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = [System.Diagnostics.Process]::Start($psi)
    $err = $p.StandardError.ReadToEnd()
    $p.WaitForExit()
    $sw.Stop()
    $cpu = $p.TotalProcessorTime.TotalSeconds
    $frames = if ($err -match '(?s).*frame=\s*(\d+)') { $matches[1] } else { '?' }
    $size = if ($file -and (Test-Path $file)) { (Get-Item $file).Length } else { 0 }
    $mft = if ($err -match 'MFT name: .([^\r\n'']+)') { $matches[1].Trim() } else { '' }
    "{0,-22} exit={1} frames={2,4} cpu_s={3,6:N2} wall_s={4,5:N2} bytes={5,9} {6}" -f $name, $p.ExitCode, $frames, $cpu, $sw.Elapsed.TotalSeconds, $size, $mft
    if ($p.ExitCode -ne 0) { ($err -split "`n" | Select-Object -Last 2) -join ' | ' }
}

$mf = '-c:v h264_mf -rate_control quality -quality 70 -g 20 -pix_fmt nv12'
foreach ($clip in @(@{n='text'; f="$src\test_wc_6.mp4"}, @{n='still'; f="$src\fin_wc_6.mp4"})) {
    $i = "-hide_banner -y -loglevel verbose -i `"$($clip.f)`""
    $n = $clip.n
    Invoke-Run "$n decode"            @($i, '-f null -') $null
    Invoke-Run "$n decode+newonly"    @($i, '-vf mpdecimate -fps_mode vfr -f null -') $null
    Invoke-Run "$n mf_sw all"         @($i, $mf, '-hw_encoding 0', "`"$out\$n`_sw_all.mp4`"") "$out\$n`_sw_all.mp4"
    Invoke-Run "$n mf_hw all"         @($i, $mf, '-hw_encoding 1', "`"$out\$n`_hw_all.mp4`"") "$out\$n`_hw_all.mp4"
    Invoke-Run "$n mf_sw newonly"     @($i, '-vf mpdecimate -fps_mode vfr', $mf, '-hw_encoding 0', "`"$out\$n`_sw_new.mp4`"") "$out\$n`_sw_new.mp4"
    Invoke-Run "$n mf_hw newonly"     @($i, '-vf mpdecimate -fps_mode vfr', $mf, '-hw_encoding 1', "`"$out\$n`_hw_new.mp4`"") "$out\$n`_hw_new.mp4"
    Invoke-Run "$n mf_sw all g100"    @($i, '-c:v h264_mf -rate_control quality -quality 70 -g 100 -pix_fmt nv12 -hw_encoding 0', "`"$out\$n`_sw_g100.mp4`"") "$out\$n`_sw_g100.mp4"
}
