# 审计日志持久化（P5）任务清单

> 版本：1.0
> 日期：2026-08-08
> 关联：spec.md / design.md

## T0: AuditEventType 枚举
- [x] 定义 13 种事件类型
- [x] 派生 Debug, Clone, serde::Serialize, serde::Deserialize
- 状态：✅ 完成

## T1: AuditEvent 结构体
- [x] 定义 AuditEvent（event_type, user_id, device_id, timestamp, detail）
- [x] 派生 Debug, Clone, serde::Serialize, serde::Deserialize
- 状态：✅ 完成

## T2: AuditStore trait
- [x] 定义 2 个异步方法（save / query）
- [x] async_trait 标注，Send + Sync 约束
- 状态：✅ 完成

## T3: MemoryAuditStore 实现
- [x] Arc<RwLock<Vec<AuditEvent>>> 存储
- [x] save 追加事件
- [x] query 按 user_id 过滤，返回最近 limit 条
- 状态：✅ 完成

## T4: SsoService 新增 audit_store 字段
- [x] SsoService 新增 `audit_store: Option<Arc<dyn AuditStore>>`
- [x] `with_audit_store(&mut self, store) -> &mut Self`
- [x] `new` 中默认 None
- 状态：✅ 完成

## T5: record_audit 内部方法
- [x] best-effort：失败仅 warn，不中断业务
- [x] 自动填充 timestamp
- 状态：✅ 完成

## T6: 关键操作自动审计
- [x] login 记录 Login 事件
- [x] revoke_all 记录 RevokeAll 事件
- [x] degrade 记录 Degrade 事件
- [x] ticket_generate 记录 TicketGenerate 事件
- [x] ticket_exchange 记录 TicketExchange 事件
- 状态：✅ 完成

## T7: query_audit API
- [x] 查询用户审计事件
- [x] limit 参数控制返回数量
- 状态：✅ 完成

## T8: 模块导出
- [x] AuditEvent / AuditEventType / AuditStore / MemoryAuditStore 在 refresh.rs pub
- 状态：✅ 完成

## T9: 单元测试
- [x] audit_records_login — 登录审计
- [x] audit_records_revoke_all — 撤销审计
- [x] audit_records_degrade — 降级审计
- [x] audit_records_ticket_generate_and_exchange — ticket 审计
- [x] audit_query_without_store_return_err — 未配置 store 返回错误
- 状态：✅ 完成

## T10: 全量门禁
- [x] cargo test --workspace
- [x] cargo clippy -p sz-rust-auth-facade --all-features -- -D warnings
- [x] sz-pay 兼容性检查
- 状态：✅ 完成