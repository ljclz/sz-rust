# SSO 跨域单点登录（P4）任务清单

> 版本：1.0
> 日期：2026-08-08
> 关联：spec.md / design.md

## T0: SsoTicket 结构体
- [x] 定义 SsoTicket（ticket, user_id, username, redirect_uri, roles, permissions, created_at, expires_at）
- [x] 派生 Debug, Clone, serde::Serialize, serde::Deserialize
- 状态：✅ 完成

## T1: TicketStore trait
- [x] 定义 3 个异步方法（save / take / peek）
- [x] async_trait 标注，Send + Sync 约束
- 状态：✅ 完成

## T2: MemoryTicketStore 实现
- [x] Arc<RwLock<HashMap<String, SsoTicket>>> 存储
- [x] take 方法取出并删除（一次性使用）
- [x] peek 方法仅查看不删除
- 状态：✅ 完成

## T3: SsoService 新增 ticket_store 字段
- [x] SsoService 新增 `ticket_store: Option<Arc<dyn TicketStore>>`
- [x] `with_ticket_store(&mut self, store) -> &mut Self`
- [x] `new` 中默认 None
- 状态：✅ 完成

## T4: generate_ticket API
- [x] 生成 UUID v4 ticket
- [x] 保存到 ticket_store
- [x] 记录审计日志（TicketGenerate）
- 状态：✅ 完成

## T5: exchange_ticket API
- [x] take 操作（一次性使用）
- [x] 验证未过期
- [x] 签发新 TokenPair
- [x] 记录审计日志（TicketExchange）
- 状态：✅ 完成

## T6: validate_ticket API
- [x] peek 操作（不消费）
- [x] 返回 Option<SsoTicket>
- 状态：✅ 完成

## T7: 模块导出
- [x] SsoTicket / TicketStore / MemoryTicketStore 在 refresh.rs pub
- 状态：✅ 完成

## T8: 单元测试
- [x] generate_and_exchange_ticket — 生成并交换 ticket
- [x] ticket_one_time_use — 一次性使用验证
- [x] ticket_ttl_expired — TTL 过期测试
- [x] ticket_without_store_return_err — 未配置 store 返回错误
- [x] validate_ticket_without_consuming — validate 不消费 ticket
- 状态：✅ 完成

## T9: 全量门禁
- [x] cargo test --workspace
- [x] cargo clippy -p sz-rust-auth-facade --all-features -- -D warnings
- [x] sz-pay 兼容性检查
- 状态：✅ 完成