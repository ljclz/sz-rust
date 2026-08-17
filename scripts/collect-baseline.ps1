﻿$crates = @(
    "sz-rust-core",
    "sz-rust-http-facade",
    "sz-rust-cache-facade",
    "sz-rust-state-facade",
    "sz-rust-infra-facade",
    "sz-rust-auth-facade",
    "sz-rust-pay-facade",
    "sz-rust-orm-facade",
    "sz-rust-orm-ext-facade",
    "sz-rust-router-facade",
    "sz-rust-middleware-facade",
    "sz-rust-mvc-facade",
    "sz-rust-mcp",
    "sz-rust-facade-tests",
    "sz-rust-addons-loader",
    "sz-rust-addons-crm",
    "sz-rust-addons-ecommerce",
    "sz-rust-addons-cms",
    "sz-rust-cli",
    "sz-rust-observability",
    "sz-rust-sz300",
    "sz-rust-ai-facade",
    "sz-rust-capability",
    "sz-rust-rag",
    "sz-rust-frontend-codegen"
)

$cargo = "C:\Users\Administrator\.cargo\bin\cargo.exe"
$outDir = "target\coverage"
$results = @()

foreach ($crate in $crates) {
    Write-Host "`n=== Collecting coverage for $crate ===" -ForegroundColor Cyan
    $xmlPath = "$outDir\cobertura-$crate.xml"
    
    & $cargo llvm-cov -p $crate --cobertura --output-path $xmlPath 2>&1 | Out-Null
    
    if (Test-Path $xmlPath) {
        [xml]$xml = Get-Content $xmlPath
        $lineRate = [double]$xml.coverage.'line-rate'
        $branchRate = [double]$xml.coverage.'branch-rate'
        $linesValid = [int]$xml.coverage.'lines-valid'
        $linesCovered = [int]$xml.coverage.'lines-covered'
        $pct = [math]::Round($lineRate * 100, 2)
        
        $results += [PSCustomObject]@{
            Crate = $crate
            LineRate = $pct
            BranchRate = [math]::Round($branchRate * 100, 2)
            LinesCovered = $linesCovered
            LinesValid = $linesValid
        }
        Write-Host "  ${crate}: ${pct}% (${linesCovered}/${linesValid} lines)" -ForegroundColor Green
    } else {
        $results += [PSCustomObject]@{
            Crate = $crate
            LineRate = "FAILED"
            BranchRate = "N/A"
            LinesCovered = 0
            LinesValid = 0
        }
        Write-Host "  ${crate}: FAILED" -ForegroundColor Red
    }
}

Write-Host "`n========== BASELINE COVERAGE SUMMARY ==========" -ForegroundColor Yellow
$results | Format-Table -AutoSize

$totalCovered = ($results | Where-Object { $_.LineRate -ne "FAILED" } | Measure-Object -Property LinesCovered -Sum).Sum
$totalValid = ($results | Where-Object { $_.LineRate -ne "FAILED" } | Measure-Object -Property LinesValid -Sum).Sum
if ($totalValid -gt 0) {
    $overallPct = [math]::Round(($totalCovered / $totalValid) * 100, 2)
    Write-Host "`nOverall: $overallPct% ($totalCovered/$totalValid lines)" -ForegroundColor Yellow
}

$results | Export-Csv "$outDir\baseline-summary.csv" -NoTypeInformation
Write-Host "`nBaseline CSV saved to $outDir\baseline-summary.csv" -ForegroundColor Green