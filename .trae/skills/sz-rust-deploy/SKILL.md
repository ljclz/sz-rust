---
name: sz-rust-deploy
description: 部署就绪检查 — 确保应用可安全部署到生产环境。提交 release 分支时触发。
tools: [docker, kubectl, helm]
agentMode: manual
---

# 部署就绪检查（sz-rust）

## 触发条件

- 合并到 `release/*` 或 `main` 分支
- 版本号变更

## 检查清单

### 构建
- [ ] `cargo build --release --workspace` 成功
- [ ] Docker 镜像构建成功
- [ ] 镜像大小 <= 基线 + 10%

### 配置
- [ ] 环境变量文档已更新
- [ ] 敏感配置不使用硬编码
- [ ] 健康检查端点可用（`/health`, `/health/ready`）

### 数据库
- [ ] 迁移脚本已准备
- [ ] 回滚方案已确认

### 监控
- [ ] Prometheus metrics 端点可用
- [ ] 结构化日志格式正确

## 通过标准

所有检查项通过，或已有明确的风险接受记录。
