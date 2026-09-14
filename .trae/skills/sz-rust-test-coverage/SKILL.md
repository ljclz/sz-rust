---
name: sz-rust-test-coverage
description: 测试覆盖率门禁 — 确保新增代码测试覆盖率达到基线。修改业务代码时触发。
tools: [cargo-tarpaulin, cargo-llvm-cov]
agentMode: auto
---

# 测试覆盖率门禁（sz-rust）

## 触发条件

- 修改 `packages/sz-rust-sz300/src/` 或 `packages/sz-rust-addons-*/src/` 中的业务逻辑
- 新增 controller / service / model

## 检查步骤

1. 运行覆盖率：`cargo llvm-cov --lcov --output-path lcov.info`
2. 检查新增/修改文件的行覆盖率是否 >= 80%
3. 分支覆盖率是否 >= 70%

## 通过标准

- 新增业务代码测试覆盖率 >= 80%
- 关键路径（auth、payment、data mutation）覆盖率 >= 90%
- 无未测试的 `unwrap()` 调用

## 失败处理

覆盖率不足时，补充单元测试或集成测试，直到达标。
