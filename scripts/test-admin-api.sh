#!/usr/bin/env bash
# ============================================================================
# Admin Monitor API 手动测试脚本
# 用途：启动 sz300 服务器后，测试 3 个 admin 端点 + RoleGuard 鉴权
#
# 前置条件（任选其一）：
#   A. Docker 一键启动（推荐）：
#        echo "SZ300_JWT_SECRET=mysecret" > .env
#        echo "SZ300_DB_PASSWORD=mysecret" >> .env
#        docker compose up -d mysql          # 启动 MySQL
#        cargo run -p sz-rust-sz300 --features admin --bin sz300-server
#   B. 已有 MySQL 运行中：
#        export SZ300_JWT_SECRET=mysecret
#        export SZ300_DB_HOST=127.0.0.1 SZ300_DB_PASSWORD=mysecret
#        cargo run -p sz-rust-sz300 --features admin --bin sz300-server
#
# 用法：
#   bash scripts/test-admin-api.sh              # 测试已运行的服务器
#   bash scripts/test-admin-api.sh --start      # 自动启动服务器（后台）并测试
# ============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
SERVER_BIN="$ROOT_DIR/target/debug/sz300-server"
PID_FILE="/tmp/sz300-test-admin.pid"

# 默认配置
PORT="${SZ300_SERVER_PORT:-8300}"
BASE_URL="http://127.0.0.1:${PORT}"
JWT_SECRET="${SZ300_JWT_SECRET:-test-admin-secret-2026}"
START_SERVER="${1:-}"

# ============================================================================
# 颜色输出
# ============================================================================
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
info()    { echo -e "${CYAN}[INFO]${NC} $*"; }
ok()      { echo -e "${GREEN}[PASS]${NC} $*"; }
fail()    { echo -e "${RED}[FAIL]${NC} $*"; }
section() { echo -e "\n${YELLOW}══ $* ══${NC}"; }

# ============================================================================
# JWT 令牌生成（Python pyjwt）
# ============================================================================
generate_token() {
    local username="$1"
    local roles="$2"   # JSON array string, e.g. '["admin"]'
    python3 - "$JWT_SECRET" "$username" "$roles" <<'PYEOF'
import sys, jwt, time
secret, username, roles = sys.argv[1], sys.argv[2], eval(sys.argv[3])
payload = {
    "sub": username,
    "iss": "sz300",
    "exp": int(time.time()) + 3600,
    "iat": int(time.time()),
    "roles": roles,
    "user_id": 999,
}
print(jwt.encode(payload, secret, algorithm="HS256"))
PYEOF
}

# ============================================================================
# HTTP 请求辅助
# ============================================================================
http_get() {
    local url="$1"
    local token="${2:-}"
    local extra_args=()
    if [[ -n "$token" ]]; then
        extra_args=(-H "Authorization: Bearer $token")
    fi
    curl -sS -w "\n__STATUS__:%{http_code}" "$url" "${extra_args[@]}" 2>/dev/null
}

check_endpoint() {
    local name="$1" url="$2" token="${3:-}" expect_status="${4:-200}"
    local raw resp status body
    raw=$(http_get "$url" "$token")
    status=$(echo "$raw" | grep "__STATUS__:" | sed 's/__STATUS__://')
    body=$(echo "$raw" | sed '/__STATUS__/d')
    if [[ "$status" == "$expect_status" ]]; then
        ok "$name → HTTP $status"
        echo "$body" | python3 -m json.tool 2>/dev/null || echo "$body"
        return 0
    else
        fail "$name → HTTP $status (期望 $expect_status)"
        echo "$body"
        return 1
    fi
}

# ============================================================================
# 启动服务器（可选）
# ============================================================================
cleanup() {
    if [[ -f "$PID_FILE" ]]; then
        pid=$(cat "$PID_FILE" 2>/dev/null || true)
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            info "停止测试服务器 (PID $pid)..."
            kill "$pid" 2>/dev/null || true
            sleep 1
            kill -9 "$pid" 2>/dev/null || true
        fi
        rm -f "$PID_FILE"
    fi
}
trap cleanup EXIT

if [[ "$START_SERVER" == "--start" ]]; then
    if [[ ! -x "$SERVER_BIN" ]]; then
        info "编译 sz300-server（admin feature）..."
        cargo build -p sz-rust-sz300 --features admin --bin sz300-server
    fi
    info "启动测试服务器（端口 $PORT，后台运行）..."
    export SZ300_JWT_SECRET="$JWT_SECRET"
    export SZ300_SERVER_PORT="$PORT"
    export SZ300_SERVER_HOST="127.0.0.1"
    # 数据库：若无真实 DB，服务器会启动失败；这里仅尝试
    nohup "$SERVER_BIN" > /tmp/sz300-test-admin.log 2>&1 &
    echo $! > "$PID_FILE"
    # 等待启动（最多 15s）
    for i in $(seq 1 15); do
        if curl -sf "${BASE_URL}/health" >/dev/null 2>&1; then
            ok "服务器已启动 (PID $(cat $PID_FILE))"
            break
        fi
        sleep 1
    done
    if ! curl -sf "${BASE_URL}/health" >/dev/null 2>&1; then
        fail "服务器启动失败，查看日志：/tmp/sz300-test-admin.log"
        tail -30 /tmp/sz300-test-admin.log 2>/dev/null || true
        exit 1
    fi
