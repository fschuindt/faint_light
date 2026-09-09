# Test plate solving end to end via the nova.astrometry.net API at /nova.
#
# The PowerShell twin of test_submission.sh, for Windows hosts without a
# shell or jq. Uses curl.exe (shipped with Windows 10+) for the multipart
# uploads and PowerShell's own JSON parsing for the rest.
#
# Usage:   .\scripts\test_submission.ps1 <image_file> <host:port>
# Example: .\scripts\test_submission.ps1 .\testdata\bench\base_m.jpg localhost:7222

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$File,
    [Parameter(Mandatory = $true)][string]$Server,
    [int]$PollSeconds = 2,
    [int]$TimeoutSeconds = 600
)

$ErrorActionPreference = 'Stop'
$ApiKey = '1jcrmadfnxngxscd'
$BaseUrl = "http://$Server/nova"

if (-not (Test-Path -LiteralPath $File)) {
    Write-Error "file '$File' not found"
}
$File = (Resolve-Path -LiteralPath $File).Path

function Invoke-Api {
    param([string]$Path, [string[]]$Form)
    $curlArgs = @('-s', '--fail-with-body', "$BaseUrl$Path")
    foreach ($f in $Form) { $curlArgs += @('-F', $f) }
    $body = & curl.exe @curlArgs
    if ($LASTEXITCODE -ne 0) { Write-Error "$Path failed: $body" }
    $body | ConvertFrom-Json
}

$clock = [System.Diagnostics.Stopwatch]::StartNew()

Write-Host '--- Login ---'
$login = Invoke-Api -Path '/api/login' -Form @("request-json={""apikey"": ""$ApiKey""}")
if ($login.status -ne 'success') { Write-Error "login failed: $($login | ConvertTo-Json)" }
Write-Host "Authenticated. Session: $($login.session)"

Write-Host ''
Write-Host "--- Upload: $(Split-Path -Leaf $File) ---"
$upload = Invoke-Api -Path '/api/upload' -Form @(
    "request-json={""session"": ""$($login.session)""}",
    "file=@$File"
)
if ($upload.status -ne 'success') { Write-Error "upload failed: $($upload | ConvertTo-Json)" }
Write-Host "Submission ID: $($upload.subid)"

Write-Host ''
Write-Host '--- Waiting for job ---'
$jobId = $null
while (-not $jobId) {
    if ($clock.Elapsed.TotalSeconds -ge $TimeoutSeconds) {
        Write-Error "timed out after ${TimeoutSeconds}s waiting for a job"
    }
    $sub = Invoke-RestMethod "$BaseUrl/api/submissions/$($upload.subid)"
    $jobId = @($sub.jobs) | Where-Object { $null -ne $_ } | Select-Object -First 1
    if (-not $jobId) {
        Write-Host ("  {0,3:N0}s  submission queued..." -f $clock.Elapsed.TotalSeconds)
        Start-Sleep -Seconds $PollSeconds
    }
}
Write-Host "  Job created: $jobId"

Write-Host ''
Write-Host '--- Solving ---'
while ($true) {
    if ($clock.Elapsed.TotalSeconds -ge $TimeoutSeconds) {
        Write-Error "timed out after ${TimeoutSeconds}s waiting for the solve"
    }
    $job = Invoke-RestMethod "$BaseUrl/api/jobs/$jobId"
    if ($job.status -eq 'success') {
        Write-Host ("  Solved in {0:N1}s." -f $clock.Elapsed.TotalSeconds)
        break
    }
    if ($job.status -eq 'failure') {
        Write-Error ("solver failed after {0:N1}s" -f $clock.Elapsed.TotalSeconds)
    }
    Write-Host ("  {0,3:N0}s  status: {1}" -f $clock.Elapsed.TotalSeconds, $job.status)
    Start-Sleep -Seconds $PollSeconds
}

Write-Host ''
Write-Host '--- Calibration ---'
Invoke-RestMethod "$BaseUrl/api/jobs/$jobId/calibration" | ConvertTo-Json
