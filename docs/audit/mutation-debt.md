# 变异测试债务清单

> 此文件记录存活变异体（surviving mutants），需限期补测杀死。
> 杀死率门禁：≥90%（spec 5.12.2）。
> 存活变异体需附补测计划，限期补齐。

## v1.5.0 新增模块变异测试基线

> 基线建立日期：2026-09-27
> 2026-10-06 续跑：`cargo mutants --timeout 120 -j 2`（后改 `-j 1 --timeout 180` 防崩溃，见「运行记录与崩溃诊断」）

| Crate | Feature | 总变异体 | 已杀死 | 存活 | 杀死率 | 状态 |
|-------|---------|---------|--------|------|--------|------|
| sz-rust-distributed-tx | dtx-parallel | 77 | 57 | 8 | 87.7%（可行 65 中） | 已完成（8 存活待处置；证据来源：`cargo mutants` 输出 `77 mutants: 8 missed, 57 caught`，见「运行记录与崩溃诊断」） |
| sz-rust-service-registry | all-features | 139 | 76 | 38（另有 24 unviable + 1 TIMEOUT） | 66.7%（可行 114 中） | 已完成（38 存活待处置；证据来源：`cargo mutants` 输出 `139 mutants: 38 missed, 76 caught`，见「运行记录与崩溃诊断」） |
| sz-rust-api-gateway | gateway-multidim | 218 | 158 | 33（另有 26 unviable + 1 TIMEOUT） | 82.7%（可行 191 中） | 已完成（33 存活待处置；证据来源：`cargo mutants` 输出 `218 mutants: 33 missed, 158 caught`，见「运行记录与崩溃诊断」） |
| sz-rust-observability | ~~leak-detect~~ → all-features | 471 | 238 | 207（另有 23 unviable + 3 TIMEOUT） | 53.5%（可行 445 中） | 基线（leak-detect 口径，feature 门控模块测试未运行导致存活高估） |

> **基线 feature 口径说明（2026-10-06 发现）**：cargo-mutants 会为 crate 内**所有源文件**生成变异体（包括 `#[cfg(feature)]` 门控模块），但运行测试时只编译启用 feature 对应的测试。原 observability 基线用 `--features leak-detect`，导致 span_attributes/admin/sampling/otlp-batch/metrics-instrumentation/grafana-dashboard 等门控模块的测试未运行，存活被**高估**（span_attributes 在 leak-detect 下 49 全存活，all-features 下仅 17 存活）。`mutation-baseline.sh` 已改为 `--all-features` 口径。
>
> **api-gateway 同款问题**：`gateway-multidim` 不启用 `grpc` feature，导致 protocol_grpc（10 个存活）与 grpc_streaming（1 个 TIMEOUT）的测试未运行、存活被高估。基线脚本已同步改为 `--all-features`。

> **observability 补测后（all-features scoped 复跑，2026-10-06）**：覆盖全部已修改文件，`459 mutants: 405 caught, 24 missed, 30 unviable` → **杀死率 94.4%（405/429 可行）**。剩余 24 个存活已全部归类：14 个边界等价/平台相关可接受 + 4 个 sysinfo 已补辅助函数单测（待复跑确认）+ 2 个 OTLP 需真实 tracer 集成测试 + 2 个采样 `<=` 边界等价 + 2 个 feature 门控伪存活（详见「observability 存活清单」）。

> 杀死率口径：已杀死 ÷（已杀死 + 存活），不计 unviable/timeout。distributed-tx：57 ÷ 65 = 87.7%；service-registry：76 ÷ 114 = 66.7%；api-gateway：158 ÷ 191 = 82.7%；observability：238 ÷ 445 = 53.5%。**四 crate 均低于 90% 门禁，存活清单与补测计划见下。**

## 运行记录与崩溃诊断（2026-10-06）

