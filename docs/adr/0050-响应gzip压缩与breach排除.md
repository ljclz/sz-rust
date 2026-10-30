# ADR-050：响应 gzip 压缩（含令牌端点 BREACH 排除）

> **状态**：已接受（能力落地，**默认关闭**，默认值待同源 A/B 实测后确定）
> **日期**：2026-10-30
> **决策者**：SZ-Rust Team
> **相关代码**：`packages/sz-rust-sz300/src/middleware/compression.rs`、`packages/sz-rust-sz300/src/router.rs`、`packages/sz-rust-sz300/src/config.rs`、`packages/sz-rust-sz300/Cargo.toml`、`packages/sz-rust-sz300/tests/compression_test.rs`
> **关联 ADR**：[ADR-046](0046-性能工程整改-限流分桶与熔断窗口与门禁重写.md)（性能工程整改）、[ADR-049](0049-鉴权授权上下文缓存.md)（鉴权授权上下文缓存）、[ADR-045](0045-数据归属过滤与令牌生命周期安全加固.md)（令牌签发与 WS/SSE 鉴权）

## 背景

《2026-10-29 性能工程评测报告》§6 瓶颈 3 列出的第四条成本是**响应不压缩**：`Accept-Encoding: gzip, br` 下 `product/list` 的 5,314 B 响应原样出网（当时 `packages/sz-rust-sz300/Cargo.toml` 的 tower-http 只启 `cors`），2.1k rps 折算约 11 MB/s。31–60 天路线项写的是「开 gzip/brotli（注意 BREACH）」。

动手前有两项与本决策直接相关的事实：

- **生产形态经过反向代理**：`deploy/k8s/sz300-ingress-configmap.yaml` 用 nginx ingress controller 暴露 `sz300-service`，清单里没有任何 gzip 注解 —— 边缘是否压缩、压哪些 MIME、阈值多少，取决于控制器自身的 ConfigMap，本仓库不可见、不可证；而开发环境与客户端直连（本机 8300、SDK 直连）完全不过代理。
- **BREACH 前提在本项目确实成立**：`packages/sz-rust-sz300/src/controllers/auth.rs` 登录成功响应把请求体里的 `username` 原样回显（`"username": username`），同一条响应内还有本次生成的 `token` / `refresh_token`。「攻击者可控明文 + 每次请求变化的密钥出现在同一份被压缩明文里」正是 BREACH 的成立条件。错误分支不回显输入（`auth_service` 的失败文案固定为「用户名或密码错误」），所以风险集中在签发成功的响应上。

## 决策

1. **压缩实现**：使用 tower-http 0.7 的 `CompressionLayer`（sz300 显式声明 `compression-gzip` feature），仅当客户端 `Accept-Encoding` 含 gzip 时压缩，并自动处理 `Vary: Accept-Encoding`。
2. **默认关闭**，由环境变量 `SZ300_COMPRESSION` 开启（`config::compression_enabled()`，`1/true/yes/on` 为真，未设置或非法值为假 = fail-closed）。理由有两条：
   - 压缩是「出网字节 ↓ / CPU ↑」的交换，评测报告的「出网 −70~80%」是估算值（W 级），本机同源 A/B 尚未跑；在没有 c=50 / 60 s × 3 的 QPS 与 p99 数据前改变线上默认行为，违反「性能结论需实测证据」的既有约定。
   - 默认值本身要由 A/B 决定，因此同一二进制必须能跑两种配置 —— 开关是这次测量的前置条件，不是临时垫片。
   - 关闭时该层**不进入服务栈**（`create_router` 里是 `if` 分支而非运行时判断），现网行为与门禁基线均不受影响。
