param(
  [string]$solver,   # ansvr | astap | asps | ps3
  [string]$group,    # A (blind) | B (scale) | C (hinted) | D (warm/cached)
  [string]$jobs,     # JSONL: one job per line
  [string]$outfile   # JSONL results, appended as we go
)
$ErrorActionPreference = "SilentlyContinue"
$curl = "C:\Windows\System32\curl.exe"

# ---- process-family accounting -------------------------------------------
function NewTracker($regex) {
  $b = @{}
  foreach ($pr in Get-Process) {
    if ($pr.ProcessName -match $regex) { try { $b[$pr.Id] = $pr.TotalProcessorTime.TotalSeconds } catch {} }
  }
  return @{ regex = $regex; base = $b; cpu = @{}; mem = @{}; peak = 0 }
}
function Track($t) {
  foreach ($pr in Get-Process) {
    if ($pr.ProcessName -match $t.regex) {
      try { $t.cpu[$pr.Id] = $pr.TotalProcessorTime.TotalSeconds } catch {}
      try { $t.mem[$pr.Id] = $pr.WorkingSet64 } catch {}
    }
  }
  $s = 0; foreach ($v in $t.mem.Values) { $s += $v }
  if ($s -gt $t.peak) { $t.peak = $s }
}
function CpuOf($t) {
  $s = 0.0
  foreach ($k in $t.cpu.Keys) {
    $b = 0.0; if ($t.base.ContainsKey($k)) { $b = $t.base[$k] }
    $d = $t.cpu[$k] - $b; if ($d -gt 0) { $s += $d }
  }
  return $s
}

$rx = switch ($solver) {
  "ansvr" { '^perl$|solve-field|astrometry-engine|image2pnm|image2xy|an-fitstopnm|jpegtopnm|augment-xylist' }
  "astap" { '^astap_cli$' }
  "asps"  { '^PlateSolver$|solve-field|astrometry-engine|image2pnm|image2xy|an-fitstopnm' }
  "ps3"   { '^PlateSolve3$' }
}

