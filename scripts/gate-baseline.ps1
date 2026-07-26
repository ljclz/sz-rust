# szrsql baseline scan - reports only, does not block
# Scans for: placeholder patterns, all-features compile
$ErrorActionPreference = "Continue"
$rootDir = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$reportFile = Join-Path $rootDir "gate-baseline-report.txt"

function Write-Header($title) {
    Write-Host "`n===== $title =====" -ForegroundColor Cyan
}

# === Gate 8: placeholder detection ===
Write-Header "Gate 8: Placeholder detection (todo!/unimplemented!/unreachable!)"

$count = 0
Get-ChildItem -Path $rootDir -Recurse -Filter "*.rs" -Exclude "*target*" | ForEach-Object {
    $content = Get-Content $_.FullName -Encoding UTF8
    $lineNum = 0
    foreach ($line in $content) {
        $lineNum++
        if ($line -match '\btodo!\b' -or $line -match '\bunimplemented!\b' -or $line -match '\bunreachable!\b') {
            Write-Host "  [MED] $($_.FullName):$lineNum" -ForegroundColor Yellow
            Write-Host "    >>> $($line.Trim())"
            $count++
        }
    }
}

if ($count -eq 0) {
    Write-Host "  [OK] No placeholder found" -ForegroundColor Green
} else {
    Write-Host "  [WARN] Found $count placeholders" -ForegroundColor Yellow
}

# === Gate 9: all-features compile ===
Write-Header "Gate 9: cargo check --all-features"

Push-Location $rootDir
$result = & cargo check --workspace --all-targets --all-features 2>&1 | Out-String
Pop-Location
if ($LASTEXITCODE -eq 0) {
    Write-Host "  [OK] cargo check --all-features passed" -ForegroundColor Green
} else {
    Write-Host "  [FAIL] cargo check --all-features failed" -ForegroundColor Red
    $result | Select-Object -First 20 | ForEach-Object { Write-Host "    $_" -ForegroundColor Red }
}

# === Summary ===
Write-Header "Baseline Summary"
$summary = @"
===== szrsql Baseline Scan Report =====
Date: $(Get-Date -Format "yyyy-MM-dd HH:mm:ss")
Gate 8 (placeholders): $count
Gate 9 (all-features): $(if ($LASTEXITCODE -eq 0) { "PASS" } else { "FAIL" })
Note: Baseline scan is non-blocking. Fix issues before enabling gate-commit.ps1.
"@
Write-Host $summary -ForegroundColor White
$summary | Out-File -FilePath $reportFile -Encoding utf8
Write-Host "Report saved to: $reportFile" -ForegroundColor Gray
