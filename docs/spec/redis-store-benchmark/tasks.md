# Redis 存储后端压测任务分解

> **任务编号**: P0-2
> **基线版本**: sz-rust v0.6.7
> **被测模块**: `packages/sz-rust-auth-facade/src/redis_store.rs`（646 行，只读不修改）
> **目标服务器**: 122.51.216.76（Redis 127.0.0.1:6379，无密码，经 SSH 隧道本地 16379 访问）
> **生成日期**: 2026-08-08
> **文档类型**: 任务分解（tasks.md）
> **上游规格**: `docs/spec/redis-store-benchmark/spec.md` + `design.md`

---

## 全局约束（适用于所有任务）

1. **禁止修改被测源码**：`packages/sz-rust-auth-facade/src/redis_store.rs` 只读；严禁触碰上游 `../sz-orm/` 任何文件。
2. **异步约束**：所有 `async fn` 必须 `Send + 'static`；禁止 `std::fs`，统一 `tokio::fs`。
3. **Windows 环境**：`CARGO_INCREMENTAL=0`；Rust 2024 Edition；Cargo Workspace。
4. **SSH 凭证**：私钥从本地 `deploy_key` 文件读取，统一使用 Node.js `ssh2` 包，禁止 `sshpass` / PowerShell 重定向。
5. **数据隔离**：压测 key 前缀必须为 `sso:bench:*`，禁止使用生产前缀 `sso:ver` / `sso:bl` / `sso:sessions`。
6. **产物释放**：本地临时二进制、临时脚本、上传到服务器的测试脚本，压测完成后必须及时删除。
7. **Redis 集成测试串行**：`--test-threads=1`。
8. **file:line 证据强制**：每条性能结论必须附被测代码 file:line 证据，行号必须真实存在。

---

## 1. 基础设施：配置与 SSH 隧道

### 1.1 创建压测配置文件 bench-config.json
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/bench-config.json` 创建配置文件，包含：`redisUrl=redis://127.0.0.1:16379`、`keyPrefix=sso:bench`、`concurrencyLevels=[10,100,500,1000]`、`totalRequests=100000`、`soakDurationSecs=600`、`soakConcurrency=200`、`mixedRatio="3:2:2:1:1:1"`、`server={host:122.51.216.76,port:22,username:root,privateKeyPath:deploy_key}`、`commandTimeout=2`、`connectionTimeout=3`
- [ ] 配置 11 条 PERF 红线阈值表（PERF-1~PERF-11），每条含 `id/operation/concurrency/qpsMin/p99MaxMs/errorRateMax`
- **验证命令**：`node -e "const c=require('./docs/spec/redis-store-benchmark/scripts/bench-config.json'); console.log(c.perfRedLines.length)"`
- **预期结果**：输出 `11`，且 JSON 合法可解析

### 1.2 实现 SSH 隧道模块 ssh-tunnel.js
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/lib/ssh-tunnel.js` 新增 ESM 模块，导出 `openTunnel({ sshClient, localPort, remoteHost, remotePort })`，返回 `{ server, close }`
- [ ] 内部用 `net.createServer` 监听本地 `localPort`，每个新连接通过 ssh2 `sshClient.forwardOut` 转发到服务器 `remoteHost:remotePort`
- [ ] `close()` 方法关闭 net.Server 并结束所有在途连接，返回 Promise<void>，失败不抛错（记录后 resolve）
- [ ] 处理 `EADDRINUSE`（本地端口占用）与 forwardOut 失败（服务器 Redis 不可达）异常
- **验证命令**：`node -e "import('./docs/spec/redis-store-benchmark/scripts/lib/ssh-tunnel.js').then(m=>console.log(typeof m.openTunnel))"`
- **预期结果**：输出 `function`

### 1.3 实现证据索引常量 evidence-index.js
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/lib/evidence-index.js` 新增 ESM 模块，导出 `REDIS_STORE_EVIDENCE` 常量对象
- [ ] 包含 19 个被测方法的 file:line 映射（对齐 spec.md 附录）：`get_version:142-154`、`increment_version:156-168`、`revoke:199-214`、`is_revoked:216-226`、`register_session:263-292`、`get_sessions:294-312`、`get_session:314-338`、`revoke_session:340-372`、`update_last_active:374-409`、`update_session_jti:411-448`、`cleanup_expired:450-497`、`clear_user_sessions:499-527`、`create_redis_stores:536-548`、`create_redis_stores_with_devices:554-572` 等
- [ ] 每条含 `{ file, line, cmd }` 三字段，`file` 为 `packages/sz-rust-auth-facade/src/redis_store.rs`
- **验证命令**：`node -e "import('./docs/spec/redis-store-benchmark/scripts/lib/evidence-index.js').then(m=>console.log(Object.keys(m.REDIS_STORE_EVIDENCE).length))"`
- **预期结果**：输出 `19`（或对应方法数）