3. **只做 gzip，暂不做 brotli**：`compression-br` 会把 brotli 编解码拉进依赖图（当前 `compression-codecs` 只编到 `flate2`），且 brotli 默认质量档 CPU 明显高于 gzip。在 gzip 的 A/B 数据出来之前同时引入「新依赖 + 新编码」等于叠加两个未测变量。若 gzip 实测后 CPU 代价可接受而出网收益仍未达目标，再单独立项评估 brotli。
4. **谓词沿用 tower-http `DefaultPredicate`**，即自动跳过：小于 32 B 的响应（`/health` 一类，压不出收益只烧 CPU）、`application/grpc`、`image/*`、以及 `text/event-stream`。最后一条是硬约束：`/events` 与 `/api/v1/ai/stream`（`controllers/ai.rs` 里 content-type 为 `text/event-stream`）是长驻分块流，压缩会缓冲掉事件即时性与 15 s keep-alive。
5. **令牌端点排除（BREACH 防线）**：`/api/v1/auth/login` 与 `/api/v1/auth/refresh` 两条路由挂 `middleware::from_fn(mark_no_compress)`，由它给响应插入 `NoCompression` 扩展；压缩层用 `DefaultPredicate.and(not_marked)` 读到该标记即原样放行。三点设计理由：
   - tower-http 的谓词只能看到**响应**（status/version/headers/extensions），看不到请求路径，因此按路径排除必须靠响应扩展传递，不能只写一个路径黑名单谓词。
   - 标记只在被挂载的那两条路由上产生，业务读接口（`product/list`、`order/list`）的热路径不经过它。
   - 压缩层位于脱敏层**之外**（响应先过脱敏、再过压缩），`DataMaskLayer` 用 `Response::from_parts` 重建响应，扩展因此能传播到压缩谓词。`tests/compression_test.rs` 按这个真实层次（脱敏层夹在中间）验证，而不是验证一个更简单的假想栈。
6. **`/metrics` 不做排除**：text/plain 且体积可观，Prometheus 抓取自带 `Accept-Encoding: gzip`；抓取频率低，CPU 摊销可忽略。

## 替代方案

| 方案 | 做法 | 未采纳理由 |
|------|------|-----------|
| A. 只在边缘（ingress-nginx）开 gzip | 应用不管压缩，交给反向代理 | 本仓库不可证：清单无 gzip 注解，实际行为随集群控制器 ConfigMap 漂移；开发/直连场景完全不过代理，等于同一套代码两种出网形态。更关键的是边缘不知道该排除哪条响应，BREACH 防线要落实就得把「哪条业务响回显用户输入」写进基础设施注解 —— 业务语义泄漏到部署层 |
| B. gzip + brotli 同时开 | `compression-full` 一次到位 | 新增 brotli 编解码依赖 + 两种编码的 CPU 差异未测，两个未测变量混在一轮 A/B 里无法归因（见决策 3） |
| C. 默认开启，待实测再决定关闭 | 先上线拿收益 | 未实测的性能变更默认上线，且门禁基线会把压缩成本静默吸收成「现状」，后续任何回归都测不出来 |
| D. 逐个 handler 手工压缩 | 控制器里调 gzip 工具函数 | 每个新接口都是一次遗漏机会，压缩必须在唯一收口点（服务栈）生效 |

## 后果

### 正面后果

- 具备按响应内容类型/尺寸自动决策的 gzip 能力，且令牌签发响应被显式排除，压缩与 BREACH 防线可以在同一处审查。
- 默认关闭 → 现网默认行为、门禁基线、`/health` 一类小响应均未改变；开启只需一个环境变量，同一二进制即可完成 A/B。
- 打开该能力顺带暴露并修复了 `v18-sse` feature **从未被编译**的问题（见注意事项）。

### 负面后果