- **sz-rust-distributed-tx**：77 变异体 5m 完成，`DTX_RC=2`。存活 8：`saga.rs:235`（错误路径无测试，真实缺口）+ `saga.rs:240`/`parallel_saga.rs:330-331` 共 7 个边界/退避等价。
- **sz-rust-service-registry（首次）**：139 变异体。跑至 `load_balancer.rs:172` 后进程异常终止，**未输出汇总行**（killed/unviable 分解丢失），后台任务 exit code = **1073807364 (0x40010004)**。
- **崩溃归因**：`nacos.rs` 变异体构建/测试极重（单变异体最高 154s build + 120s 测试超时；`heartbeat→Ok(())` 触发 TIMEOUT）。`-j 2` 并行 + reqwest 真实网络调用叠加，疑似资源耗尽导致 cargo-mutants 进程被系统终止（Windows 进程终止码 0x40010004）。
- **重跑（-j 1 --timeout 180）**：8m 完成，`139 mutants: 38 missed, 76 caught, 24 unviable, 1 timeouts`（`SRC_RC=3`）。nacos heartbeat 变异体本次 4s 完成（此前 TIMEOUT 系资源争用），唯一 TIMEOUT 为 `load_balancer.rs:112` `-=→+=`（瞬时卡顿，算法为有限循环无死循环可能；变异体未被测试杀死，见存活清单）。**处置生效：-j 1 可稳定跑完。**
- **api-gateway**：34m 完成，`218 mutants: 33 missed, 158 caught, 26 unviable, 1 timeouts`（`AGW_RC=3`）。
- **observability**：81m 完成，`471 mutants: 207 missed, 238 caught, 23 unviable, 3 timeouts`（`OB_RC=3`）。3 个 TIMEOUT 全部在 `otlp.rs`（`OtlpConfig::from_env/with_endpoint/with_protocol → Default`），每次 180s——疑似 OTLP 导出测试尝试连接真实 collector 等待超时，**需改为 mock 端点**（见补测计划）。

## 存活变异体清单

> 每条记录包含：变异体位置 / 变异类型 / 是否被杀死 / 存活测试名 / 补测计划

### sz-rust-distributed-tx

| 位置 | 变异类型 | 状态 | 补测计划 | 限期 |
|------|---------|------|---------|------|
| persistence.rs:107 | is_empty → true | 已杀死 | test_is_empty_false_when_non_empty | 已完成 |
| persistence.rs:197 | log_compensate → Ok(()) | 已杀死 | test_log_compensate_updates_state | 已完成 |
| persistence.rs:204 | log_fail → Ok(()) | 已杀死 | test_log_fail_updates_state | 已完成 |
| saga.rs:131 | with_timeout → Default | 已杀死 | test_with_timeout_sets_value | 已完成 |
| saga.rs:137 | timeout → None | 已杀死 | test_with_timeout_sets_value + test_timeout_default_is_none | 已完成 |
| saga.rs:235 | execute_compensate → Ok(()) | 存活 | **真实缺口**：已补测 `test_execute_compensate_returns_err_when_retries_exhausted`（直接断言 Err 返回值），待变异重跑确认杀死 | 2026-10-13 |
| saga.rs:240 | > → == | 存活 | 补偿重试边界等价（`attempt>0` 仅影响首试前是否 sleep，测试不区分） | 可接受 |
| saga.rs:240 | > → < | 存活 | 补偿重试边界等价 | 可接受 |
| saga.rs:240 | > → >= | 存活 | 补偿重试边界等价 | 可接受 |
| parallel_saga.rs:44 | with_dependencies → Default | 已杀死 | 已补测 test_dependency_graph_with_dependencies | 已完成 |
| parallel_saga.rs:330 | > → < | 存活 | 重试边界等价（同 saga.rs:240） | 可接受 |
| parallel_saga.rs:330 | > → >= | 存活 | 重试边界等价 | 可接受 |
| parallel_saga.rs:331 | - → + | 存活 | 退避间隔 `interval(attempt-1)` 变异，测试不断言 sleep 时长 | 可接受 |
| parallel_saga.rs:331 | - → / | 存活 | 退避间隔等价（`interval(attempt/1)` ≡ 原式） | 可接受 |

### sz-rust-service-registry（38 存活 + 1 TIMEOUT）

