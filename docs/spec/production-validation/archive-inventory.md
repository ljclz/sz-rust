# 归档文档清单

> **归档日期**: 2026-08-08
> **归档目录**: `docs/spec/production-validation/`

---

## 一、归档文件

| 文件 | 描述 | 大小 |
|------|------|------|
| spec.md | 需求规格文档 | 28KB |
| design.md | 技术设计文档 | 64KB |
| tasks.md | 编码任务分解文档 | 20KB |
| validation-config.json | 验证配置文件 | 2KB |
| validation-report.md | 验证报告 | 4KB |
| report-review.md | 报告审查记录 | 3KB |

## 二、验证脚本

| 目录 | 描述 |
|------|------|
| scripts/lib/ | 基础设施模块（ssh-operator / local-builder / evidence-collector / cleaner / report-generator） |
| scripts/validators/ | 验证模块（mysql / postgresql / redis / mqtt / deploy / e2e） |
| scripts/orchestrator.js | 主入口编排脚本 |
| scripts/package.json | Node.js 包配置 |

## 三、产物清理确认

| 产物 | 状态 | 证据 |
|------|------|------|
| 服务器测试脚本 | ✅ 已删除 | Cleaner 报告：服务器测试脚本 已删除 |
| MySQL test.sz_validation_tmp | ✅ 已删除 | Cleaner 报告：MySQL test.sz_validation_tmp 已删除 |
| MySQL shop.sz_validation_tmp | ✅ 已删除 | Cleaner 报告：MySQL shop.sz_validation_tmp 已删除 |
| MySQL njszjt.sz_validation_tmp | ✅ 已删除 | Cleaner 报告：MySQL njszjt.sz_validation_tmp 已删除 |
| Redis sz_*_test keys | ✅ 已删除 | Cleaner 报告：Redis sz_*_test keys 已删除 |
| PostgreSQL sz_pg_validation_tmp | ✅ 已删除 | Cleaner 报告：PostgreSQL sz_pg_validation_tmp 已删除 |
| mosquitto_sub 验证进程 | ✅ 已终止 | Cleaner 报告：mosquitto_sub 验证进程 已终止 |
| 本地临时脚本 | ✅ 已删除 | diagnose.js / diagnose-mqtt.js / diagnose-dns.js / check-server.js / test-e2e.js / start-pay.js / run-start-pay.js / start-pay.sh / check-pay-log.js / check-mysql.js 均已删除 |

## 四、环境问题记录

| 问题 | 影响模块 | 原因 | 修复建议 |
|------|----------|------|----------|
| MQTT DNS 解析失败 | MQTT | 服务器 /etc/resolv.conf 配置问题，无法解析 iot.鲜视达.cn | 在服务器 /etc/hosts 中添加 iot.鲜视达.cn 的 IP 映射，或修复 DNS 配置 |
| MySQL 3306 未监听 | sz-pay-server 部署 | sz-pay .env 配置 MySQL 端口 3306，但服务器 MySQL 运行在 8802 端口 | 修改 sz-pay .env 中 SZ_PAY_DB_PORT 为 8802，或启动 MySQL 3306 端口 |
| Windows 无法交叉编译 | Deploy | Windows 上无 Docker，无法使用 cross 工具编译 musl target | 安装 Docker + cross，或在服务器上直接编译 |