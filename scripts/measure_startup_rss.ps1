<#
.SYNOPSIS
    启动内存 RSS 测量脚本（W5/W6 PB-3）
.DESCRIPTION
    启动 sz300 空载进程，等待 2 秒稳定，测量 RSS，终止进程，输出 RSS 数值与阈值对比结论。
.PARAMETER BinaryPath
    sz300 release 二进制路径（必填）
.PARAMETER ThresholdMB
    RSS 阈值（MB），默认 30
.EXAMPLE
    pwsh scripts/measure_startup_rss.ps1 -BinaryPath target/release/sz300 -ThresholdMB 30
#>
param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [int]$ThresholdMB = 30
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $BinaryPath)) {
    Write-Error "二进制文件不存在: $BinaryPath"
    exit 1
}

$startTime = Get-Date
$proc = Start-Process -FilePath $BinaryPath -PassThru -WindowStyle Hidden

Start-Sleep -Seconds 2

if ($proc.HasExited) {
    Write-Error "进程在测量前已退出，退出码: $($proc.ExitCode)"
    exit 1
}

$refreshed = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
if ($null -eq $refreshed) {
    Write-Error "无法获取进程信息"
    exit 1
}

$rssBytes = $refreshed.WorkingSet64
$rssMB = [math]::Round($rssBytes / 1024 / 1024, 2)

$proc.Kill()
$proc.WaitForExit(5000) | Out-Null

$elapsed = ((Get-Date) - $startTime).TotalSeconds

$conclusion = if ($rssMB -le $ThresholdMB) { "通过" } else { "未通过" }

Write-Output "===== 启动内存 RSS 测量结果 ====="
Write-Output "二进制路径: $BinaryPath"
Write-Output "RSS: $rssMB MB ($rssBytes bytes)"
Write-Output "阈值: $ThresholdMB MB"
Write-Output "结论: $conclusion"
Write-Output "测量耗时: $([math]::Round($elapsed, 2)) 秒"
Write-Output "来源: Get-Process.WorkingSet64 (PID $($proc.Id))"
Write-Output "=================================="

if ($rssMB -gt $ThresholdMB) {
    exit 1
}