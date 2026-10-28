# ignored 测试豁免清单

> 门禁 30 配套：`scripts/audit/ignored-test-routing.js` 校验每个 `#[ignore]` 测试
> 必须满足以下任一条件，否则构建失败（fail-closed）：
> 1. 被某 workflow 的 `--ignored` 命令覆盖（`*.github/workflows/*.yml` 中
>    `cargo test ... -- ... --ignored` 显式列出测试文件/名称/包）；或
> 2. 登记在本豁免清单（下表中）。
>
> 豁免原则：仅限**外部凭据/收费服务/物理设备**类测试。DB/Redis 由
> `db-integration.yml` 提供 service，一律不许豁免。

| crate | 测试文件 | 豁免理由 | approver | approval_date | expiry_date |
|-------|---------|---------|----------|---------------|-------------|
| sz-rust-ai-facade | tests/real_api_test.rs | 需要真实 LLM API key（OpenAI 等收费服务），CI 无凭据；保持人工执行 | SDET 评测修复 | 2026-10-27 | 2027-01-27 |
| sz-rust-ai-facade | tests/openai_vision_real_test.rs | 需要真实 OpenAI Vision API key（收费服务），CI 无凭据；保持人工执行 | SDET 评测修复 | 2026-10-27 | 2027-01-27 |