Write-Host "=== XianShiDa SZ-300 API Baseline Test ==="
Write-Host ""

# 1. Connectivity test
Write-Host "1. Connectivity Test:"
$r = Invoke-RestMethod -Uri "http://localhost:8300/health" -Method GET
Write-Host "   /health => status=$($r.data.status), code=$($r.code)"

$body = @{username="admin";password="123456"} | ConvertTo-Json
try {
    $r2 = Invoke-RestMethod -Uri "http://localhost:8300/api/v1/auth/login" -Method POST -Body $body -ContentType "application/json"
    Write-Host "   /auth/login => msg=$($r2.msg), code=$($r2.code)"
} catch {
    Write-Host "   /auth/login => FAILED: $_"
}

# 2. Latency test
Write-Host ""
Write-Host "2. Latency Test (50 sequential health calls):"
$times = @()
$sw = [System.Diagnostics.Stopwatch]::StartNew()
for ($i=0; $i -lt 50; $i++) {
    $t = [System.Diagnostics.Stopwatch]::StartNew()
    $r = Invoke-RestMethod -Uri "http://localhost:8300/health" -Method GET
    $t.Stop()
    $times += $t.ElapsedMilliseconds
}
$sw.Stop()

$avg = [math]::Round(($times | Measure-Object -Average).Average, 1)
$max = ($times | Measure-Object -Maximum).Maximum
$min = ($times | Measure-Object -Minimum).Minimum
$sorted = $times | Sort-Object
$p50 = $sorted[[math]::Floor(50*0.50)]
$p90 = $sorted[[math]::Floor(50*0.90)]
$p99 = $sorted[[math]::Floor(50*0.99)]

Write-Host "   Total: $($sw.ElapsedMilliseconds)ms"
Write-Host "   Requests: 50"
Write-Host "   Avg: $avg ms | Min: $min ms | Max: $max ms"
Write-Host "   P50: $p50 ms | P90: $p90 ms | P99: $p99 ms"
$qps = [math]::Round(50000 / $sw.ElapsedMilliseconds, 1)
Write-Host "   QPS: $qps (sequential)"

# 3. Concurrent test
Write-Host ""
Write-Host "3. Concurrent Test (10 connections, 5 seconds):"

$counter = 0
$lock = [System.Threading.Mutex]::new()
$tasks = @()

for ($c=0; $c -lt 10; $c++) {
    $tasks += [System.Threading.Tasks.Task]::Run({
        $end = [datetime]::Now.AddSeconds(5)
        $localCount = 0
        while ([datetime]::Now -lt $end) {
            try {
                $r = Invoke-RestMethod -Uri "http://localhost:8300/health" -Method GET
                if ($r.code -eq 1) { $localCount++ }
            } catch {
                # ignore errors
            }
        }
        $lock.WaitOne()
        $script:counter += $localCount
        $lock.ReleaseMutex()
    })
}

[System.Threading.Tasks.Task]::WaitAll($tasks)
Write-Host "   Concurrency: 10, Duration: 5 sec"
Write-Host "   Total requests: $counter"
$qps2 = [math]::Round($counter / 5, 1)
Write-Host "   QPS: $qps2"

Write-Host ""
Write-Host "===== Benchmark Complete ====="
