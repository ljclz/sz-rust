<#
.SYNOPSIS
    SZ-300 后端 HTTP 负载测试脚本
.DESCRIPTION
    使用内置 PowerShell cmdlet 对 SZ-300 后端 API 进行并发负载测试。
    支持 GET /health 和 POST /api/v1/auth/login 两个场景。
    报告响应时间 (P50/P90/P99)、成功率和 QPS。
.PARAMETER TargetUrl
    目标服务器基础 URL，默认 http://localhost:8300
.PARAMETER DurationSeconds
    测试持续时间（秒），默认 10
.PARAMETER Concurrency
    并发数，默认 5
.EXAMPLE
    .\scripts\load_test.ps1 -TargetUrl "http://localhost:8300" -DurationSeconds 30 -Concurrency 10
#>

param(
    [string]$TargetUrl = "http://localhost:8300",
    [int]$DurationSeconds = 10,
    [int]$Concurrency = 5
)

$ErrorActionPreference = "Stop"

# ─── 辅助函数 ───────────────────────────────────────────────────────────────

function Write-Timestamp {
    param([string]$Message)
    $time = Get-Date -Format "HH:mm:ss.fff"
    Write-Host "[$time] $Message"
}

function Measure-Request {
    param(
        [string]$Method,
        [string]$Url,
        [object]$Body = $null,
        [string]$Token = $null
    )
    $params = @{
        Method = $Method
        Uri    = $Url
        UseBasicParsing = $true
        TimeoutSec = 5
    }
    if ($Body) {
        $params["Body"] = ($Body | ConvertTo-Json)
        $params["ContentType"] = "application/json"
    }
    if ($Token) {
        $params["Headers"] = @{ "Authorization" = "Bearer $Token" }
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $response = Invoke-RestMethod @params
        $sw.Stop()
        return @{
            ElapsedMs = $sw.Elapsed.TotalMilliseconds
            Success   = $true
            Code      = $response.code
        }
    } catch {
        $sw.Stop()
        return @{
            ElapsedMs = $sw.Elapsed.TotalMilliseconds
            Success   = $false
            Code      = $_.Exception.Response.StatusCode.value__
        }
    }
}

function Get-Percentile {
    param([double[]]$SortedValues, [double]$Percentile)
    if ($SortedValues.Length -eq 0) { return 0 }
    $index = [math]::Max(0, [math]::Min($SortedValues.Length - 1,
        [int][math]::Ceiling($Percentile / 100 * $SortedValues.Length) - 1))
    return $SortedValues[$index]
}

# ─── 健康检查负载测试 ──────────────────────────────────────────────────────

