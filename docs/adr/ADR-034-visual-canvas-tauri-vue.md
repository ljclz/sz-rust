# ADR-034: 可视化画布 Tauri+Vue

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-visual/src/preview.rs`, `packages/sz-rust-visual/src/event_forwarder.rs`, `packages/sz-rust-visual/src/hitl_router.rs`

## 背景

P2-1 缺口：SDD Agent 缺乏可视化编排界面，开发者无法直观地看到规格、任务进度、实时日志和应用预览。

## 决策

1. **Tauri 桌面壳**：Rust 后端 + WebView 前端，冷启动 ≤ 5s
2. **Vue 前端**：六大功能区域独立组件
3. **事件流订阅**：EventForwarder 订阅 SDD PhaseEvent，支持按阶段/级别过滤
4. **HITL 路由**：HitlResponseRouter 支持 Confirm/Modify/Supplement/Abort，超时 30 分钟
5. **应用预览内嵌浏览器**：preview_app 启动应用进程→等待就绪→加载到 WebView

## 替代方案

- **Electron**：体积大，内存占用高
- **Web 纯浏览器**：无法启动本地应用进程和文件系统访问

## Bug 定位提示

- `preview.rs:80` — `preview_app` 方法，tokio::process::Command 启动应用
- `event_forwarder.rs:52` — `spawn_with_filter` 事件过滤转发
- `hitl_router.rs:14` — `HitlResponse` 枚举，含 Abort 变体
- `hitl_router.rs:68` — `wait_with_timeout` 超时等待

## 影响

- 56 tests passed（41 lib + 4 integration + 11 visual_test）
- 六大功能区域：需求描述/规格可视化/任务看板/实时日志/插件管理/应用预览
- Tauri 冷启动测量脚本 `scripts/measure_tauri_startup.ps1`