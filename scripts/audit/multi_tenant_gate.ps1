# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2024-2026 SZ-Rust Team
#
# T13.2 多租户 CI 门禁脚本 — 检查 std::fs / dead_code / unsafe / 直接读取 tenant_id
#
# 用法: powershell -ExecutionPolicy Bypass -File scripts/audit/multi_tenant_gate.ps1
# 返回: 0=通过, 1=失败

$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot/../.."
$failed = $false

function Check-Grep {
    param([string]$pattern, [string[]]$paths, [string]$ruleName)
    $found = $false
    foreach ($p in $paths) {
        $fullPath = Join-Path $root $p
        if (Test-Path $fullPath) {
            $matches = Get-ChildItem -Path $fullPath -Recurse -Filter "*.rs" -ErrorAction SilentlyContinue |
                Select-String -Pattern $pattern -ErrorAction SilentlyContinue
            if ($matches) {
                Write-Host "[FAIL] $ruleName : $pattern"(Get-ChildItem -Path $fullPath -Recurse -Filter "*.rs" | Select-String -Pattern $pattern | Select-Object -First 3 | ForEach-Object { "  $($_.Path):$($_.LineNumber): $($_.Line.Trim())" })
                $script:failed = $true
                $found = $true
            }
        }
    }
    if (-not $found) {
        Write-Host "[PASS] $ruleName"
    }
}

Write-Host "=== Multi-Tenant CI Gate ==="
Write-Host ""

# 检查 1: orm-facade tenant 模块禁止 std::fs
Check-Grep "std::fs" @("packages/sz-rust-orm-facade/src/tenant") "Check-1: no std::fs in orm-facade tenant"

# 检查 2: tenant 模块禁止 #[allow(dead_code)]
Check-Grep "#\[allow\(dead_code\)\]" @("packages/sz-rust-orm-facade/src/tenant", "packages/sz-rust-middleware-facade/src/tenant", "packages/sz-rust-middleware-facade/src/tenant_admin") "Check-2: no #[allow(dead_code)] in tenant modules"

# 检查 3: tenant 模块禁止 unsafe
Check-Grep "unsafe" @("packages/sz-rust-orm-facade/src/tenant", "packages/sz-rust-middleware-facade/src/tenant") "Check-3: no unsafe in tenant modules"

# 检查 4: middleware-facade tenant 模块禁止 std::fs
Check-Grep "std::fs" @("packages/sz-rust-middleware-facade/src/tenant", "packages/sz-rust-middleware-facade/src/tenant_admin") "Check-4: no std::fs in middleware tenant"

# 检查 5: 禁止 crate 级 #![allow(dead_code)]
Check-Grep "#!\[allow\(dead_code\)\]" @("packages/sz-rust-orm-facade/src", "packages/sz-rust-middleware-facade/src") "Check-5: no crate-level #![allow(dead_code)]"

Write-Host ""

# 检查 6: 公开 API 签名验证 — DataScopeUserContext::new 签名不变
$dsFile = Join-Path $root "packages/sz-rust-middleware-facade/src/data_scope.rs"
if (Test-Path $dsFile) {
    $content = Get-Content $dsFile -Raw
    if ($content -match "pub fn new\(user_id: i64\) -> Self") {
        Write-Host "[PASS] Check-6: DataScopeUserContext::new signature unchanged"
    } else {
        Write-Host "[FAIL] Check-6: DataScopeUserContext::new signature changed"
        $script:failed = $true
    }
}

# 检查 7: DataScopeError 现有 18 变体不变
$errorFile = Join-Path $root "packages/sz-rust-orm-facade/src/data_scope/error.rs"
if (Test-Path $errorFile) {
    $errorContent = Get-Content $errorFile -Raw
    $originalVariants = @(
        "MissingUserContext", "DeptTreeUnavailable", "InvalidRule", "UnsafeCustomCondition",
        "GeneratorNotFound", "RuleNotFound", "PolicyNotFound", "RuleFieldMissing",
        "CustomGeneratorNotFound", "GenerationConflict", "ConfigFileNotFound",
        "ConfigParseError", "PathNotAllowed", "ConfigFileTooLarge",
        "AdminRequired", "AuthRequired", "RequestBodyInvalid", "RateLimited"
    )
    $missingVariants = $originalVariants | Where-Object { $errorContent -notmatch $_ }
    if ($missingVariants.Count -eq 0) {
        Write-Host "[PASS] Check-7: DataScopeError 18 original variants preserved"
    } else {
        Write-Host "[FAIL] Check-7: Missing variants: $($missingVariants -join ', ')"
        $script:failed = $true
    }
}

# 检查 8: AC-28 业务 handler 禁止直接从 Query/Body 读取 tenant_id
$handlersFile = Join-Path $root "packages/sz-rust-middleware-facade/src/tenant_admin/handlers.rs"
if (Test-Path $handlersFile) {
    $handlerContent = Get-Content $handlersFile -Raw
    $requestStructs = @("CreateTenantRequest", "UpdateTenantRequest", "ListTenantsQuery", "SetConfigRequest")
    $violationFound = $false
    foreach ($struct in $requestStructs) {
        if ($handlerContent -match "struct $struct\s*\{[^}]*pub tenant_id") {
            Write-Host "[FAIL] Check-8: $struct contains tenant_id field (AC-28 violation)"
            $script:failed = $true
            $violationFound = $true
        }
    }
    if (-not $violationFound) {
        Write-Host "[PASS] Check-8: no request struct directly reads tenant_id (AC-28)"
    }
}

# 检查 9: AC-28 业务 handler 使用 Extension<TenantContext>
$demoFile = Join-Path $root "packages/sz-rust-examples/src/bin/multi_tenant_demo.rs"
if (Test-Path $demoFile) {
    $demoContent = Get-Content $demoFile -Raw
    if ($demoContent -match "Extension<TenantContext>") {
        Write-Host "[PASS] Check-9: business handler uses Extension<TenantContext> (AC-28)"
    } else {
        Write-Host "[FAIL] Check-9: business handler does not use Extension<TenantContext>"
        $script:failed = $true
    }
}

Write-Host ""

if ($failed) {
    Write-Host "=== CI Gate: FAILED ==="
    exit 1
} else {
    Write-Host "=== CI Gate: PASSED ==="
    exit 0
}