### 1.4 复用并校验 P0-1 SSHOperator 与 EvidenceCollector
- [ ] 确认 `docs/spec/production-validation/scripts/lib/ssh-operator.js` 与 `evidence-collector.js` 可直接 import 复用（ESM 路径调整）
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/package.json` 声明 `ssh2 ^1.17.0` 依赖，并配置 `"type": "module"`
- [ ] 用 EvidenceCollector.verifyAll 校验 evidence-index.js 中 19 条证据的 file:line 真实性
- **验证命令**：`cd docs/spec/redis-store-benchmark/scripts && npm install && node -e "import('../production-validation/scripts/lib/evidence-collector.js').then(async m=>{const e=(await import('./lib/evidence-index.js')).REDIS_STORE_EVIDENCE; const r=await m.verifyAll(Object.values(e).map(x=>({file:x.file,line:x.line})),process.cwd()); console.log(r.passed+'/'+r.total)})"`
- **预期结果**：输出 `19/19`，全部证据行号真实存在

---

## 2. Rust 压测执行层核心框架

> 所有 Rust 代码放在 `packages/sz-rust-auth-facade/examples/bench-runner.rs`（单文件 example bin，内部用 mod 组织）。
> 编译命令：`cargo build --example bench-runner --features redis-store`（CARGO_INCREMENTAL=0）。

### 2.1 创建 bench-runner example 入口与 CLI 参数解析
- [ ] 在 `packages/sz-rust-auth-facade/examples/bench-runner.rs` 创建 example bin，`#[tokio::main] async fn main()`
- [ ] 手写 `env::args` 解析（避免引入 clap 依赖），参数：`--op`（枚举）、`--concurrency`（u16）、`--total`（u64）、`--redis-url`（String）、`--prefix`（String，必须为 `sso:bench`）、`--soak-secs`（u64）、`--mixed-ratio`（String `3:2:2:1:1:1`）、`--devices-per-user`（u16，默认 10）
- [ ] 参数校验：`--prefix` 非 `sso:bench` 时退出码 2；`--concurrency` 超过 2000 时退出码 2
- [ ] 根据 `--op` 分发到对应压测函数，结果以 JSON 写 stdout（serde_json::to_string）
- **验证命令**：`cargo build --example bench-runner --features redis-store`
- **预期结果**：编译成功，无 warning，产物在 `target/debug/examples/bench-runner.exe`

### 2.2 实现并发度控制 ConcurrencyDriver
- [ ] 在 bench-runner.rs 内实现 `async fn run_concurrent<F, Fut, T>(concurrency: u16, total: u64, f: F) -> Vec<Result<T>>`，其中 `F: Fn(u64) -> Fut + Send + Sync + 'static`，`Fut: Future<Output = Result<T>> + Send + 'static`，`T: Send + 'static`
- [ ] 用 `tokio::sync::Semaphore` 控制在途任务数，`Arc<Semaphore>` clone 共享；用 `Vec<JoinHandle>` 收集任务句柄
- [ ] 用 `Arc<dyn RefreshTokenStore>` 等 clone 共享被测 Store（ConnectionManager 内部 Arc 共享连接池）
- [ ] 记录每请求起止 `Instant::now()`，收集延迟样本 `Vec<Duration>`（u64 纳秒）
- **验证命令**：`cargo build --example bench-runner --features redis-store`
- **预期结果**：编译通过，`async fn` 满足 `Send + 'static`（无编译错误即满足）

### 2.3 实现延迟分位数统计 LatencyHistogram
- [ ] 在 bench-runner.rs 内实现 `fn percentiles(samples: &mut [u64]) -> (f64, f64, f64)`，返回 (p50, p95, p99) 毫秒
- [ ] 对 samples 排序后取分位索引：`idx = (len * p / 100).min(len - 1)`，转换为毫秒（纳秒 / 1_000_000.0）
- [ ] 断言返回值满足 `p50 ≤ p95 ≤ p99`（浮点容差 0.001）
- [ ] 单轮 100000 样本内存占用 ≤ 10MB（8 字节 × 100000 = 0.8MB，满足）
- **验证命令**：`cargo test --example bench-runner --features redis-store bench_runner::tests::percentiles_test`
- **预期结果**：单元测试通过，验证 p50 ≤ p95 ≤ p99

### 2.4 实现错误分类计数 ErrorClassifier
- [ ] 在 bench-runner.rs 内实现 `struct ErrorClassifier { counts: HashMap<String, u64> }`，方法 `record(&mut self, err: &RefreshTokenError)` 和 `error_rate(&self, total: u64) -> f64`
- [ ] `record` 用 `match` 匹配 RefreshTokenError 变体名（`ServiceUnavailable` / `Cache` / `NotFound` / `InvalidToken` 等），HashMap 计数
- [ ] `error_breakdown` 各值之和 / total = error_rate，断言 error_rate ∈ [0.0, 1.0]
- **验证命令**：`cargo test --example bench-runner --features redis-store bench_runner::tests::error_classifier_test`
- **预期结果**：单元测试通过，error_rate 计算正确

### 2.5 实现 stdout JSON 输出 JsonSink
- [ ] 在 bench-runner.rs 内实现 `fn emit_round_result(result: &RoundResult)`，用 `serde_json::to_string` 写 stdout（println!）
- [ ] JSON 字段对齐 spec §6.2：`operation/concurrency/qps/latency_p50_ms/latency_p95_ms/latency_p99_ms/error_rate/error_breakdown/total_requests/duration_secs/rss_peak_kb/rss_start_kb/evidence_file/evidence_line/verdict/consistency_check`
- [ ] Soak 模式实现 `fn emit_soak_snapshot(snapshot: &SoakSnapshot)`，每行一个 JSON（流式输出）
- [ ] Soak 结束输出 `soak_summary` 汇总行，含 `qps_stable/p99_stable/memory_ok` 判定
- **验证命令**：`cargo build --example bench-runner --features redis-store`
- **预期结果**：编译通过，RoundResult / SoakSnapshot 结构体可序列化

