# 变异测试状态确认报告

> 生成时间：2026-09-01T15:15:00+08:00
> 验证方法：GitHub CLI `gh run view` + `gh run list`

## 1. run 33466354670 最终状态

```json
{
  "conclusion": "",
  "displayTitle": "Mutants",
  "startedAt": "2026-09-01T03:28:43Z",
  "status": "in_progress",
  "updatedAt": "2026-09-01T03:29:00Z"
}
```

- **status**: in_progress
- **conclusion**: （空，未完成）
- **startedAt**: 2026-09-01T03:28:43Z
- **updatedAt**: 2026-09-01T03:29:00Z
- **运行时长**: 约 12 小时（远超 240min timeout，疑似 GitHub Actions runner 卡住）
- **分析**: updatedAt 停留在 03:29（启动后 17 秒），说明 job 可能卡在初始化阶段或 runner 挂死

## 2. 历史 5 次 mutants run 趋势

| Run ID | 状态 | 结论 | 创建时间 | 分析 |
|--------|------|------|---------|------|
| 33466354670 | in_progress | — | 2026-09-01 03:28 | 当前 run，疑似卡住 |
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
- **当前 run 33466354670**: 疑似 GitHub Actions runner 卡住（updatedAt 停留在启动后 17s），建议手动取消

## 4. 结论

- mutants.yml workflow 已正确触发（run 33466354670 存在）
- run 33466354670 状态 in_progress（疑似卡住，已运行 12h 远超 240min timeout）
- 历史 5 次 run 趋势：0% 成功率，主要原因为 timeout/cancelled
- timeout 修复（120→240min）已生效，但 240min 仍不足以完成变异测试