<#
.SYNOPSIS
    sz300-server standalone process memory baseline (Iron Rule 9: startup RSS < 30MB)

.DESCRIPTION
    Builds (if needed) and starts the release sz300-server binary as an independent
    process, samples its RSS via Get-Process, probes /health and /health/ready,
    then writes a JSON baseline report.

    DB credentials are read from environment variables (never hardcoded):
      SZ300_DB_USER      (default: shop)
      SZ300_DB_PASSWORD  (required)
      SZ300_DB_NAME      (default: shop)
      SZ300_DB_HOST      (default: 127.0.0.1)
      SZ300_DB_PORT      (default: 3306)
      SZ300_JWT_SECRET   (required; also set SZ_JWT_SECRET for core validation)

.PARAMETER ServerExe
    Path to sz300-server release binary. Default: <CARGO_TARGET_DIR>/release/sz300-server.exe
    or <repo>/target/release/sz300-server.exe. If missing, builds via cargo.

.PARAMETER SampleCount
    Number of RSS samples. Default 6.

.PARAMETER OutputPath
    JSON report output path. Default: <repo>/reports/server-memory-baseline-<timestamp>.json

.EXAMPLE
    $env:SZ300_DB_PASSWORD='...'; $env:SZ300_JWT_SECRET='...'
    .\scripts\server_memory_baseline.ps1
#>

param(
    [string]$ServerExe = "",
    [int]$SampleCount = 6,
    [string]$OutputPath = ""
)

$ErrorActionPreference = 'Stop'

$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot

# ---- resolve binary ----
if (-not $ServerExe) {
    if ($env:CARGO_TARGET_DIR) {
        $candidate = Join-Path $env:CARGO_TARGET_DIR 'release\sz300-server.exe'
        if (Test-Path $candidate) { $ServerExe = $candidate }
    }
    if (-not $ServerExe) {
        $ServerExe = Join-Path $ProjectRoot 'target\release\sz300-server.exe'
    }
}
if (-not (Test-Path $ServerExe)) {
    Write-Host "Binary not found at $ServerExe. Building release sz300-server ..."
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'cargo not found on PATH. Set PATH/CARGO_HOME/CARGO_TARGET_DIR per your toolchain, or pass -ServerExe.'
    }
    Push-Location $ProjectRoot
    cargo build --release -p sz-rust-sz300 --bin sz300-server | Out-Host
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    Pop-Location
}

# ---- env ----
$env:SZ300_DB_USER = if ($env:SZ300_DB_USER) { $env:SZ300_DB_USER } else { 'shop' }
$env:SZ300_DB_NAME = if ($env:SZ300_DB_NAME) { $env:SZ300_DB_NAME } else { 'shop' }
$env:SZ300_DB_HOST = if ($env:SZ300_DB_HOST) { $env:SZ300_DB_HOST } else { '127.0.0.1' }
$env:SZ300_DB_PORT = if ($env:SZ300_DB_PORT) { $env:SZ300_DB_PORT } else { '3306' }
$env:SZ300_SERVER_HOST = if ($env:SZ300_SERVER_HOST) { $env:SZ300_SERVER_HOST } else { '127.0.0.1' }
$env:SZ300_SERVER_PORT = if ($env:SZ300_SERVER_PORT) { $env:SZ300_SERVER_PORT } else { '8300' }

if (-not $env:SZ300_DB_PASSWORD) { throw 'SZ300_DB_PASSWORD environment variable is required' }
if (-not $env:SZ300_JWT_SECRET)  { throw 'SZ300_JWT_SECRET environment variable is required' }
if (-not $env:SZ_JWT_SECRET)     { $env:SZ_JWT_SECRET = $env:SZ300_JWT_SECRET }

$outLog = Join-Path $env:TEMP 'sz300-server-baseline.out.log'
$errLog = Join-Path $env:TEMP 'sz300-server-baseline.err.log'
if (Test-Path $outLog) { Remove-Item $outLog -Force }
if (Test-Path $errLog) { Remove-Item $errLog -Force }