### 2.6 实现数据一致性校验 ConsistencyChecker
- [ ] 在 bench-runner.rs 内实现 `async fn check_version_consistency(store: &Arc<dyn RefreshTokenStore>, user_id: i64, expected: u64) -> ConsistencyCheck`，调用 `get_version` 比对最终版本号
- [ ] 实现 `async fn check_sessions_consistency(store: &Arc<dyn DeviceSessionStore>, user_id: i64, expected_count: usize) -> ConsistencyCheck`，调用 `get_sessions` 比对记录数
- [ ] 实现 `async fn check_blacklist_consistency(store: &Arc<dyn TokenBlacklist>, jtis: &[String]) -> ConsistencyCheck`，逐个 `is_revoked` 验证全部命中
- [ ] 并发无丢失更新校验：收集 1000 个 increment 返回值，断言集合 = {1..1000}（用 HashSet 去重比对）
- **验证命令**：`cargo test --example bench-runner --features redis-store bench_runner::tests::consistency_test -- --test-threads=1`
- **预期结果**：单元测试通过（需 Redis 连接，串行运行）

### 2.7 实现 Soak 快照采样 SoakSampler
- [ ] 在 bench-runner.rs 内实现 `struct SoakSampler { start_rss: u64, snapshots: Vec<SoakSnapshot> }`
- [ ] 用 `tokio::time::interval(Duration::from_secs(60))` 每分钟触发快照，记录当前累计 QPS / p99 / RSS / error_rate
- [ ] RSS 读取：Windows 用 `sysinfo` crate（或 `GetProcessMemoryInfo` Win32 API）；启动时记录基线 `rss_start_kb`
- [ ] 结束时计算：`qps_stable = snapshots[9].qps ≥ snapshots[0].qps * 0.8`、`p99_stable = snapshots[9].p99 ≤ snapshots[0].p99 * 2`、`memory_ok = (rss_peak - rss_start ≤ 51200) && (rss_end - rss_start ≤ 30720)`
- [ ] 在 Cargo.toml `[dev-dependencies]` 添加 `sysinfo = { workspace = true }`（若 workspace 已有）
- **验证命令**：`cargo build --example bench-runner --features redis-store`
- **预期结果**：编译通过，SoakSampler 结构体可用

---

## 3. 版本号存储压测（spec §5.1，PERF-1~4）

### 3.1 实现 increment_version 压测（PERF-1/2/3）
- [ ] 在 bench-runner.rs 实现 `async fn bench_increment_version(config: &BenchConfig, concurrency: u16, total: u64) -> RoundResult`
- [ ] 预清理：删除 `sso:bench:ver:{user_id}` key
- [ ] 用 `create_redis_stores` 创建 `Arc<dyn RefreshTokenStore>`，key_prefix 设为 `sso:bench:ver`
- [ ] 并发调用 `increment_version(user_id)` 共 total 次，收集延迟样本与错误分类
- [ ] 压测后调用 `get_version` 校验最终版本号 = total（ConsistencyChecker）
- [ ] 附证据 `evidence_file=redis_store.rs, evidence_line=156-168`
- [ ] PERF 红线判定：并发 100 → QPS≥8000/p99≤5ms/err≤0.01%；并发 500 → QPS≥20000/p99≤15ms/err≤0.05%；并发 1000 → QPS≥30000/p99≤30ms/err≤0.1%
- **验证命令**：`target/debug/examples/bench-runner.exe --op increment_version --concurrency 100 --total 100000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout 输出合法 JSON，`verdict=pass`，`consistency_check.passed=true`，`final_version=100000`

### 3.2 实现 get_version 压测（PERF-4）
- [ ] 在 bench-runner.rs 实现 `async fn bench_get_version(config, concurrency, total) -> RoundResult`
- [ ] 预置：`increment_version(user_id)` 42 次，使版本号 = 42
- [ ] 并发调用 `get_version(user_id)` 共 total 次，验证每次返回值 = 42
- [ ] 附证据 `evidence_line=142-154`
- [ ] PERF-4 红线：并发 1000 → QPS≥40000/p99≤10ms/err≤0.01%
- **验证命令**：`target/debug/examples/bench-runner.exe --op get_version --concurrency 1000 --total 100000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`，所有返回值 = 42

### 3.3 实现并发无丢失更新校验（spec §5.1.1 规则 3/4）
- [ ] 在 bench-runner.rs 实现 `async fn bench_concurrent_no_lost_update(config) -> RoundResult`
- [ ] 并发 1000 个任务各调用 `increment_version(user_id)` 一次，收集 1000 个返回值
- [ ] 校验：最终版本号 = 1000，返回值集合（HashSet）= {1, 2, ..., 1000}，无重复无丢失
- [ ] 跨用户干扰校验：user_id=1 和 user_id=2 各并发 100 次 increment，断言互不影响（版本号各 = 100）
- **验证命令**：`target/debug/examples/bench-runner.exe --op concurrent_increment --concurrency 1000 --total 1000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `consistency_check.passed=true`，返回值集合大小 = 1000

---

## 4. Token 黑名单压测（spec §5.2，PERF-5~6）