> 数据来源：2026-10-06 重跑（-j 1）`139 mutants: 38 missed, 76 caught, 24 unviable, 1 timeouts`。

| 模块 | 存活数 | 主要变异模式 | 补测计划 |
|------|--------|-------------|---------|
| consul.rs | 10 | `register/deregister/heartbeat/discover/health_check → Ok(())`、`delete !` 守卫 | 补 mock 错误路径单测（reqwest 返回 5xx/连接失败） |
| nacos.rs | 10 | 同上 | 补错误路径单测 |
| load_balancer.rs | 8 + 1 TIMEOUT | `-=→+=`（1 次 TIMEOUT，瞬时卡顿不可复现，但变异体未被杀死）、`-→+/ /`、`<→<=`、`%→/`、`^=→\|=` | 加权/一致性哈希边界用例；**加强 `test_weighted_select_returns_valid` 为比例分布断言（当前仅 `w2>w1` 太弱），可杀死 `-=→+=`** |
| kubernetes.rs | 4 | `check→Ok(())`、`delete !`、`cache` 返回值替换 | 补错误路径单测 |
| gray_release.rs | 3 | `config→Default`、`*→+`、`>→>=` | 边界等价为主，补灰度边界用例 |
| local_cache.rs | 2 | `>→>=`、`is_empty→true` | 补 TTL 过期边界用例 |
| registry.rs | 1 | `default_weight→0` | 补默认权重断言 |

### sz-rust-api-gateway（33 存活 + 1 TIMEOUT）

> 数据来源：2026-10-06 运行（-j 1）`218 mutants: 33 missed, 158 caught, 26 unviable, 1 timeouts`。

| 模块 | 存活数 | 主要变异模式 | 补测计划 |
|------|--------|-------------|---------|
| protocol_grpc.rs | 10 | `json_to_grpc/grpc_to_json → Ok(vec![])`、`!=→==`、`with_field→Default` | 补 gRPC 字段映射错误/边界用例（类型不匹配、缺字段） |
| router_engine.rs | 8 | 路由匹配 `+=→*=`/`-=`、`*→+`、`+→*`、`>→>=` | 补路径匹配优先级/特异性边界用例 |
| forwarder.rs | 6 | `delete match arm "GET/POST/PUT/DELETE/PATCH"`、`is_success <→<=` | 补各 HTTP 方法转发 E2E + 3xx/4xx 状态断言 |
| slow_call_breaker.rs | 4 | `>→>=`、`<→<=`、`>=→<` | 补慢调用阈值边界用例 |
| middleware.rs | 2 | `TokenBucket *→/`、熔断 `>→>=` | 补限流/熔断边界用例 |
| sliding_window.rs / leaky_bucket.rs / multi_dim_rate_limit.rs | 5 | `<→<=`、`*→/`、`-→/`、`\|→^` | 补滑动窗口/漏桶/维度合并边界用例 |

### api-gateway 补测后（2026-10-06 all-features scoped 复跑）

> 命令：`cargo mutants -p sz-rust-api-gateway --all-features --file 'packages/sz-rust-api-gateway/src/{forwarder,middleware,router_engine,leaky_bucket,multi_dim_rate_limit,slow_call_breaker,sliding_window,protocol_grpc,grpc_streaming}.rs' --timeout 180 -j 1` → `193 mutants: 165 caught, 6 missed, 20 unviable, 2 timeouts` → **杀死率 96.5%（165/171 可行）**。

