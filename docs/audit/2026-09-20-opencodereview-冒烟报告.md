# OpenCodeReview 接入冒烟报告（2026-09-20）

> **触发**：评估阿里开源 OpenCodeReview（ocr CLI v1.12.7，github.com/alibaba/open-code-review）能否作为 `/sz-rust-review` 的第二意见。文章↔仓库交叉核验结论：delegate 模式零新增 Key、精确率优先、AACR-Bench 50 仓/200 PR/10 语言/1505 标注问题。
> **方法**：委托模式（delegate，零 API Key）端到端演练——提取历史漏洞态 → OCR 文件选择/规则路由 → **未污染子代理**按其规则评审（避免评审者已知 ground truth 的自证污染）→ 逐条裁定。
> **审查时点**：HEAD `53bfa22`；漏洞态取自 `98ec48e^`（白帽审计 H-1/H-2/M-3 修复前，2026-08-14）。
> **冒烟仓库**：`F:/tmp-ocr-smoke`（`git archive 98ec48e^` 提取 sz-rust-http-facade + sz-rust-mvc-facade 全量，18 个 .rs，独立 git 仓库）。

---

## 一、管道验证（4/4 通过）

| 项 | 结果 | 证据 |
|---|---|---|
| Rust 文件选择 | ✅ | `ocr delegate preview`：19 reviewable / 44 total，**.rs 全部入选**；.md（unsupported_ext）与 tests/（default_path）按官方文档排除 |
| Rust 原生规则路由 | ✅ | `ocr delegate rule` 返回 `system / **/*.rs` 规则组：所有权/生命周期、生产路径 unwrap/panic、unsafe 边界、**跨 .await 持锁**、异步取消安全（同步 fs/network 进请求路径）、认证代码必须用成熟 crate —— 与铁律 2（unwrap）/铁律 4（std::fs）天然对齐 |
| 委托模式零 Key | ✅ | OCR 只做文件选择+规则解析，推理由宿主 Agent 自有模型执行 |
| Windows + Cargo workspace 实跑 | ✅ | v1.12.7 (85cecfe) windows/amd64 全流程无兼容问题 |

**结论：README 未列语言清单的疑虑解除——Rust 是一等支持语言（规则级实证）。**

## 二、检测能力标定（对照 ground truth）

**种子缺陷回放（98ec48e 修复的两处漏洞态）**：

| 种子 | 结果 | 归因 |
|---|---|---|
| H-2：`fetch_post_data` 无上限读 body → OOM | **漏抓** | 规则组无「资源耗尽/无上限读取」类别 |
| M-3：JWT 弱密钥（<32 字节）不拒绝启动 | **漏抓** | 规则组无「密钥强度」类别 |

官方「精确率优先、召回率刻意偏低」的实证；可通过 OCR 自定义规则补强（格式待验证，见遗留 #3）。

**子代理按 OCR 规则组评审产出 9 条发现，抽验 4/4 属实、0 误报**（对照：2026-08-16 外部 AI 审查 19/20 指控不可复现）：

| # | 发现 | 定级 | 裁定（对照 HEAD `53bfa22`） |
|---|---|---|---|
| 1 | `SZ_JWT_AUDIENCE` 加载后 verify 从不校验 aud，字段 doc 承诺「非空必匹配」 | high | **属实，现行缺陷**（controller.rs:78-79 doc 承诺；:109-110 加载；verify_token_with_config 仅查 iss）。缓解：模块头注释交代 sz-orm-auth JwtClaims 暂无 aud 字段、由业务层实现。定性：**配置假安全感 + 文档失实**——设置了该变量的部署以为 aud 校验生效。修复方向待裁定（诚实化文档+移除 no-op 加载 vs 升级 sz-orm-auth JwtClaims 补 aud 字段） |
| 2 | `strip_bearer_prefix` 的 `&trimmed[..6]` 字节切片在多字节字符边界 panic | medium | **属实，现行缺陷（潜在）**（HEAD:142-149）。可达性取决于调用方是否保证 header 为 ASCII（`HeaderValue::to_str` 保证，但 trait 方法接受任意 `&str`）；建议防御性修复（`get(..6)`） |
| 3 | `url_decode` 丢 `%` 后字符（`100%a`→`100%`，与「非法十六进制保留原样」注释矛盾） | medium | **属实（漏洞态）**；已随 `03415d1` 拆包重构顺带修复（HEAD else 分支补 `push_char_utf8`）——真实历史缺陷 |
| 5 | `u8::from_str_radix` 接受前导 `+`：`%+1` 解码为 0x01 而非保留原样 | low | **属实，现行**（HEAD url_decode 仍用 from_str_radix） |
| 4/6/7/8/9 | 轮转零间隔 → spawn 内 `tokio::time::interval` panic 且绕过错误日志 / do_rotation 双锁窗口 / get_token doc 承诺 Err 从不返回 / 公共边界字符串错误类型 / 双 JWT 配置体系并存 | med~low | 类别合理、引用行号属实，未逐行复验（后续裁定） |