### 4.1 实现 is_revoked 压测（PERF-5）
- [ ] 在 bench-runner.rs 实现 `async fn bench_is_revoked(config, concurrency, total) -> RoundResult`
- [ ] 预置 K=1000 个 jti 入黑名单：`revoke(jti, 3600)`
- [ ] 并发调用 `is_revoked(jti)` 共 total 次，命中与非命中各 50%（预生成 jti 序列，一半命中一半未命中）
- [ ] 分别统计命中与未命中两组延迟（spec §5.2.3 异常 2）
- [ ] 附证据 `evidence_line=216-226`
- [ ] PERF-5 红线：并发 1000 → QPS≥40000/p99≤10ms/err≤0.01%
- **验证命令**：`target/debug/examples/bench-runner.exe --op is_revoked --concurrency 1000 --total 100000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`，含命中/未命中分组延迟

### 4.2 实现 revoke 压测（PERF-6）
- [ ] 在 bench-runner.rs 实现 `async fn bench_revoke(config, concurrency, total) -> RoundResult`
- [ ] 并发调用 `revoke(jti, 3600)` 共 total 次（每次 jti 唯一）
- [ ] 压测后逐个 `is_revoked` 验证全部命中
- [ ] 监控 Redis `INFO memory` used_memory 增长（spec §5.2.3 异常 1），若增长 > 100MB 标注告警
- [ ] 附证据 `evidence_line=199-214`
- [ ] PERF-6 红线：并发 500 → QPS≥15000/p99≤15ms/err≤0.05%
- **验证命令**：`target/debug/examples/bench-runner.exe --op revoke --concurrency 500 --total 50000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`，事后全部 jti 命中黑名单

### 4.3 实现 TTL 过期与零 TTL 验证（spec §5.2.1 规则 3/4）
- [ ] 在 bench-runner.rs 实现 `async fn bench_ttl_validation(config) -> RoundResult`
- [ ] TTL 过期：`revoke(jti, 1)` 后 `tokio::time::sleep(2s)`，断言 `is_revoked(jti)` = false
- [ ] 零 TTL：`revoke(jti, 0)`，断言 Redis 无对应 key（`is_revoked` = false，no-op 不写入）
- **验证命令**：`target/debug/examples/bench-runner.exe --op ttl_validation --concurrency 1 --total 1 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `consistency_check.passed=true`，TTL 过期与零 TTL 均符合预期

---

## 5. 设备会话存储压测（spec §5.3，PERF-7~11）

### 5.1 实现 register_session 压测（PERF-7）
- [ ] 在 bench-runner.rs 实现 `async fn bench_register_session(config, concurrency, total) -> RoundResult`
- [ ] 用 `create_redis_stores_with_devices` 创建三元组 Store，key_prefix 设为 `sso:bench:sessions`
- [ ] 并发调用 `register_session(user_id, device_id, info, jti, access_jti)` 共 total 次（每次 device_id 唯一）
- [ ] 压测后 `get_sessions(user_id)` 校验返回记录数 = total
- [ ] 附证据 `evidence_line=263-292`
- [ ] PERF-7 红线：并发 500 → QPS≥10000/p99≤20ms/err≤0.05%
- **验证命令**：`target/debug/examples/bench-runner.exe --op register_session --concurrency 500 --total 50000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`，`consistency_check.passed=true`，记录数 = 50000

### 5.2 实现 get_session 压测（PERF-8）
- [ ] 在 bench-runner.rs 实现 `async fn bench_get_session(config, concurrency, total) -> RoundResult`
- [ ] 预置 K=1000 个设备会话，并发调用 `get_session(user_id, device_id)` 共 total 次（命中与非命中各 50%）
- [ ] 附证据 `evidence_line=314-338`
- [ ] PERF-8 红线：并发 1000 → QPS≥20000/p99≤15ms/err≤0.05%
- **验证命令**：`target/debug/examples/bench-runner.exe --op get_session --concurrency 1000 --total 100000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`

### 5.3 实现 get_sessions 批量读取压测（PERF-9）
- [ ] 在 bench-runner.rs 实现 `async fn bench_get_sessions(config, concurrency, total, devices_per_user) -> RoundResult`
- [ ] 预置单 user_id 下 D 个设备会话，并发调用 `get_sessions(user_id)` 共 total 次，验证每次返回 D 条
- [ ] D=10 场景：PERF-9 红线 并发 500 → QPS≥5000/p99≤30ms
- [ ] D=100 场景：并发 100 → p99≤100ms（验证大批量反序列化，spec §5.3.3 异常 1）
- [ ] 附证据 `evidence_line=294-312`
- **验证命令**：`target/debug/examples/bench-runner.exe --op get_sessions --concurrency 500 --total 10000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench --devices-per-user 10`
- **预期结果**：stdout JSON `verdict=pass`，每次返回 10 条记录

### 5.4 实现 revoke_session 压测（PERF-10）
- [ ] 在 bench-runner.rs 实现 `async fn bench_revoke_session(config, concurrency, total) -> RoundResult`
- [ ] 预置 K 个设备会话，并发调用 `revoke_session` 共 K 次（每次 device_id 唯一）
- [ ] 压测后 `get_sessions` 校验返回空
- [ ] 附证据 `evidence_line=340-372`
- [ ] PERF-10 红线：并发 500 → QPS≥8000/p99≤20ms/err≤0.05%
- **验证命令**：`target/debug/examples/bench-runner.exe --op revoke_session --concurrency 500 --total 10000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`，事后记录数 = 0

### 5.5 实现 update_last_active 压测（PERF-11）
- [ ] 在 bench-runner.rs 实现 `async fn bench_update_last_active(config, concurrency, total) -> RoundResult`
- [ ] 预置 K=100 个设备会话，并发对同一 user_id 不同 device_id 调用 `update_last_active` 共 total 次
- [ ] 压测后校验 last_active 时间戳 > 压测开始时间（spec §5.3.3 异常 3：并发非原子，取最终值）
- [ ] 附证据 `evidence_line=374-409`
- [ ] PERF-11 红线：并发 500 → QPS≥8000/p99≤20ms/err≤0.05%
- **验证命令**：`target/debug/examples/bench-runner.exe --op update_last_active --concurrency 500 --total 50000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `verdict=pass`，last_active 已更新

