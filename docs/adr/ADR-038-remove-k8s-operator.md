# ADR-038：移除 sz-rust-k8s-operator 孤儿 crate

> **状态**：已决策
> **日期**：2026-08-21
> **决策者**：sz-rust 团队
> **影响**：workspace 成员 37 → 36

## 背景

2026-08-21 综合深度审计发现 `sz-rust-k8s-operator` 是孤儿 crate：

- **无消费者**：全仓库无任何 crate 依赖它（`grep -rn "sz-rust-k8s-operator" packages/ --include="*.toml"` 仅自身 Cargo.toml 命中）
- **无生产入口**：sz300 不依赖 k8s-operator，无路由/端点/初始化代码引用
- **CI 无 K8s 集群**：GitHub Actions workflow 无 K8s 环境配置，k8s-operator 的 22 个测试仅在本地运行
- **reconcile.rs:33 TODO**：自承认测试覆盖缺口（Err 分支未覆盖）

## 决策

**移除 sz-rust-k8s-operator crate**（方案 B）。

移除范围：
- `Cargo.toml` members 数组移除 `"packages/sz-rust-k8s-operator"`
- `Cargo.toml` workspace.dependencies 移除 `sz-rust-k8s-operator`、`k8s-openapi`、`kube`、`kube-derive`
- `packages/sz-rust-k8s-operator/` 目录保留在 git 历史（22 测试可追溯）

## 理由

1. **违反铁律**："任务完成必须是有入口、有接线、能运行、能观测" — k8s-operator 无入口无接线
2. **不破坏 sz-pay 兼容性**：sz-pay 不依赖 k8s-operator
3. **不回退 CI**：CI 22 jobs 不含 k8s-operator 测试
4. **维护成本**：孤儿 crate 的依赖（kube 0.96/k8s-openapi 0.23）增加 Cargo.lock 膨胀

## 后果

- workspace 成员 37 → 36
- Cargo.lock 减少 kube/k8s-openapi 等传递依赖
- 22 个测试保留在 git 历史，未来需要 K8s operator 时可从历史恢复
- README.md K8s Operator 描述更新为"已移除"

## 验证

- `grep "sz-rust-k8s-operator" Cargo.toml` 无命中
- `cargo check --workspace` 编译通过
- `cargo test -p sz-rust-sz300` 不受影响