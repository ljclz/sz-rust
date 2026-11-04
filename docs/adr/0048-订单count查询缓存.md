# ADR-048：订单列表 COUNT 查询缓存（LRU + TTL，写路径整体失效）

> **状态**：已接受
> **日期**：2026-10-30
> **决策者**：SZ-Rust Team
> **关联 ADR**：[ADR-009](0009-缓存策略-Cache-facade-全局实例-多驱动.md)（缓存策略）、[ADR-046](0046-性能工程整改-限流分桶与熔断窗口与门禁重写.md)（性能工程整改）

## 背景

31–90 天性能路线项中，「先做 COUNT 缓存（最高 ROI）」经用户于 2026-10-30 选定优先实施。

`POST /api/v1/order/list` 每次请求都执行一次 `SELECT COUNT(*)` + 一次列表查询。COUNT 在无索引覆盖的筛选条件下会做全表/全索引扫描，且其值与分页无关——同一筛选条件下，不同 `page` 的请求共享同一个 COUNT 结果。压测基线中 order/list 的 p99 达 574–2112 ms 且三次重复 CV 高达 67.26%（见 `docs/audit/2026-10-29-性能整改现场验证记录.md` §3.3），COUNT 全扫描是主要嫌疑之一。

仓库已有 `packages/sz-rust-sz300/src/query_cache.rs`（v1.9.0 引入，LRU + TTL + 命中率统计，`with_defaults()` = 10,000 条 / TTL 60 s），但从未接入任何业务查询（grep 仅命中 `lib.rs` 的 `pub mod query_cache`）。本决策将其接入 order/list 的 COUNT 查询。

## 决策

1. **`AppState` 新增 `query_cache: SharedQueryCache`**（无 feature gate），所有 AppState 构造点（main.rs / mock_app_state / 集成测试 / bench）统一接线。
2. **`OrderService::list(pool, cache, page, page_size, filters)`**：COUNT 查询前先按「WHERE 子句 + 参数化值」构造缓存键（`order:count:<where_clause>|<params_json>`），命中则直接使用缓存值；未命中则执行 COUNT 并写入（值 = `i64` 小端字节，TTL 用缓存默认 60 s）。列表查询不受缓存影响，照常执行。
3. **写路径失效**：`OrderService::create` 提交成功后调用 `cache.clear()`。因为新订单会改变所有筛选口径下的 COUNT，而 QueryCache 不支持前缀批量失效，整体清空是最简单且不会产生过期值的做法。
4. **旁路写路径依赖 TTL**：GraphQL mutation、MQTT 离线订单、Saga 分布式事务等写路径不经过 `OrderService::create`，也不持有 `AppState` 引用（只有 Pool），本轮不强行接线，靠 60 s TTL 自然过期，最多 60 s 最终一致。
5. **观测**：QueryCache 自带的 `hits`/`misses`/`hit_rate` 统计随每次 list 调用更新，可直接观测命中率。

## 后果

### 正面后果

- 同一筛选条件下所有分页请求共享一次 COUNT 执行，读多写少场景下 order/list 的 COUNT 开销从每次请求变为每 60 s 一次（或每次订单写入后一次）。
- 复用既有 v1.9.0 QueryCache 基础设施，无新增依赖。
- 缓存值仅 8 字节/项，10,000 条上限内存开销可忽略。

### 负面后果

- **COUNT 值最多 60 s 最终一致**：旁路写路径（GraphQL/MQTT/Saga）不触发清空，列表总数可能短暂滞后。
- **`clear()` 粒度粗**：订单写入会清空整个共享缓存（未来商品列表缓存接入后也会被一起清空）。这是正确性优先的取舍；若未来商品缓存接入且订单写频繁，可改为「每领域独立 QueryCache 实例」或为 QueryCache 增加前缀批量失效。
- 缓存键构造需要序列化参数值（serde_json），每次 list 增加少量 CPU 开销（相对 DB 查询可忽略）。

## 注意事项

- 缓存键必须包含参数化值本身（`serde_json::to_string(params)`），不能只依赖 WHERE 子句文本——同一 `WHERE merchant_id = ?` 在不同值下必须使用不同键。
- 缓存值异常（非 8 字节）时按未命中处理，重新执行 COUNT，不 panic、不返回脏值。
- 集成测试必须覆盖「命中/未命中」与「create 后清空」两条路径，防止缓存导致 total 永久错误。

## Bug 定位提示

- 若 order/list 的 total 与实际行数不一致：先查 `query_cache.rs` 的 TTL 与清空逻辑（`OrderService::create` 是否提交成功才清空；旁路写路径是否在 60 s 窗口内未触发清空）。
- 若命中率长期为 0：检查 `count_cache_key` 是否因参数序列化不稳定导致键漂移（同条件不同键），或 `OrderService::list` 是否真的传入了 `Some(&state.query_cache)`。
- 关键代码路径：`packages/sz-rust-sz300/src/services/order_service.rs`（`list` COUNT 缓存段 + `create` 提交后清空）、`packages/sz-rust-sz300/src/controllers/order.rs`（传入缓存）、`packages/sz-rust-sz300/src/query_cache.rs`。