### 5.6 实现 JSON 序列化开销量化（spec §5.3.1 规则 6）
- [ ] 在 bench-runner.rs 实现 `async fn bench_json_overhead(config, concurrency, total) -> RoundResult`
- [ ] 对比同并发度下 `register_session`（含 serde_json 序列化）与纯 redis HSET（绕过 Store，直连 redis crate）的耗时差
- [ ] 计算 JSON 序列化占比 = (register_total - hset_total) / register_total × 100%
- [ ] 同理对比 `get_session`（含反序列化）与纯 HGET 基准
- [ ] 报告中标注"JSON 序列化预估占比 X%"
- **验证命令**：`target/debug/examples/bench-runner.exe --op json_overhead --concurrency 500 --total 50000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON 含 `json_serialize_ratio` 与 `json_deserialize_ratio` 百分比字段

---

## 6. 混合负载压测（spec §5.4）

### 6.1 实现混合操作比例分发
- [ ] 在 bench-runner.rs 实现 `async fn bench_mixed(config, concurrency, total, ratio) -> RoundResult`
- [ ] 解析 `--mixed-ratio 3:2:2:1:1:1`，预生成 total 个操作类型的确定性随机序列（固定种子，可复现）
- [ ] 比例：30% increment_version + 20% get_version + 20% is_revoked + 10% revoke + 10% register_session + 10% get_session
- [ ] 并发分发到三 Store，按操作类型分别统计 qps/p99/error_rate（`by_op` 字段）
- [ ] 附证据：各子操作对应 file:line
- [ ] 验收：并发 500，M=100000 → 整体 QPS≥12000/p99≤30ms/err≤0.05%，各子类指标分列
- **验证命令**：`target/debug/examples/bench-runner.exe --op mixed --concurrency 500 --total 100000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench --mixed-ratio 3:2:2:1:1:1`
- **预期结果**：stdout JSON `operation=mixed`，含 `by_op` 对象，各子操作独立指标

### 6.2 实现混合负载数据一致性校验（spec §5.4.1 规则 2）
- [ ] 在 bench-runner.rs 混合压测结束后调用 ConsistencyChecker
- [ ] 校验：increment 总次数 = 最终版本号、register 总次数 = 最终设备会话数、revoke 的 jti 事后全部命中黑名单
- [ ] 跨用户会话干扰校验（spec §5.3.1 规则 7）：user_id=1 和 user_id=2 各注册 100 设备，`get_sessions` 互不影响
- **验证命令**：同 6.1 验证命令
- **预期结果**：stdout JSON `consistency_check.passed=true`，版本号/会话数/黑名单命中均一致

---

## 7. 连接池稳定性压测（spec §5.5）

### 7.1 实现 5 分钟持续施压与 ServiceUnavailable 统计
- [ ] 在 bench-runner.rs 实现 `async fn bench_pool_stability(config, concurrency=1000, duration=300) -> RoundResult`
- [ ] 三 Store 并发 spawn（increment_version / is_revoked / register_session 交替），持续 5 分钟
- [ ] 统计 `ServiceUnavailable` 错误占比，验收：≤ 0.1%
- [ ] 记录 Redis `INFO connected_clients` 峰值，若接近 max-clients（默认 10000）标注告警（spec §5.5.3 异常 2）
- **验证命令**：`target/debug/examples/bench-runner.exe --op pool_stability --concurrency 1000 --total 0 --redis-url redis://127.0.0.1:16379 --prefix sso:bench --soak-secs 300`
- **预期结果**：stdout JSON `service_unavailable_rate ≤ 0.001`，`verdict=pass`

### 7.2 实现 SSH 隧道中断恢复验证
- [ ] 在 Node.js 编排层实现：压测启动后 60s，关闭隧道 5s，再重建隧道
- [ ] Rust 二进制持续施压，记录成功率时序（每秒一个采样点）
- [ ] 验收：隧道恢复后 10 秒内成功率 ≥ 99%（spec §5.5.1 规则 2）
- [ ] 报告中标注中断时间窗、错误峰值、恢复耗时
- **验证命令**：`node docs/spec/redis-store-benchmark/scripts/bench-tunnel-recovery.js`
- **预期结果**：输出恢复后 10 秒内成功率 ≥ 99%，ConnectionManager 自动重连无需重启

### 7.3 实现三 Store 共享连接无死锁验证
- [ ] 在 bench-runner.rs 实现 `async fn bench_shared_pool_no_deadlock(config) -> RoundResult`
- [ ] 用 `create_redis_stores_with_devices` 创建共享 ConnectionManager 的三 Store
- [ ] 三 Store 并发各 10000 次（increment / is_revoked / register_session），`join_all` 超时 30s
- [ ] 验收：全部在 30s 内完成，无死锁、无永久阻塞（spec §5.5.1 规则 3）
- **验证命令**：`target/debug/examples/bench-runner.exe --op shared_pool --concurrency 500 --total 10000 --redis-url redis://127.0.0.1:16379 --prefix sso:bench`
- **预期结果**：stdout JSON `duration_secs < 30`，`verdict=pass`，无死锁