fi

# ============================================================================
# 预检：服务器可达性
# ============================================================================
section "预检 — 服务器健康检查"
if ! curl -sf "${BASE_URL}/health" >/dev/null 2>&1; then
    fail "服务器不可达 ($BASE_URL)"
    echo ""
    echo "请先启动服务器："
    echo "  SZ300_JWT_SECRET=your-secret cargo run -p sz-rust-sz300 --features admin --bin sz300-server"
    echo ""
    echo "或使用本脚本自动启动："
    echo "  bash scripts/test-admin-api.sh --start"
    exit 1
fi
ok "服务器健康检查通过 ($BASE_URL/health)"

# ============================================================================
# 生成测试令牌
# ============================================================================
section "生成测试 JWT 令牌"
TOKEN_ADMIN=$(generate_token "admin_user" '["admin", "user"]')
TOKEN_USER=$(generate_token "regular_user" '["user"]')
TOKEN_NO_ROLE=$(generate_token "guest" '[]')

echo "admin 令牌（前 40 字符）: ${TOKEN_ADMIN:0:40}..."
echo "普通用户令牌（前 40 字符）: ${TOKEN_USER:0:40}..."

# ============================================================================
# 测试 1：无令牌 → 401
# ============================================================================
section "测试 1 — 无令牌访问（期望 401）"
check_endpoint "GET /api/admin/server/info（无令牌）" \
    "${BASE_URL}/api/admin/server/info" "" 401 || true

# ============================================================================
# 测试 2：无效令牌 → 401
# ============================================================================
section "测试 2 — 无效令牌（期望 401）"
check_endpoint "GET /api/admin/server/info（无效令牌）" \
    "${BASE_URL}/api/admin/server/info" "invalid.token.here" 401 || true

# ============================================================================
# 测试 3：有令牌但无 admin 角色 → 403
# ============================================================================
section "测试 3 — 有令牌但无 admin 角色（期望 403）"
check_endpoint "GET /api/admin/server/info（普通用户）" \
    "${BASE_URL}/api/admin/server/info" "$TOKEN_USER" 403 || true

check_endpoint "GET /api/admin/db/pool（无角色）" \
    "${BASE_URL}/api/admin/db/pool" "$TOKEN_NO_ROLE" 403 || true

# ============================================================================
# 测试 4：admin 令牌 → 200（核心功能）
# ============================================================================
section "测试 4 — admin 令牌访问 3 个端点（期望 200）"
check_endpoint "GET /api/admin/server/info" \
    "${BASE_URL}/api/admin/server/info" "$TOKEN_ADMIN" 200 || true

check_endpoint "GET /api/admin/db/pool" \
    "${BASE_URL}/api/admin/db/pool" "$TOKEN_ADMIN" 200 || true

check_endpoint "GET /api/admin/redis/info" \
    "${BASE_URL}/api/admin/redis/info" "$TOKEN_ADMIN" 200 || true

# ============================================================================
# 测试 5：Redis 降级（无 ADMIN_REDIS_URL 时返回 connected=false）
# ============================================================================
section "测试 5 — Redis 降级响应"
raw=$(http_get "${BASE_URL}/api/admin/redis/info" "$TOKEN_ADMIN")
body=$(echo "$raw" | sed '/__STATUS__/d')
if echo "$body" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('connected') in (True,False)" 2>/dev/null; then
    ok "Redis 端点返回有效 JSON（connected=$(python3 -c "import sys,json; print(json.load(sys.stdin).get('connected'))" <<<"$body")）"
    echo "$body" | python3 -m json.tool
else
    fail "Redis 端点返回非预期内容: $body"
fi

# ============================================================================
# 汇总
# ============================================================================
section "测试完成"
echo "端点列表："
echo "  GET ${BASE_URL}/api/admin/server/info  — CPU/内存/磁盘/负载/Rust版本/主机名"
echo "  GET ${BASE_URL}/api/admin/db/pool      — 连接池 active/idle/max/usage"
echo "  GET ${BASE_URL}/api/admin/redis/info   — Redis 版本/模式/连接数/内存/角色"
echo ""
echo "鉴权规则：需 Bearer token 且 claims.roles 包含 \"admin\""
echo "  401 = 无令牌/令牌无效 | 403 = 令牌有效但缺 admin 角色 | 200 = 通过"