$lines = Get-Content $jobs
foreach ($line in $lines) {
  if (-not $line.Trim()) { continue }
  $j = $line | ConvertFrom-Json
  $img = $j.win_path
  $res = @{ id = $j.id; solver = $solver; group = $group; status = "error" }
  $t = NewTracker $rx
  $sw = [Diagnostics.Stopwatch]::StartNew()

  if ($solver -eq "ansvr") {
    # nova-style API; ansvr only understands SGP-framed uploads, and its JSON
    # must come from a file or PowerShell mangles the quotes.
    $jf = "C:\astrometry\_j.json"
    $sess = ""
    '{"apikey":"x"}' | Set-Content -Path $jf -Encoding ascii -NoNewline
    $lr = & $curl -s -m 15 -H "Expect:" -X POST "http://127.0.0.1:8080/api/login" -F ("request-json=<" + $jf)
    $m = [regex]::Match(($lr -join ""), '"session"\s*:\s*"([^"]*)"')
    if ($m.Success) { $sess = $m.Groups[1].Value }
    $hint = ""
    if ($group -ne "A") {
      $lo = [double]$j.pixscale * 0.9; $hi = [double]$j.pixscale * 1.1
      $hint = ',"scale_units":"arcsecperpix","scale_type":"ul","scale_lower":' + $lo + ',"scale_upper":' + $hi
      if ($group -ne "B") { $hint += ',"center_ra":' + $j.ra + ',"center_dec":' + $j.dec + ',"radius":5' }
    }
    ('{"session":"' + $sess + '","allow_commercial_use":"d","allow_modifications":"d","publicly_visible":"y"' + $hint + '}') |
        Set-Content -Path $jf -Encoding ascii -NoNewline
    $up = & $curl -s -m 600 -H "Expect:" -X POST "http://127.0.0.1:8080/api/upload" `
          -F ("request-json=<" + $jf) -F ("file=@" + $img.Replace("\","/") + ";type=application/octet-stream")
    $ms = [regex]::Match(($up -join ""), '"subid"\s*:\s*(\d+)')
    if ($ms.Success) {
      $sub = $ms.Groups[1].Value
      $job = $null
      for ($i = 0; $i -lt 4000; $i++) {
        Track $t
        $sj = & $curl -s -m 10 ("http://127.0.0.1:8080/api/submissions/" + $sub)
        $mj = [regex]::Match(($sj -join ""), '"jobs"\s*:\s*\[\s*(\d+)')
        if ($mj.Success) { $job = $mj.Groups[1].Value; break }
        Start-Sleep -Milliseconds 100
      }
      $sw.Stop()
      if ($job -ne $null) {
        $st = & $curl -s -m 10 ("http://127.0.0.1:8080/api/jobs/" + $job)
        if ($st -match '"status":"(success|failure)"') { $res.status = $Matches[1] }
        if ($res.status -eq "success") {
          $cal = & $curl -s -m 10 ("http://127.0.0.1:8080/api/jobs/" + $job + "/calibration")
          if ($cal -match '"ra":([-0-9.eE]+)') { $res.ra = [double]$Matches[1] }
          if ($cal -match '"dec":([-0-9.eE]+)') { $res.dec = [double]$Matches[1] }
          if ($cal -match '"pixscale":([-0-9.eE]+)') { $res.scale = [double]$Matches[1] }
        }
      } else { $res.status = "timeout" }
    } else { $res.status = "upload_failed"; $sw.Stop() }
  }
  else {
    $style = "Normal"; $waitFile = $false
    switch ($solver) {
      "astap" {
        $style = "Hidden"
        $exe = "C:\Program Files\astap\astap_cli.exe"
        $out = [IO.Path]::ChangeExtension($img, ".ini")
        $fov = if ($group -eq "A") { "0" } else { [string]$j.fov_deg }
        # ASTAP's own FOV->database guidance: >20deg W08, 6-20deg G05,
        # 0.5-6deg D50, <0.5deg D80. "auto" fails outright on wide fields.
        $fdeg = [double]$j.fov_deg
        $db = if ($group -eq "A") { "auto" }
              elseif ($fdeg -gt 20) { "w08" } elseif ($fdeg -gt 6) { "g05" }
              elseif ($fdeg -gt 0.5) { "d50" } else { "d80" }
        $argl = @("-f", ('"' + $img + '"'), "-fov", $fov, "-D", $db,
                  "-d", '"C:\Program Files\astap"', "-s", "500", "-t", "0.007")
        if ($group -eq "A" -or $group -eq "B") { $argl += @("-r", "180") }
        else { $argl += @("-ra", [string]([double]$j.ra / 15.0), "-spd", [string]([double]$j.dec + 90.0), "-r", "5") }
      }
      "asps" {
        $exe = "C:\Program Files (x86)\PlateSolver\PlateSolver.exe"
        $waitFile = $true
        # groups A-C: unique filename per solve so nothing can be reused;
        # group D deliberately reuses the path to let its cache work.
        if ($group -eq "D") { $aimg = $img; $out = [IO.Path]::ChangeExtension($img, ".asps.txt") }
        else {
          $tag = [guid]::NewGuid().ToString("N").Substring(0, 8)
          $aimg = "C:\astrometry\great\_a_" + $tag + ".fits"
          Copy-Item $img $aimg -Force
          $out = "C:\astrometry\great\_a_" + $tag + ".txt"
        }
        $focal = 206.265 * 10.0 / [double]$j.pixscale
        if ($group -eq "C" -or $group -eq "D") {
          $argl = @("/SOLVEFILE", ('"' + $aimg + '"'), ('"' + $out + '"'),
                    [string][math]::Round($focal, 2), "10", [string]$j.ra, [string]$j.dec, "5")
        } else {
          $argl = @("/SOLVEFILE", ('"' + $aimg + '"'), ('"' + $out + '"'),
                    [string][math]::Round($focal, 2), "10", "0", "0", "0")
        }
      }
      "ps3" {
        $exe = "C:\PlateSolve3.80_patched\PlateSolve3.exe"
        $out = [IO.Path]::GetDirectoryName($img) + "\" + [IO.Path]::GetFileNameWithoutExtension($img) + "_PS3.txt"
        # PS3 wants RA/Dec in DEGREES but the field size in RADIANS.
        $sizeRad = [double]$j.fov_deg * [Math]::PI / 180.0
        if ($group -eq "A")      { $argl = @(('"' + $img + '"'), "0", "0", "0", "0") }
        elseif ($group -eq "B")  { $argl = @(('"' + $img + '"'), "0", "0", [string]$sizeRad, [string]$sizeRad) }
        else                     { $argl = @(('"' + $img + '"'), [string]$j.ra, [string]$j.dec, [string]$sizeRad, [string]$sizeRad) }
      }
    }
    Remove-Item $out -Force -ErrorAction SilentlyContinue
    $p = Start-Process -FilePath $exe -ArgumentList $argl -PassThru -WindowStyle $style
    $deadline = [int]$j.timeout_s
    while (-not $p.HasExited) {
      Track $t
      if ($sw.Elapsed.TotalSeconds -gt $deadline) { try { $p.Kill() } catch {}; break }
      Start-Sleep -Milliseconds 100
    }
    if ($waitFile) {
      while ($sw.Elapsed.TotalSeconds -lt $deadline) {
        Track $t
        if ((Test-Path $out) -and ((Get-Item $out).Length -gt 0)) { break }
        Start-Sleep -Milliseconds 100
      }
    }
    $sw.Stop()
    Track $t

    if (Test-Path $out) {
      $txt = Get-Content $out
      if ($solver -eq "astap") {
        if (($txt -join "`n") -match "PLTSOLVD=T") {
          $res.status = "success"
          foreach ($l in $txt) {
            if ($l -match "^CRVAL1=\s*(\S+)") { $res.ra = [double]$Matches[1] }
            if ($l -match "^CRVAL2=\s*(\S+)") { $res.dec = [double]$Matches[1] }
            if ($l -match "^CDELT2=\s*(\S+)") { $res.scale = [double]$Matches[1] * 3600.0 }
          }
        } else { $res.status = "failure" }
      }
      elseif ($solver -eq "asps") {
        if ($txt[0].Trim() -eq "OK") {
          $res.status = "success"; $res.ra = [double]$txt[1]; $res.dec = [double]$txt[2]; $res.scale = [double]$txt[5]
        } else { $res.status = "failure" }
      }
      elseif ($solver -eq "ps3") {
        if ($txt[0].Trim().ToLower() -eq "true") {
          $res.status = "success"
          $c = $txt[1].Split(","); $res.ra = [double]$c[0] * 180.0 / [Math]::PI; $res.dec = [double]$c[1] * 180.0 / [Math]::PI
          $sc = $txt[2].Split(","); if ([double]$sc[0] -ne 0) { $res.scale = 206264.8 / [double]$sc[0] }
          if ($txt.Count -ge 4) { $res.method = $txt[3] }
        } else { $res.status = "failure" }
      }
    } else { $res.status = "no_output" }

    if ($solver -eq "ps3") {
      Copy-Item "$env:LOCALAPPDATA\PlateSolve3\PlateSolve.cfg.flbak" "$env:LOCALAPPDATA\PlateSolve3\PlateSolve.cfg" -Force
    }
    if ($solver -eq "asps") {
      # single-instance app: let it fully exit before the next job starts
      for ($k = 0; $k -lt 600; $k++) {
        if (-not (Get-Process PlateSolver -ErrorAction SilentlyContinue)) { break }
        Start-Sleep -Milliseconds 200
      }
      if ($group -ne "D") { Remove-Item $aimg -Force -ErrorAction SilentlyContinue }
    }
  }

  $res.wall_s = $sw.Elapsed.TotalSeconds
  $res.cpu_s = CpuOf $t
  $res.peak_mem = $t.peak
  ($res | ConvertTo-Json -Compress) | Add-Content -Path $outfile
}
Write-Output "BATCH_DONE"