---

## 8. Soak 长时间测试（spec §5.6）

### 8.1 实现 10 分钟混合负载 Soak
- [ ] 在 bench-runner.rs 实现 `async fn bench_soak(config, concurrency=200, duration=600, ratio) -> SoakSummary`
- [ ] 启动 ConcurrencyDriver（并发 200，混合负载 3:2:2:1:1:1）
- [ ] 记录启动 RSS 基线，用 `tokio::time::interval(60s)` 每分钟触发 SoakSampler 快照
- [ ] 每分钟输出 `soak_snapshot` JSON（流式，每行一个），共 10 段
- [ ] 结束时输出 `soak_summary` 汇总行
- **验证命令**：`target/debug/examples/bench-runner.exe --op soak --concurrency 200 --total 0 --redis-url redis://127.0.0.1:16379 --prefix sso:bench --soak-secs 600 --mixed-ratio 3:2:2:1:1:1`
- **预期结果**：stdout 输出 10 行 `soak_snapshot` JSON + 1 行 `soak_summary`，`minute_index` 1..10

### 8.2 实现稳定性与内存检测
- [ ] 在 SoakSummary 中计算：`qps_stable = snapshots[9].qps ≥ snapshots[0].qps * 0.8`
- [ ] `p99_stable = snapshots[9].p99 ≤ snapshots[0].p99 * 2`
- [ ] `memory_ok = (rss_peak - rss_start ≤ 51200) && (rss_end - rss_start ≤ 30720)`（50MB / 30MB，单位 KB）
- [ ] 若内存持续增长标注"疑似内存泄漏"阻断（spec §5.6.3 异常 1）
- [ ] 若 p99 逐分钟上升超过首分钟 200% 标注"延迟漂移"告警（spec §5.6.3 异常 2）
- **验证命令**：同 8.1 验证命令
- **预期结果**：`soak_summary` JSON `qps_stable=true, p99_stable=true, memory_ok=true`

---

## 9. Node.js 编排层

### 9.1 实现 bench-orchestrator.js 主编排逻辑
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/bench-orchestrator.js` 新增 ESM 模块，导出 `async function runBench(configPath)`
- [ ] 流程：读取 bench-config.json → SSHOperator.connect → openTunnel(16379→6379) → cargo build → 编排 15 轮压测 → 聚合 → 生成报告 → 清理 → 返回 Go/No-Go
- [ ] 编译命令：`cargo build --example bench-runner --features redis-store`，环境变量 `CARGO_INCREMENTAL=0`
- [ ] spawn 二进制时设置 `maxBuffer: 64 * 1024 * 1024`，捕获 stdout JSON
- [ ] 异常处理：隧道建立失败终止；编译失败终止并报告；单轮崩溃记录 fail 继续下一轮
- **验证命令**：`node -e "import('./docs/spec/redis-store-benchmark/scripts/bench-orchestrator.js').then(m=>console.log(typeof m.runBench))"`
- **预期结果**：输出 `function`

### 9.2 实现 15 轮压测编排与 spawn 收集
- [ ] 在 bench-orchestrator.js 实现 `async function runAllRounds(config, tunnel)`，按 PERF 红线表遍历 11 轮单操作 + 混合 + 连接池稳定性 + Soak + 无死锁 = 15 轮
- [ ] 每轮：预清理 `redis-cli DEL sso:bench:*` → spawn bench-runner → 解析 stdout JSON → 比对 PERF 红线 → 收集 RoundResult
- [ ] Soak 轮次：流式收集每分钟 `soak_snapshot` JSON（按行解析），最后收集 `soak_summary`
- [ ] 隧道中断恢复轮次（7.2）：spawn 后 60s 关隧道 5s 再重建
- [ ] 聚合所有 RoundResult + SoakSummary + PoolResult 传给报告生成
- **验证命令**：`node docs/spec/redis-store-benchmark/scripts/bench-orchestrator.js docs/spec/redis-store-benchmark/scripts/bench-config.json`
- **预期结果**：15 轮全部执行，输出 `benchmark-report.md`，返回 Go/No-Go 结论

---

## 10. 压测报告生成（spec §5.7）

### 10.1 实现 bench-report-generator.js 8 章节渲染
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/lib/bench-report-generator.js` 新增 ESM 模块，导出 `async function generateBenchReport({ roundResults, soakResult, poolResult, cleanResult, config, projectRoot, reportPath })`
- [ ] 渲染 8 章节：①整体结论 ②指标汇总表（11 条 PERF 红线对照）③分并发度详细表 ④file:line 证据表 ⑤资源占用曲线 ⑥Soak 10 段快照 ⑦清理确认 ⑧阻断项清单
- [ ] 复用 EvidenceCollector.verifyAll 校验所有证据 file:line 真实性
- [ ] 查 REDIS_STORE_EVIDENCE 索引为每条结论附 file:line
- [ ] 报告内嵌 JSON 块（供 CI/Grafana 解析，spec §1.3.2）
- [ ] 文件写入失败时报告内容输出 stdout 兜底（spec §5.7.3 异常 2）
- **验证命令**：`node -e "import('./docs/spec/redis-store-benchmark/scripts/lib/bench-report-generator.js').then(m=>console.log(typeof m.generateBenchReport))"`
- **预期结果**：输出 `function`

