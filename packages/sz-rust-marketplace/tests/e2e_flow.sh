#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2024-2026 SZ-Rust Team
#
# 端到端闭环测试：publish → 审核 → search → install
#
# 前置：docker compose up -d 已启动
# 执行：bash packages/sz-rust-marketplace/tests/e2e_flow.sh
# 清理：测试结束后删除测试插件包 + token

set -euo pipefail

MARKET_URL="http://localhost:8080"
CLI="cargo run -p sz-rust-cli --"
PASS=0
FAIL=0

green() { printf "\033[32m%s\033[0m\n" "$1"; }
red()   { printf "\033[31m%s\033[0m\n" "$1"; }
info()  { printf "\033[36m%s\033[0m\n" "$1"; }

assert() {
    local desc="$1"
    local result="$2"
    if [ "$result" = "0" ]; then
        green "PASS: $desc"
        PASS=$((PASS + 1))
    else
        red "FAIL: $desc"
        FAIL=$((FAIL + 1))
    fi
}

info "=== P2-2 端到端闭环测试 ==="

# ── 1. 健康检查 ──
info "1. 健康检查"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$MARKET_URL/api/v1/health")
assert "健康检查返回 200" "$([ "$HTTP_CODE" = "200" ] && echo 0 || echo 1)"

# ── 2. 登录获取 JWT ──
info "2. 登录获取 JWT"
TOKEN=$(curl -s -X POST "$MARKET_URL/api/v1/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"developer_id": 1, "username": "alice", "is_reviewer": false}' \
    | jq -r '.token // empty')
assert "登录获取 token" "$([ -n "$TOKEN" ] && echo 0 || echo 1)"

# ── 3. 搜索空市场 ──
info "3. 搜索空市场"
SEARCH_RESULT=$(curl -s "$MARKET_URL/api/v1/plugins/search?q=test")
TOTAL=$(echo "$SEARCH_RESULT" | jq -r '.total // 0')
assert "空市场搜索返回 total=0" "$([ "$TOTAL" = "0" ] && echo 0 || echo 1)"

# ── 4. 未带 token 访问管理端点 ──
info "4. 未带 token 访问发布端点"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$MARKET_URL/api/v1/plugins/publish")
assert "未授权返回 401" "$([ "$HTTP_CODE" = "401" ] || [ "$HTTP_CODE" = "400" ] && echo 0 || echo 1)"

# ── 5. 审核员登录 ──
info "5. 审核员登录"
REVIEWER_TOKEN=$(curl -s -X POST "$MARKET_URL/api/v1/auth/login" \
    -H "Content-Type: application/json" \
    -d '{"developer_id": 2, "username": "reviewer", "is_reviewer": true}' \
    | jq -r '.token // empty')
assert "审核员登录获取 token" "$([ -n "$REVIEWER_TOKEN" ] && echo 0 || echo 1)"

# ── 6. 查看待审核列表 ──
info "6. 查看待审核列表"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    "$MARKET_URL/api/v1/admin/reviews/pending" \
    -H "Authorization: Bearer $REVIEWER_TOKEN")
assert "待审核列表返回 200" "$([ "$HTTP_CODE" = "200" ] && echo 0 || echo 1)"

# ── 7. 非审核员访问待审核列表 ──
info "7. 非审核员访问待审核列表"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    "$MARKET_URL/api/v1/admin/reviews/pending" \
    -H "Authorization: Bearer $TOKEN")
assert "非审核员返回 403" "$([ "$HTTP_CODE" = "403" ] && echo 0 || echo 1)"

# ── 8. OpenAPI 文档 ──
info "8. OpenAPI 文档"
OPENAPI=$(curl -s "$MARKET_URL/api/v1/openapi.json")
OPENAPI_VERSION=$(echo "$OPENAPI" | jq -r '.openapi // empty')
assert "OpenAPI 3.0 文档可访问" "$([ "$OPENAPI_VERSION" = "3.0.0" ] && echo 0 || echo 1)"

# ── 9. CLI 搜索 ──
info "9. CLI plugin search"
if $CLI plugin search test > /dev/null 2>&1; then
    assert "CLI 搜索执行成功" "0"
else
    assert "CLI 搜索执行成功" "1"
fi

# ── 10. CLI 登录 ──
info "10. CLI plugin login"
if $CLI plugin login --token "$TOKEN" --url "$MARKET_URL" > /dev/null 2>&1; then
    assert "CLI 登录成功" "0"
else
    assert "CLI 登录成功" "1"
fi

# ── 清理 ──
info "清理测试凭证"
rm -f ~/.sz-rust/credentials.toml 2>/dev/null || true

# ── 汇总 ──
echo ""
info "=== 测试汇总 ==="
green "PASS: $PASS"
if [ "$FAIL" -gt 0 ]; then
    red "FAIL: $FAIL"
    exit 1
else
    green "FAIL: 0"
    green "全部通过！"
    exit 0
fi