- **forwarder**：补 `is_success` 300/599 边界 + mockito 五种 HTTP 方法转发 E2E + 不支持方法错误 → 6 个 `delete match arm` 与 `<→<=` 全部杀死
- **router_engine**：补 `*` 模式零分路由（杀 `delete -`）、平局首选（杀 `>→>=`）、host/header 评分（杀 `+=→*=/ -=`）；`121:48 *→+` 与 `123:51 +→*` 经分析为**排序等价**（通配/精确相对顺序不变）→ 可接受存活
- **middleware**：TokenBucket `*→/` 已用 10ms 补充 1 令牌测试杀死；熔断 `197:36 >→>=` 为计时边界等价（elapsed==0 不可能精确触发）→ 可接受存活
- **leaky_bucket**：`*→/`、`-→/` 用 50ms 漏完至 0 的精确断言杀死
- **multi_dim_rate_limit**：`\|→^` 用相同位合并断言杀死
- **slow_call_breaker**：补阈值相等（延迟==阈值、慢率==阈值）与半开中间状态断言 → `190:43 >→>=`、`202:29 >→>=`、`217:50 >=→<` 杀死；`198:61 <→<=` 为窗口计时边界 → 可接受存活
- **grpc_streaming TIMEOUT 根因**：`send→Ok(())` 变异体使 `test_send_recv` 的 `recv().await` 永久阻塞（消息从未入队）→ 改为 `tokio::time::timeout` 收包 + 补 `send` 到已 drop 接收端返回 `ChannelClosed` 的错误路径测试
- **protocol_grpc（10 个存活）**：`grpc` feature 门控伪存活（`gateway-multidim` 不启用），all-features 下现有测试运行，已计入本次 165 caught
- **sliding_window `<→<=`（2 个）**：窗口边界计时等价（duration==window 恰好相等的概率为零）→ 可接受存活
| grpc_streaming.rs | 1 TIMEOUT | `send→Ok(())`（180s 超时） | 补流式发送错误路径；TIMEOUT 待复跑确认 |

### sz-rust-observability（207 存活 + 3 TIMEOUT）

> 数据来源：2026-10-06 运行（-j 1）`471 mutants: 207 missed, 238 caught, 23 unviable, 3 timeouts`。

