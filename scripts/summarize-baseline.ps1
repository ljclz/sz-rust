﻿$dir = "target\coverage"
$files = Get-ChildItem $dir -Filter "cobertura-*.xml"
$results = @()
foreach ($f in $files) {
    [xml]$xml = Get-Content $f.FullName
    $lineRate = [double]$xml.coverage."line-rate"
    $linesValid = [int]$xml.coverage."lines-valid"
    $linesCovered = [int]$xml.coverage."lines-covered"
    $pct = [math]::Round($lineRate * 100, 2)
    $crate = $f.BaseName -replace "cobertura-", ""
    $results += [PSCustomObject]@{ Crate = $crate; LineRate = $pct; LinesCovered = $linesCovered; LinesValid = $linesValid }
}
$results = $results | Sort-Object Crate
$results | Format-Table -AutoSize
$totalC = ($results | Measure-Object -Property LinesCovered -Sum).Sum
$totalV = ($results | Measure-Object -Property LinesValid -Sum).Sum
$overall = [math]::Round(($totalC / $totalV) * 100, 2)
Write-Host ""
Write-Host "Overall: $overall% ($totalC/$totalV lines)" -ForegroundColor Yellow
$below85 = $results | Where-Object { $_.LineRate -lt 85 }
Write-Host ""
Write-Host "Crates below 85%:" -ForegroundColor Red
$below85 | Format-Table -AutoSize