<#
.SYNOPSIS
    Windows 内存基线测试脚本

.DESCRIPTION
    运行 sz-rust-core 测试套件，采集测试前后的进程内存使用情况，
    生成 JSON 格式的内存基线报告，用于检测内存泄漏。

    流程：
      1. 记录测试前 cargo / rustc / node 进程的内存使用与系统可用内存
      2. 运行 cargo test -p sz-rust-core --lib
      3. 记录测试后的内存使用
      4. 对比测试前后内存增量，判断是否存在泄漏
      5. 生成 JSON 报告到 reports/memory_baseline_<timestamp>.json

.PARAMETER Package
    要测试的包名，默认 sz-rust-core

.PARAMETER LeakThresholdMB
    内存泄漏判定阈值（MB），默认 100。测试前后系统可用内存减少量
    超过此值则判定为疑似泄漏。

.EXAMPLE
    .\scripts\windows_memory_baseline.ps1
    .\scripts\windows_memory_baseline.ps1 -Package sz-rust-core -LeakThresholdMB 100
#>

param(
    [string]$Package = "sz-rust-core",
    [int]$LeakThresholdMB = 100
)

# 遇到错误立即停止
$ErrorActionPreference = "Stop"

# 切换到项目根目录（脚本位于 scripts/ 下，父目录即为项目根）
$ProjectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $ProjectRoot

# 创建报告输出目录
$ReportsDir = Join-Path $ProjectRoot "reports"
if (-not (Test-Path $ReportsDir)) {
    New-Item -ItemType Directory -Path $ReportsDir -Force | Out-Null
}

# 生成时间戳与报告路径
$Timestamp = Get-Date -Format "yyyyMMdd_HHmmss"
$ReportPath = Join-Path $ReportsDir "memory_baseline_${Timestamp}.json"

# -----------------------------------------------------------------------------
# 采集进程内存快照
# -----------------------------------------------------------------------------
# 捕获 cargo / rustc / node 及 sz-rust 相关测试进程的内存使用，
# 同时记录系统总内存与可用内存。
function Get-MemorySnapshot {
    param([string]$Label)

    # 匹配构建工具链与测试相关进程
    $ProcessPattern = "cargo|rustc|node|sz_rust|sz-rust"
    $ProcessList = @()

    $AllProcesses = Get-Process -ErrorAction SilentlyContinue
    foreach ($Proc in $AllProcesses) {
        if ($Proc.Name -match $ProcessPattern) {
            $ProcessList += [PSCustomObject]@{
                Name            = $Proc.Name
                Pid             = $Proc.Id
                WorkingSetMB    = [math]::Round($Proc.WorkingSet64 / 1MB, 2)
                PrivateMemoryMB = [math]::Round($Proc.PrivateMemorySize64 / 1MB, 2)
            }
        }
    }

    # 采集系统级内存信息（Win32_OperatingSystem 返回 KB）
    $OSInfo = Get-CimInstance Win32_OperatingSystem
    $TotalMemoryMB = [math]::Round($OSInfo.TotalVisibleMemorySize / 1024, 2)
    $FreeMemoryMB  = [math]::Round($OSInfo.FreePhysicalMemory / 1024, 2)

    return [PSCustomObject]@{
        Label           = $Label
        Timestamp       = (Get-Date).ToString("o")
        TotalMemoryMB   = $TotalMemoryMB
        FreeMemoryMB    = $FreeMemoryMB
        UsedMemoryMB    = [math]::Round($TotalMemoryMB - $FreeMemoryMB, 2)
        ProcessCount    = $ProcessList.Count
        Processes       = $ProcessList
    }
}

# -----------------------------------------------------------------------------
# 1. 采集测试前内存快照
# -----------------------------------------------------------------------------
Write-Host "===== 1. 采集测试前内存快照 =====" -ForegroundColor Cyan
$BeforeSnapshot = Get-MemorySnapshot -Label "before"
Write-Host "系统总内存:   $($BeforeSnapshot.TotalMemoryMB) MB"
Write-Host "可用内存:     $($BeforeSnapshot.FreeMemoryMB) MB"
Write-Host "已用内存:     $($BeforeSnapshot.UsedMemoryMB) MB"
Write-Host "相关进程数:   $($BeforeSnapshot.ProcessCount)"

