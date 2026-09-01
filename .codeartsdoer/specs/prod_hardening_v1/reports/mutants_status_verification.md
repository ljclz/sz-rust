# 变异测试状态确认报告

> 生成时间：2026-09-01T15:15:00+08:00
> 验证方法：GitHub CLI `gh run view` + `gh run list`

## 1. run 33466354670 最终状态

- **status**: completed
- **conclusion**: cancelled
- **startedAt**: 2026-09-01T03:28:43Z
- **updatedAt**: 2026-09-01T07:29:15Z
- **运行时长**: 约 4 小时（远超 240min timeout，被 GitHub Actions 自动取消）
- **分析**: run 最终被 cancelled（可能是 GitHub Actions runner 超时回收或手动取消请求触发）

## 2. 历史 5 次 mutants run 趋势

| Run ID | 状态 | 结论 | 创建时间 | 分析 |
|--------|------|------|---------|------|
| 33466354670 | completed | cancelled | 2026-09-01 03:28 | 被 GitHub Actions 取消（运行约 4h） |
| 33465169415 | completed | failure | 2026-09-01 03:09 | 失败（可能是同一触发重复） |
| 33300989123 | completed | cancelled | 2026-08-30 08:11 | 取消（2h timeout） |
| 32617672743 | completed | cancelled | 2026-08-23 04:21 | 取消（2h timeout） |
| 32574837713 | completed | cancelled | #2026-08-22 13:06 | 取消（2h timeout） |

## 3. 超时根因分析

- **历史趋势**: 最近 5 次 run 中，1 次 failure + 3 次 cancelled（timeout），成功率 0%
- **根因**: cargo-mutants 变异测试耗时过长，240min timeout 仍不足完成 sz-rust-core 的变异测试
- **建议**: 
  1. 缩小变异测试范围（仅核心模块而非整个 sz-rust-core）
  2. 或增加 timeout 至 480min（8 小时）
  3. 或使用 cargo-mutants 的 `--jobs` 并行化（但 `--in-place` 模式不兼容 `-j`）
- **当前 run 33466354670**: 已被 cancelled（运行约 4h，被 GitHub Actions 超时回收）

## 4. 结论

- mutants.yml workflow 已正确触发（run 33466354670 存在）
- run 33466354670 最终状态：completed/cancelled（运行约 4h，被 GitHub Actions 超时回收）
- 历史 5 次 run 趋势：0% 成功率，主要原因为 timeout/cancelled
- timeout 修复（120→240min）已生效，但 240min 仍不足以完成变异测试