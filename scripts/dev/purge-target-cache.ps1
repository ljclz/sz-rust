# Purge stale Cargo build cache periodically.
# Invoked daily by scheduled task "sz-rust-target-gc" (nightly); can also be run manually:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev\purge-target-cache.ps1 [-Days 7]
# Why: Cargo never garbage-collects target/. Dependency updates, toolchain upgrades
# and tool switches (check/clippy/llvm-cov/tarpaulin) invalidate whole layers of
# artifacts that stay on disk forever, so the cache only grows. Periodic purge is
# the fix; cost is a slower first build afterwards.
# HARD RULE: no cache may live on C:. All caches are on F: (cargo-target /
# cargo-target-ra / cargo-home / rustup-home); workspace target is the fallback.
param(
    [int]$Days = 7,
    [string[]]$Dirs = @(),
    [string]$LogPath = "F:\cargo-logs\target-gc.log"
)

$ErrorActionPreference = "SilentlyContinue"

# Default dirs: CLI cache, rust-analyzer cache, workspace fallback (derived from script location)
if ($Dirs.Count -eq 0) {
    $wsTarget = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path + "\target"
    $Dirs = @("F:\cargo-target", "F:\cargo-target-ra", $wsTarget)
}

function Get-DirSize([string]$p) {
    if (-not (Test-Path $p)) { return 0 }
    (Get-ChildItem $p -Recurse -Force -File | Measure-Object -Property Length -Sum).Sum
}

$cutoff = (Get-Date).AddDays(-$Days)
$before = 0
foreach ($d in $Dirs) { $before += Get-DirSize $d }

foreach ($d in $Dirs) {
    if (-not (Test-Path $d)) { continue }

    # incremental/: pure cache, delete unconditionally (only slows the next build)
    Get-ChildItem $d -Directory | ForEach-Object {
        $inc = Join-Path $_.FullName "incremental"
        if (Test-Path $inc) { Remove-Item $inc -Recurse -Force }
    }

    # Everything else: delete files older than $Days (cargo rebuilds missing artifacts)
    Get-ChildItem $d -Recurse -Force -File |
        Where-Object { $_.LastWriteTime -lt $cutoff } |
        Remove-Item -Force

    # Bottom-up removal of now-empty directories
    Get-ChildItem $d -Recurse -Force -Directory |
        Sort-Object { $_.FullName.Length } -Descending |
        Where-Object { -not (Get-ChildItem $_.FullName -Force) } |
        Remove-Item -Force
}

$after = 0
foreach ($d in $Dirs) { $after += Get-DirSize $d }
$freedMB = [math]::Round(($before - $after) / 1MB, 1)

$line = "{0}  freed {1} MB (purged artifacts older than {2} days + incremental, dirs: {3})" -f `
    (Get-Date -Format "yyyy-MM-dd HH:mm"), $freedMB, $Days, ($Dirs -join "; ")
New-Item -ItemType Directory -Force -Path (Split-Path $LogPath) | Out-Null
Add-Content -Path $LogPath -Value $line
Write-Output $line