function Run-HealthLoadTest {
    param(
        [string]$BaseUrl,
        [int]$Duration,
        [int]$Concurrency
    )

    Write-Timestamp "===== 开始健康检查压测 ====="
    Write-Timestamp "目标: $BaseUrl/health | 持续时间: ${Duration}s | 并发: $Concurrency"
    Write-Host ""

    $healthUrl = "$BaseUrl/health"
    $sync = [System.Collections.Hashtable]::Synchronized(@{})
    $sync.Results = [System.Collections.ArrayList]::new()
    $sync.StopTime = (Get-Date).AddSeconds($Duration)

    # 使用 RunspacePool 实现并发
    $runspacePool = [runspacefactory]::CreateRunspacePool(1, $Concurrency)
    $runspacePool.Open()
    $jobs = @()

    for ($i = 0; $i -lt $Concurrency; $i++) {
        $ps = [powershell]::Create()
        $ps.RunspacePool = $runspacePool
        $null = $ps.AddScript({
            param($Url, $SyncObj)
            while ((Get-Date) -lt $SyncObj.StopTime) {
                $sw = [System.Diagnostics.Stopwatch]::StartNew()
                try {
                    $resp = Invoke-RestMethod -Method Get -Uri $Url -UseBasicParsing -TimeoutSec 5
                    $sw.Stop()
                    $null = $SyncObj.Results.Add(@{
                        ElapsedMs = $sw.Elapsed.TotalMilliseconds
                        Success   = $true
                        Code      = $resp.code
                    })
                } catch {
                    $sw.Stop()
                    $null = $SyncObj.Results.Add(@{
                        ElapsedMs = $sw.Elapsed.TotalMilliseconds
                        Success   = $false
                        Code      = -1
                    })
                }
            }
        }).AddParameters(@($healthUrl, $sync))
        $jobs += $ps.BeginInvoke()
    }

    # 实时进度显示
    while ((Get-Date) -lt $sync.StopTime) {
        $remaining = [math]::Max(0, ($sync.StopTime - (Get-Date)).TotalSeconds)
        $count = $sync.Results.Count
        Write-Progress -Activity "健康检查压测进行中..." `
            -Status "已请求: $count 次 | 剩余: $([int]$remaining)s" `
            -PercentComplete (($Duration - $remaining) / $Duration * 100)
        Start-Sleep -Milliseconds 200
    }

    # 等待完成并清理
    for ($i = 0; $i -lt $Concurrency; $i++) {
        $null = $jobs[$i].AsyncWaitHandle.WaitOne(3000)
        $ps = [powershell]::Create()
        $ps.RunspacePool = $runspacePool
        $ps.Dispose()
    }
    $runspacePool.Close()
    $runspacePool.Dispose()

    Write-Progress -Activity "健康检查压测进行中..." -Completed

    # 汇总结果
    $total = $sync.Results.Count
    if ($total -eq 0) {
        Write-Warning "未收集到任何健康检查请求数据"
        return
    }

    $successCount = ($sync.Results | Where-Object { $_.Success }).Count
    $failCount = $total - $successCount
    $elapsedList = $sync.Results | ForEach-Object { $_.ElapsedMs }
    $sortedElapsed = $elapsedList | Sort-Object

    $avgMs = [math]::Round(($elapsedList | Measure-Object -Average).Average, 2)
    $minMs = [math]::Round($sortedElapsed[0], 2)
    $maxMs = [math]::Round($sortedElapsed[-1], 2)
    $p50  = [math]::Round((Get-Percentile $sortedElapsed 50), 2)
    $p90  = [math]::Round((Get-Percentile $sortedElapsed 90), 2)
    $p99  = [math]::Round((Get-Percentile $sortedElapsed 99), 2)
    $qps  = [math]::Round($total / $Duration, 1)

    Write-Host ""
    Write-Timestamp "===== 健康检查压测报告 ====="
    Write-Host "  总请求:        $total"
    Write-Host "  成功:          $successCount ($([math]::Round($successCount/$total*100,1))%)"
    if ($failCount -gt 0) {
        Write-Host "  失败:          $failCount (警告：存在失败请求)"
    }
    Write-Host "  QPS:           $qps"
    Write-Host "  延迟 (ms):"
    Write-Host "    平均:        $avgMs"
    Write-Host "    最小:        $minMs"
    Write-Host "    最大:        $maxMs"
    Write-Host "    P50:         $p50"
    Write-Host "    P90:         $p90"
    Write-Host "    P99:         $p99"
    Write-Host ""
}

# ─── 登录接口负载测试 ──────────────────────────────────────────────────────

