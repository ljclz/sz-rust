# Token 降级机制（P3）任务清单

> 版本：1.0  
> 日期：2026-08-08  
> 关联：spec.md / design.md

## T0: DegradationEntry 结构体
- [x] 在 `refresh.rs` 中定义 `DegradationEntry`（roles, permissions, expires_at）
- [x] 派生 Debug, Clone, serde::Serialize, serde::Deserialize
- 状态：✅ 完成

## T1: DegradationStore trait
- [x] 定义 7 个异步方法（set/get/clear × user/device + clear_all）
- [x] async_trait 标注，Send + Sync 约束
- 状态：✅ 完成

## T2: MemoryDegradationStore 实现
- [x] user_entries: Arc<RwLock<HashMap<i64, DegradationEntry>>>
- [x] device_entries: Arc<RwLock<HashMap<(i64, String), DegradationEntry>>>
- [x] get 方法检查 expires_at > now，过期返回 None
- [x] clear_all_degradations 清除两个 map 中该用户的所有条目
- 状态：✅ 完成

## T3: SsoService 新增 degradation_store 字段
- [x] SsoService 新增 `degradation_store: Option<Arc<dyn DegradationStore>>`
- [x] `with_degradation_store(&mut self, store) -> &mut Self`
- [x] `new` 中默认 None
- 状态：✅ 完成

## T4: 用户级降级 API
- [x] degrade_user(user_id, roles, permissions, ttl_secs)
- [x] clear_degradation(user_id) → clear_all_degradations
- [x] get_degradation(user_id) → get_user_degradation
- 状态：✅ 完成

## T5: 设备级降级 API
- [x] degrade_device(user_id, device_id, roles, permissions, ttl_secs)
- [x] clear_device_degradation(user_id, device_id)
- 状态：✅ 完成

## T6: apply_degradation 内部方法
- [x] 设备级优先，用户级兜底
- [x] 子集过滤：claims.roles.retain(|r| entry.roles.contains(r))
- [x] best-effort：查询失败仅 warn，不阻断
- 状态：✅ 完成

## T7: validate / validate_with_renewal 集成降级
- [x] validate 中 verify_access 后调用 apply_degradation
- [x] validate_with_renewal 同理
- 状态：✅ 完成

## T8: revoke_all 联动清除降级
- [x] revoke_all 中调用 clear_all_degradations
- [x] best-effort：失败仅 warn
- 状态：✅ 完成

## T9: revoke_device 联动清除设备降级
- [x] revoke_device 中调用 clear_device_degradation
- [x] best-effort：失败仅 warn
- 状态：✅ 完成

## T10: 模块导出
- [x] DegradationEntry / DegradationStore / MemoryDegradationStore 在 refresh.rs pub
- 状态：✅ 完成

## T11: axum 端点扩展
- [x] POST /sso/degrade/user
- [x] POST /sso/degrade/device
- [x] DELETE /sso/degrade/user/:user_id
- [x] DELETE /sso/degrade/device/:user_id/:device_id
- [x] GET /sso/degrade/:user_id
- 状态：✅ 完成

## T12: 单元测试（degradation_store + apply_degradation）
- [ ] MemoryDegradationStore CRUD 测试
- [ ] TTL 过期测试
- [ ] apply_degradation 子集过滤测试
- [ ] apply_degradation 设备级优先测试
- [ ] apply_degradation 查询失败 best-effort 测试
- 状态：⏳ 待做

## T13: 单元测试（SsoService 降级 API）
- [ ] degrade_user + validate 返回降级后权限
- [ ] clear_degradation + validate 恢复原始权限
- [ ] degrade_device + validate 设备级优先
- [ ] revoke_all 清除降级
- [ ] 降级不能提权（子集约束）
- 状态：⏳ 待做

## T14: 集成测试
- [x] 完整降级流程：登录 → 降级 → validate → 清除 → validate
- [x] 设备级降级优先于用户级
- [x] 降级 TTL 过期自动恢复
- 状态：✅ 完成

## T15: 全量门禁
- [x] cargo test --workspace
- [x] cargo clippy -p sz-rust-auth-facade -p sz-rust-middleware-facade --all-features -- -D warnings
- [x] sz-pay 兼容性检查
- 状态：✅ 完成