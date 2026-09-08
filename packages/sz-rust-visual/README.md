# SZ-Rust 可视化画布 (`sz-rust-visual`)

> P2-3 生态件：Tauri 2.x 桌面应用，SDD 编排可视化 + Capability 管理 + RAG 搜索 + 应用预览。

## 架构

```
sz-rust-visual/
├── src/
│   ├── lib.rs          # Tauri Builder + run() 入口 + CapabilityRegistry 注册
│   ├── main.rs         # binary 入口
│   ├── error.rs        # VisualError 9 变体 + Serialize + error_code()
│   ├── sdd_facade.rs   # SddFacade trait (6 异步方法) + MockSddFacade
│   ├── models.rs       # PhaseEvent/SddSession/SddPhase/SddStatus/...
│   ├── commands.rs     # 10 个 #[tauri::command] 函数（真实接线）
│   ├── event_bridge.rs # SddEventBridge + tracing 日志转发
│   └── preview.rs      # PreviewService (axum 静态文件服务 + 优雅关闭)
├── frontend/           # Vue 3 前端（已实现）
│   ├── src/
│   │   ├── api/tauri.ts           # Tauri invoke/listen 封装
│   │   ├── stores/                # Pinia stores (sdd/capability/plugin)
│   │   ├── components/            # Workbench + Canvas + 7 面板
│   │   └── i18n/                  # 中英文 i18n 资源
│   ├── package.json              # Vue 3 + Vite 4 + Pinia + Tauri API
│   └── vite.config.ts            # Vite 构建配置
├── tests/
│   └── e2e_flow.rs     # 端到端全流程集成测试（12 个测试）
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

## 前端开发

```bash
cd packages/sz-rust-visual/frontend
npm install
npm run dev    # Vite 开发服务器
npm run build  # 生产构建
```

组件树：Workbench → Canvas → {RequirementPanel, SpecPanel, DesignPanel, TaskBoard, LogPanel, PreviewPanel, PluginPanel}

### Pinia Stores

| Store | 职责 |
|-------|------|
| `sdd.ts` | SDD 编排状态管理（start/submitReview/cancel/status/readArtifact） |
| `capability.ts` | Capability 列表与调用（capList/capCall/ragSearch） |
| `plugin.ts` | 插件市场交互（search/install/uninstall/update/login） |

## 测试

```bash
# 全量测试
cargo test -p sz-rust-visual
# 38 passed; 0 failed (26 unit + 12 e2e)

# 仅端到端测试
cargo test -p sz-rust-visual --test e2e_flow
# 12 passed; 0 failed
```

### 端到端测试覆盖

| 测试 | 验证内容 |
|------|---------|
| `e2e_sdd_full_flow_start_review_complete` | SDD 启动 → 四阶段 HITL 审查 → 状态查询 |
| `e2e_sdd_review_with_modify_decision` | Modify/Supplement 审查决定 |
| `e2e_sdd_cancel_session` | 取消编排 |
| `e2e_sdd_read_artifact` | 读取产物 |
| `e2e_sdd_subscribe_events` | 事件订阅 |
| `e2e_preview_start_http_accessible_stop_releases` | 预览 HTTP 服务可访问 + stop 释放端口 |
| `e2e_preview_device_viewport_variants` | Desktop/Tablet/Mobile 三种设备 |
| `e2e_preview_artifact_not_found` | 产物不存在时返回 ARTIFACT_NOT_FOUND |
| `e2e_capability_list_and_call` | Capability 列表 + mcp.url_decode 真实调用 |
| `e2e_phase_event_serialization` | PhaseEvent 序列化/反序列化 |
| `e2e_sdd_session_trace_id_skip_serializing` | trace_id 脱敏验证 |
| `e2e_complete_four_phase_orchestration` | 完整四阶段编排模拟 |

## 设计约束

- `#![forbid(unsafe_code)]`
- 所有 `async fn` 必须 `Send + 'static`
- SddFacade trait 解耦开源版/企业版
- 敏感字段 `#[serde(skip_serializing)]`
- CSP 策略：`default-src 'self'; connect-src 'self' ipc: http://ipc.localhost`