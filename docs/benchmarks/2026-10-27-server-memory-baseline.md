# sz300-server 独立进程内存基线（2026-10-27）

> 关联全栈评测报告：[2026-10-27-全栈工程师评测报告](../audit/2026-10-27-全栈工程师评测报告.md)
> 铁律 9：启动内存 <30MB 空载 RSS。
> 数据产物：`docs/benchmarks/server-memory-baseline-2026-10-27.json`

## 结论

**通过。** `sz300-server` release 二进制独立进程空载 RSS 稳态 **≈9.39 MB**（6 次采样
9.39–9.40 MB），远低于铁律 9 的 30MB 阈值。`/health` 与 `/health/ready` 均返回
HTTP 200（`db: ok`），说明服务在 MySQL 连接正常的情况下启动并保持极低内存占用。

## 方法

1. 构建：`cargo build --release -p sz-rust-sz300 --bin sz300-server`
   （CARGO_TARGET_DIR=D:\rust-target）。
2. 启动：独立进程，注入环境变量 `SZ300_DB_USER/SZ300_DB_PASSWORD/SZ300_DB_NAME/
   SZ300_JWT_SECRET/SZ_JWT_SECRET`（凭据来自用户服务器信息文档，不回写本仓库）。
3. 采样：启动后等待 8 秒（连接池初始化完成），用 `Get-Process.WorkingSet64`
   每 2 秒采样一次，共 6 次。
4. 探测：`GET /health`、`GET /health/ready`（readiness 会执行 DB `SELECT` 探活）。
5. 停止进程，输出 JSON 报告。

## 实测数据（2026-10-27 21:10 本地）

| 项 | 值 |
|---|---|
| 二进制 | `D:\rust-target\release\sz300-server.exe`（release，v2.0.0） |
| RSS 采样 | 9.40, 9.39, 9.39, 9.39, 9.39, 9.39 MB |
| min / max / avg | 9.39 / 9.40 / 9.39 MB |
| `/health` | HTTP 200（status ok） |
| `/health/ready` | HTTP 200（checks.db = ok） |
| 结论 | PASS（<30MB） |

## 回归脚本

`scripts/server_memory_baseline.ps1` 已提交仓库，支持：

- 自动定位 release 二进制（优先 `CARGO_TARGET_DIR`，其次 `<repo>/target/release`）；
- 凭据全部从环境变量读取（`SZ300_DB_PASSWORD` / `SZ300_JWT_SECRET` 必填），
  **脚本内不硬编码任何密钥**；
- 输出 JSON 到 `reports/server-memory-baseline-<timestamp>.json`；
- RSS 峰值超过 30MB 时以非零退出码失败（可直接挂 CI 门禁）。

运行方式：

```powershell
$env:SZ300_DB_PASSWORD='...'   # 本机 MySQL 凭据（用户提供）
$env:SZ300_JWT_SECRET='...'
.\scripts\server_memory_baseline.ps1
```

## 备注

- 实测对象为 release 空载 RSS；若改用 debug 构建，RSS 会更高（参考
  `tests/windows_memory_baseline.rs` 测试进程约 15.9MB 的数值）。
- 本机 MySQL 为宝塔 MySQL 8.0（`D:\BtSoft\mysql\MySQL8.0`），`root:test123` 凭据已失效；
  实测使用 `shop` 用户可连接（见任务 4 多库连通验证）。
- 该基线可在任意 Windows 机器复跑；Linux CI 挂载可用同脚本（需改 WorkingSet 采集方式）或
  使用 `sysinfo` 编写等价 Rust 测试。