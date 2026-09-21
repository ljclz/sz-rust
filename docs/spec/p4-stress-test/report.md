# P4 边界/极端测试报告

> **日期**：2026-08-09
> **范围**：P4-15 ~ P4-18

---

## P4-15 并发 10K 连接压测

**状态**：✅ 脚本已创建

- 脚本：`p4-15-concurrent-10k.js`
- 目标：10,000 并发 HTTP 连接
- 测试端点：`/health`
- 批次策略：500 连接/批次，避免瞬时 fd 耗尽
- 指标：连接成功率、QPS、P50/P99/P99.9 延迟

**执行方式**：
```bash
# 需先启动 sz-rust-sz300 服务
node docs/spec/p4-stress-test/p4-15-concurrent-10k.js --target http://127.0.0.1:8300 --connections 10000
```

---

## P4-16 100W Token 签发基准 ✅

**状态**：✅ 已执行完成

- 脚本：`p4-16-token-1m.js`
- Token 数量：1,000,000

### 测试结果

| 指标 | 值 |
|------|-----|
| 签发耗时 | 2,852ms |
| 签发吞吐量 | 350,631 tokens/s |
| 校验耗时 | 2,297ms |
| 校验吞吐量 | 435,350 tokens/s |
| 校验成功率 | 100.00% |
| 内存增长 (RSS) | 47.1 → 516.5 MB (+469.4 MB) |
| 内存增长 (Heap) | 4.6 → 430.6 MB (+426.0 MB) |

**结论**：
- 签发 350K tokens/s，满足 100W Token 批量签发需求（~3 秒完成）
- 校验 435K tokens/s，满足高并发校验需求
- 内存增长 ~470MB / 100W Token，每 Token 约 470 字节（含 JWT 字符串存储）
- 100% 校验成功率，0 失败

---

## P4-17 Redis 断线恢复测试

**状态**：✅ 设计完成，脚本待执行

**测试目标**：
1. Redis 连接池自动重连
2. 断线期间命令重试/降级
3. 恢复后服务自愈

**测试方案**：
1. 启动 sz-rust-sz300 + Redis 正常运行
2. 通过 SSH 隧道连接 Redis，验证基本操作
3. 服务器上 `redis-cli FLUSHALL` 模拟 Redis 重启
4. 观察 sz-rust-sz300 日志中重连行为
5. 验证恢复后 Redis 操作正常

**依赖**：ConnectionManager（Redis 自动重连机制已在 sz-rust-auth-facade 中实现）

---

## P4-18 内存泄漏检测（长时间 Soak）

**状态**：✅ 已有 CI soak.yml

**现有机制**：
- CI `soak.yml` 每周日 00:00 UTC 自动执行
- 6 小时 soak test，60 秒指标采样
- 420 分钟超时

**测试目标**：
1. 6 小时持续运行，内存增长 < 10%
2. 无 panic / no OOM
3. 延迟稳定（P99 不退化）

**执行方式**：
```bash
# 手动触发
gh workflow run soak.yml
# 或本地运行
cargo test --test soak -- --ignored
```

---

## 总结

| 任务 | 状态 | 关键结果 |
|------|------|---------|
| P4-15 并发 10K | ✅ 脚本就绪 | 待服务器执行 |
| P4-16 100W Token | ✅ 已完成 | 350K 签发/s, 435K 校验/s |
| P4-17 Redis 断线 | ✅ 设计完成 | ConnectionManager 已实现 |
| P4-18 内存泄漏 | ✅ CI 已覆盖 | soak.yml 每周自动执行 |