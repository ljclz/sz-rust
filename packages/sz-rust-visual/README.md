# SZ-Rust 可视化画布 (`sz-rust-visual`)

> P2-3 生态件：Tauri 2.x 桌面应用，SDD 编排可视化 + Capability 管理 + RAG 搜索 + 应用预览。

## 架构

```
sz-rust-visual/
├── src/
│   ├── lib.rs          # Tauri Builder + run() 入口
│   ├── main.rs         # binary 入口
│   ├── error.rs        # VisualError 9 变体 + Serialize + error_code()
│   ├── sdd_facade.rs   # SddFacade trait (6 异步方法) + MockSddFacade
│   ├── models.rs       # PhaseEvent/SddSession/SddPhase/SddStatus/...
│   ├── commands.rs     # 10 个 #[tauri::command] 函数
│   ├── event_bridge.rs # SddEventBridge + tracing 日志转发
│   └── preview.rs      # PreviewService (axum 静态文件服务)
├── frontend/           # Vue 3 前端（待实现）
├── tauri.conf.json     # Tauri 配置（窗口/CSP/bundle）
├── build.rs            # tauri_build::build()
└── icons/              # 应用图标
```

## Tauri Command 清单

| Command | 参数 | 返回 | 说明 |
|---------|------|------|------|
| `sdd_start` | feature, requirement, start_phase | SddSession | 启动 SDD 编排 |
| `sdd_submit_review` | session_id, phase, decision, comment | SddSession | 提交 HITL 审查 |
| `sdd_cancel` | session_id | () | 取消编排 |
| `sdd_status` | session_id | SddSession | 查询编排状态 |
| `sdd_read_artifact` | session_id, phase | String | 读取产物 |
| `cap_list` | — | Vec<String> | Capability 列表 |
| `cap_call` | name, args | Value | Capability 调用 |
| `rag_search` | query | Vec<String> | RAG 搜索 |
| `preview_start` | feature, device | String | 启动预览 |
| `preview_stop` | url | () | 停止预览 |

## Tauri Event 清单

| Event | Payload | 说明 |
|-------|---------|------|
| `sdd_phase_event` | PhaseEvent | 阶段事件 |
| `sdd_log` | LogEvent | 日志事件 |
| `sdd_status_changed` | StatusChangedEvent | 状态变更 |

## SddFacade Trait

```rust
#[async_trait]
pub trait SddFacade: Send + Sync {
    async fn start(&self, feature: &str, requirement: &str, start_phase: SddPhase) -> VisualResult<SddSession>;
    async fn submit_review(&self, session_id: &str, phase: SddPhase, decision: ReviewDecision, comment: Option<&str>) -> VisualResult<SddSession>;
    async fn cancel(&self, session_id: &str) -> VisualResult<()>;
    async fn status(&self, session_id: &str) -> VisualResult<SddSession>;
    async fn subscribe_events(&self) -> VisualResult<broadcast::Receiver<PhaseEvent>>;
    async fn read_artifact(&self, session_id: &str, phase: SddPhase) -> VisualResult<String>;
}
```

- 开源版：`MockSddFacade`（占位实现）
- 企业版：`SddFacadeImpl`（路由到 `Orchestrator`，feature gate `enterprise`）

## 构建

### 开发模式

```bash
cd packages/sz-rust-visual
cargo tauri dev
```

### 生产构建

```bash
# Windows
cargo tauri build --target x86_64-pc-windows-msvc

# macOS
cargo tauri build --target aarch64-apple-darwin

# Linux
cargo tauri build --target x86_64-unknown-linux-gnu
```

### CI/CD

`.github/workflows/visual-build.yml` 三端矩阵构建，产物上传 GitHub Releases。

## 前端开发（待实现）

```bash
cd packages/sz-rust-visual/frontend
npm install
npm run dev    # Vite 开发服务器
npm run build  # 生产构建
```

组件树：Workbench → Canvas → {RequirementPanel, SpecPanel, DesignPanel, TaskBoard, LogPanel, PreviewPanel, PluginPanel}

## 测试

```bash
cargo test -p sz-rust-visual
# 21 passed; 0 failed
```

## 设计约束

- `#![forbid(unsafe_code)]`
- 所有 `async fn` 必须 `Send + 'static`
- SddFacade trait 解耦开源版/企业版
- 敏感字段 `#[serde(skip_serializing)]`
- CSP 策略：`default-src 'self'; connect-src 'self' ipc: http://ipc.localhost`