# Soak 自托管通用工具

> 版本：2.0（参数化通用版）
> 部署位置：`/www/rust/soak-toolkit/`

## 工具清单

| 脚本 | 功能 |
|------|------|
| `config-defaults.sh` | 默认值定义（唯一允许 sz-rust 硬编码的位置） |
| `process-guard.sh` | 进程隔离保护（预检查/后检查/清理） |
| `soak-archive.sh` | 报告归档与索引管理 |
| `soak-runner.sh` | Soak 测试执行（参数化） |
| `soak-cron-setup.sh` | cron 调度配置 |
| `soak-trigger.js` | 本地触发器（SSH 上传+执行） |
| `verify-6h-soak.sh` | 6h soak 验证脚本 |

## 参数清单

### soak-runner.sh

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--duration` | 10s | 测试持续时间 |
| `--trigger` | manual | 触发方式（manual/cron/nightly） |
| `--project` | sz-rust | 项目名（小写字母/数字/连字符） |
| `--protected-port` | 8300 | 被保护进程端口 |
| `--protected-process` | sz-rust-sz300 | 被保护进程名 |
| `--work-dir` | /www/rust/sz-rust-soak | 工作目录 |
| `--report-dir` | /www/rust/soak-reports | 归档目录 |
| `--soak-ports` | 8401-8405 | 采样端口范围 |
| `--restart-script` | /www/rust/sz-rust-soak/restart-sz300.sh | 重启脚本 |
| `--cron-marker` | # sz-rust-soak | cron 标记 |

### soak-trigger.js

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--key-path` | ../../deploy_key | SSH 密钥路径 |
| 其他参数同 soak-runner.sh | | 透传到 soak-runner.sh |

## 调用示例

### sz-rust 默认参数（向后兼容）

```bash
node soak-trigger.js --duration 10s --trigger manual
```

### sz-pay 参数化复用

```bash
node soak-trigger.js --duration 10s --project sz-pay --port 8301 \
  --work-dir /www/rust/sz-pay-soak --report-dir /www/rust/soak-reports \
  --soak-ports 8406-8410 --protected-process sz-pay-server \
  --restart-script /www/rust/sz-pay-soak/restart.sh --cron-marker "# sz-pay-soak"
```

### cron 调度配置

```bash
bash soak-cron-setup.sh setup --cron-marker "# sz-rust-soak" \
  --soak-runner /www/rust/soak-toolkit/soak-runner.sh \
  --work-dir /www/rust/sz-rust-soak
```

## 部署架构

```
/www/rust/soak-toolkit/          # 通用工具（共享）
├── config-defaults.sh
├── process-guard.sh
├── soak-archive.sh
├── soak-runner.sh
├── soak-cron-setup.sh
└── verify-6h-soak.sh

/www/rust/sz-rust-soak/          # sz-rust 工作目录
/www/rust/sz-pay-soak/           # sz-pay 工作目录
/www/rust/soak-reports/          # 归档目录（共享）
├── index.csv
└── 2026-08-09/
    └── soak-report-*.csv
```