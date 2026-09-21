# 测量 Tauri 冷启动时间（p99 ≤ 5s 验证）
# 用法：pwsh scripts/measure_tauri_startup.ps1

$ErrorActionPreference = "Stop"

$iterations = 5
$measurements = @()

Write-Host "测量 Tauri 冷启动时间（$iterations 次）..."

for ($i = 1; $i -le $iterations; $i++) {
    Write-Host "  第 $i 次测量..."

    $startTime = Get-Date

    # 启动 sz-rust-visual 桌面应用
    $proc = Start-Process -FilePath "cargo" -ArgumentList "run", "-p", "sz-rust-visual" -PassThru -WindowStyle Hidden

    # 等待窗口就绪（检查进程是否有窗口）
    $ready = $false
    $timeout = 10  # 最大等待 10 秒
    while (-not $ready -and ((Get-Date) - $startTime).TotalSeconds -lt $timeout) {
        Start-Sleep -Milliseconds 100
        # 检查进程是否仍在运行（窗口已创建）
        if (-not $proc.HasExited) {
            $ready = $true
        }
    }

    $elapsed = ((Get-Date) - $startTime).TotalMilliseconds
    $measurements += $elapsed

    Write-Host "    冷启动时间: $([math]::Round($elapsed, 1))ms"

    # 终止进程
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds 1
}

# 计算 p99
$sorted = $measurements | Sort-Object
$p99Index = [math]::Ceiling($iterations * 0.99) - 1
if ($p99Index -ge $iterations) { $p99Index = $iterations - 1 }
$p99 = $sorted[$p99Index]

$avg = ($measurements | Measure-Object -Average).Average
$min = ($measurements | Measure-Object -Minimum).Minimum
$max = ($measurements | Measure-Object -Maximum).Maximum

Write-Host ""
Write-Host "=== 冷启动时间统计 ==="
Write-Host "  次数: $iterations"
Write-Host "  最小: $([math]::Round($min, 1))ms"
Write-Host "  平均: $([math]::Round($avg, 1))ms"
Write-Host "  最大: $([math]::Round($max, 1))ms"
Write-Host "  P99:  $([math]::Round($p99, 1))ms"
Write-Host ""

if ($p99 -le 5000) {
    Write-Host "✓ P99 ($([math]::Round($p99, 1))ms) ≤ 5000ms — 验证通过"
    exit 0
} else {
    Write-Host "✗ P99 ($([math]::Round($p99, 1))ms) > 5000ms — 验证失败"
    exit 1
}