### 10.2 实现 Go/No-Go 判定与阻断项清单
- [ ] 在 bench-report-generator.js 实现 `function judgeGoNoGo(roundResults, soakResult, poolResult, cleanResult)`
- [ ] `overallPassed` = 所有轮次 `verdict=pass` ∧ soak `qps_stable && p99_stable && memory_ok` ∧ 连接池 `service_unavailable_rate ≤ 0.001` ∧ 清理 `failed.length === 0`
- [ ] `blockers` 收集所有 fail 轮次的 `{operation, concurrency, 红线编号, 实测值, 阈值}`
- [ ] 全部满足 → 结论 `✅ 可上生产`；任一不满足 → 结论 `❌ 阻断` + 阻断项清单
- [ ] 渲染 PERF 红线对照表：每条红线一行，含 `id/operation/concurrency/实测QPS/阈值QPS/实测p99/阈值p99/verdict`
- **验证命令**：同 9.2 验证命令
- **预期结果**：报告"整体结论"章节含 ✅ 或 ❌ 判定，阻断项清单非空当且仅当存在 fail 轮次

---

## 11. 强制清理（spec §5.7 规则 3）

### 11.1 实现 bench-cleaner.js 5 步清理
- [ ] 在 `docs/spec/redis-store-benchmark/scripts/lib/bench-cleaner.js` 新增 ESM 模块，导出 `async function cleanBench({ ssh, tunnel, binaryPath, redisKeyPattern })`
- [ ] 步骤 1：远程 Redis key 清理，`SSHOperator.execCommand("redis-cli --scan --pattern 'sso:bench:*' | xargs -L 100 redis-cli DEL")`
- [ ] 步骤 2：终止 Rust 压测进程，`taskkill /F /IM bench-runner.exe`（Windows）
- [ ] 步骤 3：删除本地临时二进制，`fs.rm target/debug/examples/bench-runner.exe`
- [ ] 步骤 4：关闭 SSH 隧道，`tunnel.close()`
- [ ] 步骤 5：删除上传到服务器的脚本，`SSHOperator.execCommand("rm -f /tmp/bench_*")`
- [ ] 返回 `{ cleaned: [{artifact, status}], failed: [{artifact, reason}] }`
- **验证命令**：`node -e "import('./docs/spec/redis-store-benchmark/scripts/lib/bench-cleaner.js').then(m=>console.log(typeof m.cleanBench))"`
- **预期结果**：输出 `function`

### 11.2 实现清理幂等与失败不中断
- [ ] 每步清理独立 try-catch，失败记录到 `failed` 数组，不中断后续步骤（spec §5.7.3 异常 1）
- [ ] 清理幂等：重复执行 cleanBench 不报错（key 已删则 DEL 返回 0，进程已终止则 taskkill 返回非零但不抛）
- [ ] 在 bench-orchestrator.js 的 finally 块中调用 cleanBench，保证压测结束（无论成功、失败、中断）都执行清理
- [ ] 报告"清理确认"章节标注失败项，要求人工介入
- **验证命令**：同 9.2 验证命令（观察压测结束后清理日志）
- **预期结果**：清理确认章节 `failed` 为空数组（正常情况），或列出失败项要求人工介入

---

## 12. 集成验证与端到端测试

### 12.1 验证 SSH 隧道连通性与编译
- [ ] 编写 `docs/spec/redis-store-benchmark/scripts/bench-precheck.js`，验证：SSHOperator.connect 成功 → openTunnel(16379→6379) 成功 → 本地 `redis-cli -p 16379 PING` 返回 PONG → cargo build --example bench-runner --features redis-store 成功
- [ ] 验证 deploy_key 私钥可读取且权限正确
- [ ] 验证本地 16379 端口未被占用
- **验证命令**：`node docs/spec/redis-store-benchmark/scripts/bench-precheck.js`
- **预期结果**：输出 `SSH OK / Tunnel OK / Redis PONG / Build OK`，全部通过

### 12.2 执行全量 15 轮压测
- [ ] 执行 bench-orchestrator.js 完整流程，覆盖：11 条 PERF 红线单操作 + 混合负载 + 连接池稳定性 + Soak + 无死锁 = 15 轮
- [ ] 验证每轮 stdout JSON 合法可解析，字段完整（对齐 spec §6.2）
- [ ] 验证 Soak 输出 10 段 `soak_snapshot` + 1 段 `soak_summary`
- [ ] 验证混合负载 `by_op` 含 6 个子操作独立指标
- **验证命令**：`node docs/spec/redis-store-benchmark/scripts/bench-orchestrator.js docs/spec/redis-store-benchmark/scripts/bench-config.json`
- **预期结果**：15 轮全部完成，生成 `benchmark-report.md`，返回 Go/No-Go 结论

### 12.3 验证报告完整性与红线判定
- [ ] 验证 `benchmark-report.md` 含 8 个章节（整体结论 / 指标汇总表 / 分并发度详细表 / file:line 证据表 / 资源占用曲线 / Soak 快照 / 清理确认 / 阻断项清单）
- [ ] 验证每条性能结论附 file:line 证据，用 EvidenceCollector.verifyAll 校验行号真实存在
- [ ] 验证 PERF 红线对照表含 11 行，每行 verdict 与实测值一致
- [ ] 验证 Go/No-Go 判定逻辑：全部 pass → ✅；任一 fail → ❌ + 阻断项清单
- **验证命令**：`node docs/spec/redis-store-benchmark/scripts/bench-verify-report.js`
- **预期结果**：报告 8 章节齐全，19 条证据全部通过，红线判定逻辑正确