- 启用后每个被压缩的响应多一次 gzip 编码（CPU + 一次缓冲）。**实测（V，2026-10-30，`docs/audit/2026-10-29-性能整改现场验证记录.md` §17）**：同一 `sz300-server-post049.exe` 仅切 `SZ300_COMPRESSION`，闭环 c=50 下 `/health` +1.73%、`product/list` −0.19%、`order/list` +2.11%（p99 +4.57% / +1.65% / −1.44%），三端点差异都**小于同配置重复腿的实测散布**（`/health` ≤5.60%、`product` ≤8.35%、`order` ≤11.97%）→ 结论只能写到「该口径下测不出吞吐代价」，**不等于零 CPU 代价**（服务端进程 CPU 秒/千请求未单独采集，该项 W）。
- **出网字节实测（V）**：`product/list` 4,692 → 800.8 B/req（**−82.93%**）、`order/list` 5,518 → 878.5（**−84.08%**）、`/health` 115 → 113.4（−1.39%，正文太小，gzip 容器开销几乎吃掉收益），整腿归一 2,444.9 → 445.2 B/req（−81.79%）。**路线项承诺的「出网 −70~80%」已兑现并略超**；该比例与首轮受争用批次差 ≤0.2 个百分点（字节核算与机器负载无关），且服务端→MySQL 的 `Bytes_sent` 两腿一致（2,514.4 vs 2,518.8 MB），证明节省全部发生在 HTTP 出口。
- **默认值仍为关闭**：实测虽显示「无可测吞吐代价 + 约 −82% 出口字节」，但把默认值翻转为开启属于「改变性能基线语义 + 改变线上协议行为」的决策，且 CPU 代价的直接度量与边缘代理重复压缩（下文）两项前置尚未闭环——翻转需单独批准，不在本 ADR 的落地提交内顺手做。
- 压缩后的响应体长度与 `Content-Length` 由 tower-http 改写；与脱敏层（自己会重算 `Content-Length`）叠加时以脱敏后的长度为准，测试覆盖了这一条链路。
- 多实例无共享状态，但边缘代理若再做一层压缩会白烧 CPU —— 上线前应确认 ingress-nginx 的 `enable-gzip` 实配（本仓库清单无法证明其状态）。

## 注意事项

- **压测客户端默认不发 `Accept-Encoding`**：`scripts/perf-regression.js` 的 `headersFor()` 只带 Content-Type / Authorization / Cookie；2026-10-30 起可用 `--accept-encoding gzip` 显式开启该头，并在结果里记录 `content_encoding_received` 与 `bytes_per_req`（默认仍不下发，保持与既有基线同口径）。因此当前基线数字**不含压缩路径**，若直接复跑门禁会得出「开压缩毫无影响」的假结论。做 A/B 必须显式带上 `Accept-Encoding: gzip`，并同时记录响应字节数（QPS 与出网是两个方向的指标）。
- **`v18-sse` feature 此前从未被 CI 编译**（`.github/workflows/` 中没有启用该 feature 的 job），2026-10-30 为验证 SSE 排除而打开它，暴露 4 处编译错误：`Event` 未导入、`IntoResponse` 未导入、`Sse` 未按返回类型 `into_response()`、`stream::iter` 闭包的 `Ok(_)` 缺错误类型注解。已在本次提交内修复，使 `cargo check -p sz-rust-sz300 --features v18-sse --all-targets` 通过；其**运行时行为仍未验证**（`tests/sse_test.rs` 在无 MySQL 时软跳过，且默认 feature 组合不含 `v18-sse`）。后续要么把该 feature 纳入某条 CI job，要么登记为已知未编译面。
- 若将来改为默认开启，必须同步：门禁基线重播种（压缩改变了被测配置）、`docs/audit/2026-10-29-性能整改现场验证记录.md` 的结论、以及本 ADR 状态行。
- 新增令牌签发类端点时，若响应里回显任何请求可控文本，必须一并挂 `mark_no_compress`。

## Bug 定位提示

- 客户端拿到 `Content-Encoding: gzip` 却期望明文：先查 `SZ300_COMPRESSION` 取值（`1/true/yes/on` 才开启），再看客户端是否发了 `Accept-Encoding`。
- 令牌响应被压缩了（BREACH 防线失效）：确认该路由挂了 `mark_no_compress`；再检查中间是否有层用 `Response::new(...)` 重建响应而丢掉扩展（`Response::from_parts` 保留，`Response::new` 不保留）—— `sz-rust-data-mask` 的 body 读取失败分支就是后者，会返回 502 且不携带标记，属可接受（该响应无密钥）。
- SSE 事件不再即时到达：确认响应 `content-type` 确为 `text/event-stream`（`DefaultPredicate` 靠它排除压缩）。
- 关键代码路径：`packages/sz-rust-sz300/src/middleware/compression.rs`（`NoCompression` / `not_marked` / `compression_layer` / `mark_no_compress`）、
  `packages/sz-rust-sz300/src/router.rs`（`create_router` 里的开关分支与两条 auth 路由的标记层）、
  `packages/sz-rust-sz300/tests/compression_test.rs`（5 个用例：业务 JSON 被 gzip、令牌路由跳过、<32 B 跳过、SSE 跳过、无 Accept-Encoding 时原样）。
