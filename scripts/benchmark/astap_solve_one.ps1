param(
  [string]$img,
  [string]$fov,
  [string]$db,
  [string]$smax = "500",
  [string]$tol = "0.007",
  [string]$extra = ""
)
$astap = "C:\Program Files\astap\astap_cli.exe"
$argl = @("-f", ('"' + $img + '"'), "-r", "180", "-fov", $fov, "-D", $db,
          "-d", '"C:\Program Files\astap"', "-s", $smax, "-t", $tol)
if ($extra -ne "") { $argl += $extra.Split(" ") }
$sw = [Diagnostics.Stopwatch]::StartNew()
$p = Start-Process -FilePath $astap -ArgumentList $argl -PassThru -WindowStyle Hidden
$peak = 0; $cpu = 0.0
while (-not $p.HasExited) {
  try {
    $p.Refresh()
    if ($p.PeakWorkingSet64 -gt $peak) { $peak = $p.PeakWorkingSet64 }
    $c = $p.TotalProcessorTime.TotalSeconds
    if ($c -gt $cpu) { $cpu = $c }
  } catch {}
  Start-Sleep -Milliseconds 10
}
$sw.Stop()
try { $c = $p.TotalProcessorTime.TotalSeconds; if ($c -gt $cpu) { $cpu = $c } } catch {}
try { if ($p.PeakWorkingSet64 -gt $peak) { $peak = $p.PeakWorkingSet64 } } catch {}
Write-Output ("WALL=" + $sw.Elapsed.TotalSeconds)
Write-Output ("CPU=" + $cpu)
Write-Output ("PEAK=" + $peak)
Write-Output ("EXIT=" + $p.ExitCode)