function Run-LoginLoadTest {
    param(
        [string]$BaseUrl,
        [int]$Duration,
        [int]$Concurrency
    )

    Write-Timestamp "===== 开始登录接口压测 ====="
    Write-Timestamp "目标: $BaseUrl/api/v1/auth/login | 持续时间: ${Duration}s | 并发: $Concurrency"
    Write-Host ""

    $loginUrl = "$BaseUrl/api/v1/auth/login"
    $loginBody = @{ username = "admin"; password = "123456" }
    $sync = [System.Collections.Hashtable]::Synchronized(@{})
    $sync.Results = [System.Collections.ArrayList]::new()
    $sync.StopTime = (Get-Date).AddSeconds($Duration)

    $runspacePool = [runspacefactory]::CreateRunspacePool(1, $Concurrency)
    $runspacePool.Open()
    $jobs = @()

    for ($i = 0; $i -lt $Concurrency; $i++) {
        $ps = [powershell]::Create()
        $ps.RunspacePool = $runspacePool
        $null = $ps.AddScript({
            param($Url, $BodyJson, $SyncObj)
            while ((Get-Date) -lt $SyncObj.StopTime) {
                $sw = [System.Diagnostics.Stopwatch]::StartNew()
                try {
                    $resp = Invoke-RestMethod -Method Post -Uri $Url `
                        -Body $BodyJson -ContentType "application/json" `
                        -UseBasicParsing -TimeoutSec 5
                    $sw.Stop()
                    $null = $SyncObj.Results.Add(@{
                        ElapsedMs = $sw.Elapsed.TotalMilliseconds
                        Success   = $true
                        Code      = $resp.code
                    })
                } catch {
                    $sw.Stop()
                    $null = $SyncObj.Results.Add(@{
                        ElapsedMs = $sw.Elapsed.TotalMilliseconds
                        Success   = $false
                        Code      = -1
                    })
                }
            }
        }).AddParameters(@($loginUrl, ($loginBody | ConvertTo-Json), $sync))
        $jobs += $ps.BeginInvoke()
    }

    while ((Get-Date) -lt $sync.StopTime) {
        $remaining = [math]::Max(0, ($sync.StopTime - (Get-Date)).TotalSeconds)
        $count = $sync.Results.Count
        Write-Progress -Activity "登录压测进行中..." `
            -Status "已请求: $count 次 | 剩余: $([int]$remaining)s" `
            -PercentComplete (($Duration - $remaining) / $Duration * 100)
        Start-Sleep -Milliseconds 200
    }

    for ($i = 0; $i -lt $Concurrency; $i++) {
        $null = $jobs[$i].AsyncWaitHandle.WaitOne(3000)
    }
    $runspacePool.Close()
    $runspacePool.Dispose()

    Write-Progress -Activity "登录压测进行中..." -Completed

    $total = $sync.Results.Count
    if ($total -eq 0) {
        Write-Warning "未收集到任何登录请求数据"
        return
    }

    $successCount = ($sync.Results | Where-Object { $_.Success }).Count
    $failCount = $total - $successCount
    $elapsedList = $sync.Results | ForEach-Object { $_.ElapsedMs }
    $sortedElapsed = $elapsedList | Sort-Object

    $avgMs = [math]::Round(($elapsedList | Measure-Object -Average).Average, 2)
    $minMs = [math]::Round($sortedElapsed[0], 2)
    $maxMs = [math]::Round($sortedElapsed[-1], 2)
    $p50  = [math]::Round((Get-Percentile $sortedElapsed 50), 2)
    $p90  = [math]::Round((Get-Percentile $sortedElapsed 90), 2)
    $p99  = [math]::Round((Get-Percentile $sortedElapsed 99), 2)
    $qps  = [math]::Round($total / $Duration, 1)

    Write-Host ""
    Write-Timestamp "===== 登录接口压测报告 ====="
    Write-Host "  总请求:        $total"
    Write-Host "  成功:          $successCount ($([math]::Round($successCount/$total*100,1))%)"
    if ($failCount -gt 0) {
        Write-Host "  失败:          $failCount (警告：存在失败请求)"
    }
    Write-Host "  QPS:           $qps"
    Write-Host "  延迟 (ms):"
    Write-Host "    平均:        $avgMs"
    Write-Host "    最小:        $minMs"
    Write-Host "    最大:        $maxMs"
    Write-Host "    P50:         $p50"
    Write-Host "    P90:         $p90"
    Write-Host "    P99:         $p99"
    Write-Host ""
}

# ─── 主流程 ─────────────────────────────────────────────────────────────────

Write-Host "============================================"
Write-Host "  SZ-300 后端 HTTP 负载测试"
Write-Host "============================================"
Write-Host "  目标:       $TargetUrl"
Write-Host "  持续时间:   ${DurationSeconds}s"
Write-Host "  并发数:     $Concurrency"
Write-Host "============================================"
Write-Host ""

# 1. 快速连通性检查
Write-Timestamp "连通性检查: $TargetUrl/health"
try {
    $warmup = Invoke-RestMethod -Method Get -Uri "$TargetUrl/health" -UseBasicParsing -TimeoutSec 3
    Write-Timestamp "服务正常，code=$($warmup.code)"
} catch {
    Write-Warning "连不通目标服务器，请确认服务已启动: $TargetUrl"
    Write-Host "尝试运行: cd src && cargo run"
    exit 1
}
Write-Host ""

# 2. 健康检查压测
Run-HealthLoadTest -BaseUrl $TargetUrl -Duration $DurationSeconds -Concurrency $Concurrency

# 3. 登录接口压测
Run-LoginLoadTest -BaseUrl $TargetUrl -Duration $DurationSeconds -Concurrency $Concurrency

Write-Timestamp "===== 压测全部完成 ====="
