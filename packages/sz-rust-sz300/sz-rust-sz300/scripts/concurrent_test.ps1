param(
    [string]$TargetUrl = "http://localhost:8300/health",
    [int]$Concurrency = 10,
    [int]$DurationSec = 5
)

Write-Host "Concurrent Load Test"
Write-Host "Target: $TargetUrl"
Write-Host "Concurrency: $Concurrency"
Write-Host "Duration: ${DurationSec}s"
Write-Host ""

$runspacePool = [RunspaceFactory]::CreateRunspacePool(1, $Concurrency)
$runspacePool.Open()

$handles = @()
for ($i=0; $i -lt $Concurrency; $i++) {
    $ps = [PowerShell]::Create()
    $ps.RunspacePool = $runspacePool
    [void]$ps.AddScript({
        param($url, $duration)
        $endTime = [datetime]::Now.AddSeconds($duration)
        $count = 0
        while ([datetime]::Now -lt $endTime) {
            try {
                $r = Invoke-RestMethod -Uri $url -Method GET -TimeoutSec 2
                if ($r.code -eq 1) { $count++ }
            } catch {}
        }
        return $count
    }).AddArgument($TargetUrl).AddArgument($DurationSec)
    $handles += @{PowerShell=$ps; AsyncResult=$ps.BeginInvoke()}
}

$total = 0
$errors = 0
foreach ($item in $handles) {
    $result = $item.PowerShell.EndInvoke($item.AsyncResult)
    $total += $result
    $item.PowerShell.Dispose()
}
$runspacePool.Close()
$runspacePool.Dispose()

$qps = [math]::Round($total / $DurationSec, 1)
Write-Host "==================================="
Write-Host "Total requests: $total"
Write-Host "QPS: $qps"
Write-Host "==================================="