# -----------------------------------------------------------------------------
# 2. 运行测试套件
# -----------------------------------------------------------------------------
Write-Host "`n===== 2. 运行 cargo test -p $Package --lib =====" -ForegroundColor Cyan
$TestStartTime = Get-Date

# 直接调用 cargo，捕获合并输出与退出码
$TestOutput = & cargo test -p $Package --lib 2>&1
$TestExitCode = $LASTEXITCODE
$TestOutput | Out-Host

$TestEndTime = Get-Date
$TestDurationSeconds = [math]::Round(($TestEndTime - $TestStartTime).TotalSeconds, 2)

Write-Host "测试退出码:   $TestExitCode"
Write-Host "测试耗时:     $TestDurationSeconds 秒"

# 测试后等待 2 秒，让操作系统回收已退出进程的内存
Start-Sleep -Seconds 2

# -----------------------------------------------------------------------------
# 3. 采集测试后内存快照
# -----------------------------------------------------------------------------
Write-Host "`n===== 3. 采集测试后内存快照 =====" -ForegroundColor Cyan
$AfterSnapshot = Get-MemorySnapshot -Label "after"
Write-Host "系统总内存:   $($AfterSnapshot.TotalMemoryMB) MB"
Write-Host "可用内存:     $($AfterSnapshot.FreeMemoryMB) MB"
Write-Host "已用内存:     $($AfterSnapshot.UsedMemoryMB) MB"
Write-Host "相关进程数:   $($AfterSnapshot.ProcessCount)"

# -----------------------------------------------------------------------------
# 4. 内存泄漏检测
# -----------------------------------------------------------------------------
# 对比测试前后系统可用内存增量。正值表示可用内存减少（被占用），
# 若减少量超过阈值则判定为疑似泄漏。
$MemoryDeltaMB = [math]::Round($BeforeSnapshot.FreeMemoryMB - $AfterSnapshot.FreeMemoryMB, 2)
$LeakDetected = $false

Write-Host "`n===== 4. 内存泄漏检测 =====" -ForegroundColor Cyan
Write-Host "可用内存变化: $MemoryDeltaMB MB（正值=减少，负值=增加）"
Write-Host "泄漏阈值:     $LeakThresholdMB MB"

if ($MemoryDeltaMB -gt $LeakThresholdMB) {
    $LeakDetected = $true
    Write-Host "⚠️ 检测到内存增量 $MemoryDeltaMB MB 超过阈值 $LeakThresholdMB MB，疑似内存泄漏" -ForegroundColor Yellow
} else {
    Write-Host "✅ 内存增量 $MemoryDeltaMB MB 在阈值 $LeakThresholdMB MB 范围内，未检测到泄漏" -ForegroundColor Green
}

# -----------------------------------------------------------------------------
# 5. 生成 JSON 报告
# -----------------------------------------------------------------------------
$Report = [PSCustomObject]@{
    ReportName         = "Windows Memory Baseline"
    GeneratedAt        = (Get-Date).ToString("o")
    Package            = $Package
    TestCommand        = "cargo test -p $Package --lib"
    TestExitCode       = $TestExitCode
    TestDurationSeconds = $TestDurationSeconds
    LeakThresholdMB    = $LeakThresholdMB
    MemoryDeltaMB      = $MemoryDeltaMB
    LeakDetected       = $LeakDetected
    BeforeSnapshot     = $BeforeSnapshot
    AfterSnapshot      = $AfterSnapshot
}

$Report | ConvertTo-Json -Depth 10 | Out-File -FilePath $ReportPath -Encoding utf8

Write-Host "`n===== 报告已生成 =====" -ForegroundColor Green
Write-Host "报告路径:     $ReportPath"
Write-Host "内存增量:     $MemoryDeltaMB MB"
Write-Host "泄漏判定:     $(if ($LeakDetected) { '⚠️ 疑似泄漏' } else { '✅ 正常' })"

# 若检测到泄漏，以非零退出码退出（便于 CI 集成时拦截）
if ($LeakDetected) {
    exit 1
}