## 三、判定与接入

**冒烟通过（有保留）**：管道 4/4 全绿 + 0 误报 + 顺手抓到 2 个现行缺陷；种子漏抓 2/2 归因规则覆盖缺口，属可补项而非硬伤（已修复的历史缺陷不构成接入障碍，但标定了「OCR 不能替代机械门禁」的边界）。

**接入形态（本次落地）**：
- **委托模式进 `/sz-rust-review --ocr`（实验性开关）**——不进 bash 门禁流水线（standalone 模式需单独配 key 且重复消耗模型额度；delegate 的推理由宿主 Agent 承担，恰好复用既有 AI 评审策略）。
- findings 全部记 **low、标注「OCR 委托评审（仅供参考）」、不参与阻塞判定**，逐条裁定（可落地→采纳修复；流程教训→不采纳但理由入报告）。
- **数据边界**：本地 CLI，但 standalone 模式会把代码发送到其配置的模型端点；委托模式代码只进入宿主 Agent 上下文。**企业版仓库代码禁送外部端点**，本开关仅用于开源仓库/公开代码。

## 四、遗留

1. ~~两个现行缺陷待用户裁定~~ → **2026-09-21 用户裁定"全部修复好"，当日全部处置**（7590a1f + 后续提交）：
   - #1 aud 假配置 → 诚实化（移除 no-op 配置，真校验登记 doc-debt DB-2026-09-21-01 待 sz-orm-auth 上游补 aud 字段）
   - #2 bearer 切片 panic → `get(..6)` 防御 + 多字节回归测试
   - #4 轮转零间隔 panic → from_env 告警回退默认 + new() 钳制（`rotation_interval` 恒非零）
   - #6 轮转双锁窗口 → do_rotation 单锁域原子化
   - #7 get_token 文档假承诺 → 如实化（格式错误一律 Ok(None)）
   - #5 `%+X` 解码 0x01 → 十六进制白名单修复（`is_ascii_hexdigit` 先行判定 + 回归测试，2026-09-21）
   - #9 双 JWT 配置体系 → 裁定不重构，结构文档如实标注双体系关系与接线方式
   - #8 字符串错误类型 → 裁定不采纳（公共 API breaking change，LOW 收益不成比例，留待 facade 大版本演进）
   - 验证：mvc-facade **414 lib + 12 + 8 全绿**，http-facade **168 全绿**，fmt/clippy 0 error。**至此 9 条发现全部闭环（4 修复 + 1 文档修正 + 2 裁定不采纳 + 1 上游 doc-debt + 1 历史已修）**
2. standalone 端到端（OCR 自有模型）未测——需 key：交互式 `ocr config provider` 后在真实仓库 `ocr review --from 98ec48e~1 --to 98ec48e` 可复跑本冒烟。
3. 自定义规则补强（unwrap/std::fs/资源上限/密钥强度四类）待验证 OCR 规则自定义格式。
4. 冒烟仓库 `F:/tmp-ocr-smoke` 保留备查，确认后可删。
5. 记忆勘误链：A16 修复提交 `318b29b` 与 H 系列修复 `1152ba4` 等哈希在全部可达仓库均不存在（又一处记忆转写幻影）；真实哈希以本文 `98ec48e`（H-1/H-2/M-3，08-14 20:00）与 `2e45dc1`（A6 复核，08-14 21:15）为准。

---
*生成 2026-09-20，核验会话；证据：F:/tmp-ocr-smoke/rules_output.txt + 子代理评审全文（agent_9322b683）+ git show 98ec48e。*