### 12.4 验证清理彻底性
- [ ] 压测结束后验证：Redis 中无 `sso:bench:*` key（`redis-cli --scan --pattern 'sso:bench:*' | wc -l` = 0）
- [ ] 验证无残留 bench-runner 进程（`tasklist /FI "IMAGENAME eq bench-runner.exe"` 无结果）
- [ ] 验证本地临时二进制已删除（`target/debug/examples/bench-runner.exe` 不存在）
- [ ] 验证 SSH 隧道已关闭（本地 16379 端口无监听）
- [ ] 验证服务器临时脚本已删除（`ls /tmp/bench_*` 无结果）
- **验证命令**：`node docs/spec/redis-store-benchmark/scripts/bench-verify-clean.js`
- **预期结果**：5 项清理全部确认，`failed` 数组为空

### 12.5 验证上游 sz-orm 未被修改
- [ ] 执行 `git status ../sz-orm/`（若为独立仓库）或 `git diff -- ../sz-orm/`（若为子目录），确认无任何文件变更
- [ ] 执行 `git diff -- packages/sz-rust-auth-facade/src/redis_store.rs`，确认被测源码无修改
- [ ] 验证仅新增 `examples/bench-runner.rs`、`docs/spec/redis-store-benchmark/scripts/` 下文件、`Cargo.toml` 的 dev-dependencies（sysinfo）
- **验证命令**：`git diff --name-only && git status --short`
- **预期结果**：变更文件列表中无 `../sz-orm/` 路径，无 `redis_store.rs`，仅含新增的 bench-runner 与 scripts

---

## 任务依赖关系

```
1.1 → 1.3 → 1.4
1.2 → 9.1
1.4 → 2.1
2.1 → 2.2 → 2.3 → 2.4 → 2.5 → 2.6 → 2.7
2.7 → 3.1 → 3.2 → 3.3
3.3 → 4.1 → 4.2 → 4.3
4.3 → 5.1 → 5.2 → 5.3 → 5.4 → 5.5 → 5.6
5.6 → 6.1 → 6.2
6.2 → 7.1 → 7.2 → 7.3
7.3 → 8.1 → 8.2
8.2 → 9.1 → 9.2
9.2 → 10.1 → 10.2
10.2 → 11.1 → 11.2
11.2 → 12.1 → 12.2 → 12.3 → 12.4 → 12.5
```

**关键路径**：1.1 → 1.4 → 2.1 → 2.7 → 3.1 → 5.6 → 6.2 → 7.3 → 8.2 → 9.2 → 10.2 → 11.2 → 12.5

**可并行任务**：
- 1.1 / 1.2 / 1.3 可并行（基础设施互不依赖）
- 2.3 / 2.4 / 2.5 / 2.6 / 2.7 可并行（核心组件互不依赖，均依赖 2.2）
- 3.1 / 4.1 / 5.1 可并行（三个 Store 压测互不依赖，均依赖 2.7）
- 10.1 / 11.1 可并行（报告与清理互不依赖，均依赖 9.2）

---

## 预期产物清单

| 产物 | 路径 | 类型 |
|------|------|------|
| 压测配置 | `docs/spec/redis-store-benchmark/scripts/bench-config.json` | 新增 |
| SSH 隧道模块 | `docs/spec/redis-store-benchmark/scripts/lib/ssh-tunnel.js` | 新增 |
| 证据索引 | `docs/spec/redis-store-benchmark/scripts/lib/evidence-index.js` | 新增 |
| 报告生成器 | `docs/spec/redis-store-benchmark/scripts/lib/bench-report-generator.js` | 新增 |
| 清理模块 | `docs/spec/redis-store-benchmark/scripts/lib/bench-cleaner.js` | 新增 |
| 编排器 | `docs/spec/redis-store-benchmark/scripts/bench-orchestrator.js` | 新增 |
| 预检查脚本 | `docs/spec/redis-store-benchmark/scripts/bench-precheck.js` | 新增 |
| 报告验证脚本 | `docs/spec/redis-store-benchmark/scripts/bench-verify-report.js` | 新增 |
| 清理验证脚本 | `docs/spec/redis-store-benchmark/scripts/bench-verify-clean.js` | 新增 |
| 隧道恢复脚本 | `docs/spec/redis-store-benchmark/scripts/bench-tunnel-recovery.js` | 新增 |
| package.json | `docs/spec/redis-store-benchmark/scripts/package.json` | 新增 |
| Rust 压测二进制 | `packages/sz-rust-auth-facade/examples/bench-runner.rs` | 新增 |
| Cargo.toml | `packages/sz-rust-auth-facade/Cargo.toml` | 修改（dev-dep sysinfo） |
| 压测报告 | `docs/spec/redis-store-benchmark/benchmark-report.md` | 生成（压测后） |

---

> 本任务分解由 spec-task-agent 基于 `spec.md`（37KB）与 `design.md`（954 行）生成。
> 被测代码 `packages/sz-rust-auth-facade/src/redis_store.rs`（646 行）只读不修改，上游 `../sz-orm/` 严禁触碰。
> 所有任务遵循 design.md 架构分层（Node.js 编排层 + Rust 压测执行层），15 轮压测编排，SSH 隧道方案。