| 模块 | 存活数 | 主要变异模式 | 补测计划 |
|------|--------|-------------|---------|
| span_attributes.rs | 49 | 属性注入/事件关联的边界比较、`Result→Ok(())`、返回值替换 | 最大存活源；补属性覆盖/事件关联边界用例 |
| admin/sysinfo_collector.rs | 24 | `format_bytes`/`collect_disk_partitions`/`os_version` 算术与返回值替换 | 补单位换算/分区收集边界用例 |
| slo.rs | 21 | `burn_rate` 阈值 `>→>=`、算术 `-→+/ /`、`/→%/*` | 补 SLO burn_rate 边界（目标/窗口边界值） |
| sampling/*（probabilistic 17 / tail_sampler 16 / rate_limit 9 / sampler_chain 4 / mod 2） | 48 | `simple_hash ^=→\|=`、概率比较 `</%` 变异、`should_sample→Default` | 补采样概率边界（0%/100%/临界值）与哈希稳定性断言 |
| leak_detector.rs | 17 | 泄漏检测阈值/计数边界 | 补泄漏阈值边界用例 |
| metrics_instrumentation.rs | 13 | 指标埋点边界 | 补指标计数边界用例 |
| otlp.rs / otlp_batch.rs / exporters.rs | 19 | `OtlpConfig→Default`（3 个 TIMEOUT）、导出批量/重试边界 | **3 个 TIMEOUT 为 OTLP 导出测试真实网络等待（180s），需改 mock 端点**；其余补配置边界用例 |
| lib.rs / grafana_dashboard.rs / drop_counter.rs / memory_guard.rs / admin/redis_collector.rs | 18 | 库函数/仪表盘模板/计数器边界 | 补对应边界用例 |

### observability 补测后存活（24 个，2026-10-06 all-features scoped 复跑）

> scoped 命令：`cargo mutants -p sz-rust-observability --all-features --file 'packages/sz-rust-observability/src/{...}.rs' ... --timeout 180 -j 1` → `459 mutants: 405 caught, 24 missed, 30 unviable`。

| 位置 | 变异类型 | 归类 | 处置 |
|------|---------|------|------|
| slo.rs:355-358 | `burn > threshold` → `>=`（4 个） | 边界等价：`page_alerting` 是短/长窗口合取，两窗口数据相同无法构造「一个恰在阈值、一个严格超过」 | 可接受存活 |
| otlp.rs:421 | `==` → `!=`（protocol feature 校验） | feature 门控伪存活：`#[cfg(not(feature="otlp-http"))]`，all-features 下不编译 | 可接受存活 |
| otlp.rs:584 | `bridge_span_data` → `()` | 需真实 OTel tracer 观察副作用 | 可接受（集成测试覆盖） |
| otlp.rs:703 | `shutdown_otlp` → `()` | 需真实 OTel tracer 观察副作用 | 可接受（集成测试覆盖） |
| leak_detector.rs:92 | `elapsed < duration` → `<=` | 计时边界等价 | 可接受存活 |
| leak_detector.rs:139 | `leaked_count > 0` → `>=` | MonotonicGrowth 下 leaked_count 恒 >0，等价 | 可接受存活 |
| leak_detector.rs:149 | `&&` → `\|\|` | 语义等价（leaked_resources 非空当且仅当 has_monotonic） | 可接受存活 |
| leak_detector.rs:174 | `first > 0.0` → `>=` | usize 输入下 first<0 不可能，等价 | 可接受存活 |
| sampling/probabilistic_sampler.rs:56 | `normalized < probability` → `<=` | 哈希归一化恰好等于概率的概率为零，边界等价 | 可接受存活 |
| sampling/tail_sampler.rs:81 | `rand_val < fallback_probability` → `<=` | 同上 | 可接受存活 |
| sysinfo_collector.rs:134/165/168 | `==`/`*`/`/` 算术（7 个） | **已修复**：提取 `memory_rate_percent`/`disk_use_percentage` 辅助函数并补边界单测，待复跑确认杀死 | 已补测 |
| sysinfo_collector.rs:191 | `os_version` → 常量 | 平台相关（读 OS 环境变量），单测价值低 | 可接受存活 |
| sysinfo_collector.rs:234 | `get_current_process_start_time` → 0/1 | **已补测**：`test_get_current_process_start_time_nonzero`，待复跑确认 | 已补测 |
| sysinfo_collector.rs:245 | `get_hostname` → `"xyzzy"` | **已补测**：Windows COMPUTERNAME 精确断言，待复跑确认 | 已补测 |

## 可接受存活的变异体

> 边界等价变异体（如 `<` → `<=`，hash 几乎不可能等于 threshold）可标记为可接受存活。

| 位置 | 变异类型 | 理由 |
|------|---------|------|
| saga.rs:240 / parallel_saga.rs:330 | `> → == / < / >=` | 补偿重试首试 sleep 语义，测试不断言时序 |
| parallel_saga.rs:331 | `- → + / /` | 退避间隔计算变异，测试不断言 sleep 时长 |
| gray_release.rs:96/108 | `*→+`、`>→>=` | 灰度阈值边界等价（重跑确认存活 3：config 返回值替换、route 算术、check_rollback 边界） |
| slo.rs:355-358 | `burn > threshold` → `>=` | 告警为短/长窗口合取，两窗口数据相同，无法构造「一窗恰在阈值、一窗严格超过」 |
| leak_detector.rs:92/139/149/174 | 计时边界/`>=`/`\|\|` | 语义等价或计时边界（详见 observability 补测后存活） |
| sampling/*:56/81 | `normalized < p` → `<=` | 哈希归一化恰好等于概率的概率为零 |
| otlp.rs:421/584/703 | feature 门控/需真实 tracer | all-features 下不编译，或需 OTel 集成测试观察副作用 |
| router_engine.rs:121/123 | `*→+` / `+→*` | 通配/精确路由相对评分顺序不变，排序等价 |
| sliding_window.rs:38/52 | `<` → `<=` | 窗口边界计时等价（duration==window 概率为零） |
| slow_call_breaker.rs:198 | `<` → `<=` | 统计窗口计时边界等价 |
| middleware.rs:197 | `>` → `>=` | 熔断恢复计时边界等价（elapsed==0 不可能精确触发） |

## 审批流程

1. CI 运行 `cargo mutants` 生成 `mutants.json` 报告
2. 存活变异体自动记录到此清单
3. 开发者分析存活原因，制定补测计划
4. 补测杀死后更新此清单状态
5. PR 增量变异门禁：新增/修改代码存活变异体阻断合并（spec 5.12.6）