Write-Host "Starting $ServerExe ..."
$p = Start-Process -FilePath $ServerExe -PassThru -NoNewWindow `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog

Start-Sleep -Seconds 8

$alive = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if (-not $alive) {
    Write-Host 'Process exited! stderr tail:'
    if (Test-Path $errLog) { Get-Content $errLog -Tail 40 }
    throw 'sz300-server exited during startup'
}

$rssReadings = @()
for ($i = 0; $i -lt $SampleCount; $i++) {
    $proc = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
    if ($proc) {
        $rssMb = [math]::Round($proc.WorkingSet64 / 1MB, 2)
        $rssReadings += $rssMb
        Write-Host ("sample {0}: RSS={1} MB" -f ($i + 1), $rssMb)
    } else {
        Write-Host "sample $($i+1): process not found"
    }
    Start-Sleep -Seconds 2
}

$healthStatus = $null
$readyStatus = $null
try {
    $h = Invoke-WebRequest -Uri "http://$($env:SZ300_SERVER_HOST):$($env:SZ300_SERVER_PORT)/health" -UseBasicParsing -TimeoutSec 5
    $healthStatus = [string]$h.StatusCode
} catch {
    $healthStatus = 'FAILED: ' + $_.Exception.Message
}
try {
    $r = Invoke-WebRequest -Uri "http://$($env:SZ300_SERVER_HOST):$($env:SZ300_SERVER_PORT)/health/ready" -UseBasicParsing -TimeoutSec 5
    $readyStatus = [string]$r.StatusCode
} catch {
    if ($_.Exception.Response) {
        $readyStatus = [string][int]$_.Exception.Response.StatusCode
    } else {
        $readyStatus = 'FAILED: ' + $_.Exception.Message
    }
}
Write-Host "health=$healthStatus ready=$readyStatus"

$final = Get-Process -Id $p.Id -ErrorAction SilentlyContinue
if ($final) {
    Stop-Process -Id $p.Id -Force
} else {
    Write-Host 'Process exited before probes.'
    throw 'sz300-server exited during probes'
}

$min = ($rssReadings | Measure-Object -Minimum).Minimum
$max = ($rssReadings | Measure-Object -Maximum).Maximum
$avg = [math]::Round(($rssReadings | Measure-Object -Average).Average, 2)
$thresholdMb = 30.0
$pass = $max -lt $thresholdMb

$report = [PSCustomObject]@{
    ReportName       = 'sz300-server memory baseline'
    GeneratedAt     = (Get-Date).ToString('o')
    Binary       = $ServerExe
    BuildProfile = 'release'
    IronRule     = 'Rule 9: startup RSS < 30MB'
    ThresholdMB  = $thresholdMb
    Pass         = $pass
    RssSamplesMB = $rssReadings
    RssMinMB     = $min
    RssMaxMB     = $max
    RssAvgMB     = $avg
    Health       = $healthStatus
    Readiness    = $readyStatus
    Note         = 'Measured with MySQL connected (read-only smoke).'
}

if (-not $OutputPath) {
    $reportsDir = Join-Path $ProjectRoot 'reports'
    if (-not (Test-Path $reportsDir)) { New-Item -ItemType Directory -Path $reportsDir -Force | Out-Null }
    $ts = Get-Date -Format 'yyyyMMdd_HHmmss'
    $OutputPath = Join-Path $reportsDir "server-memory-baseline-$ts.json"
}
$report | ConvertTo-Json -Depth 6 | Out-File -FilePath $OutputPath -Encoding utf8
Write-Host "Report written to $OutputPath"
Write-Host "RSS min=$min MB max=$max MB avg=$avg MB -> $(if ($pass) { 'PASS (<30MB)' } else { 'FAIL (>=30MB)' })"

if (-not $pass) { exit 1 }