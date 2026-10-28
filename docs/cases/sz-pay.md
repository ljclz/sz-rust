# 生产案例：sz-pay 支付网关

> **案例类型**：核心业务系统迁移  
> **上线时间**：（待填写）  
> **团队规模**：（待填写）  
> **迁移周期**：（待填写）

---

## 一、项目概述

### 1.1 项目背景

sz-pay 是基于 sz-rust 框架开发的支付网关核心服务，负责（待填写：具体业务描述，如"处理商户支付请求、对账、清算"）。

### 1.2 技术选型

| 组件 | 版本 | 说明 |
|------|------|------|
| sz-rust-core | 0.2.1 | Web 框架核心 |
| sz-orm | 1.2.1 | ORM 全家桶 |
| Rust | 1.81+ | 编程语言 |
| 数据库 | MySQL 8.0 / PostgreSQL 15 | 主数据库 |
| 缓存 | Redis 7.x | 分布式缓存 |
| 消息队列 | （待填写） | 异步任务 |
| 部署 | Docker + K8s | 容器化部署 |

### 1.3 系统架构

```
┌─────────────────────────────────────────────────────────────┐
│                      Nginx (负载均衡)                        │
│                   SSL 终止 + 限流 + 熔断                       │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    sz-pay × N 实例                           │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐               │
│  │  支付处理  │  │  对账服务  │  │  清算服务  │               │
│  └───────────┘  └───────────┘  └───────────┘               │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐               │
│  │  通知服务  │  │  退款服务  │  │  风控服务  │               │
│  └───────────┘  └───────────┘  └───────────┘               │
└─────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
        ┌─────────┐     ┌─────────┐     ┌─────────┐
        │  MySQL  │     │ Redis   │     │   MQ    │
        │  主从   │     │  集群   │     │         │
        └─────────┘     └─────────┘     └─────────┘
```

---

## 二、迁移背景

### 2.1 原 PHP 版本痛点

| 问题 | 影响 | 严重程度 |
|------|------|---------|
| 性能瓶颈 | 高峰期 QPS 受限，响应延迟高 | ⭐⭐⭐⭐⭐ |
| 内存占用 | 每请求独立进程，内存开销大 | ⭐⭐⭐⭐ |
| 并发模型 | PHP-FPM 多进程，连接数受限 | ⭐⭐⭐⭐ |
| 类型安全 | 动态类型，运行时错误多 | ⭐⭐⭐ |
| 部署复杂度 | 多环境配置不一致 | ⭐⭐⭐ |

### 2.2 迁移目标

| 目标 | PHP 版本 | Rust 版本 | 提升 |
|------|---------|-----------|------|
| 峰值 QPS | （待填写） | （待填写） | （待填写）x |
| p99 延迟 | （待填写）ms | （待填写）ms | 降低（待填写）% |
| 单实例内存 | （待填写）MB | （待填写）MB | 降低（待填写）% |
| 部署时间 | （待填写）min | （待填写）min | 缩短（待填写）% |

---

## 三、迁移过程

### 3.1 迁移策略

采用**渐进式迁移**策略：

1. **Phase 1**：非核心接口迁移（查询类接口）
2. **Phase 2**：核心支付接口迁移（支付、退款）
3. **Phase 3**：对账清算系统迁移
4. **Phase 4**：全量切换 + PHP 版本下线

### 3.2 关键迁移点

#### 3.2.1 控制器迁移

```php
// PHP 原代码
class PayController extends BaseController {
    public function createOrder() {
        $data = $this->request->post();
        $order = OrderService::create($data);
        return $this->renderSuccess('创建成功', $order);
    }
}
```

```rust
// Rust 迁移后
struct PayController;
impl SzController for PayController {}

impl PayController {
    pub async fn create_order(&self, req: Request<Body>) -> Response {
        let data = self.post_data(req).await?;
        let order = OrderService::create(&state.db_pool, data).await?;
        self.render_success("创建成功", row_to_json(&order))
    }
}
```

#### 3.2.2 服务层迁移

```php
// PHP 原代码
class OrderService {
    public static function create($data) {
        $order = new Order();
        $order->merchant_id = $data['merchant_id'];
        $order->amount = $data['amount'];
        $order->save(); // 直接 SQL
        return $order;
    }
}
```

```rust
// Rust 迁移后
impl OrderService {
    pub async fn create(pool: &Pool, data: &Value) -> Result<Order, DbError> {
        let order = Order {
            merchant_id: data.get("merchant_id").and_then(|v| v.as_i64()).ok_or("缺少 merchant_id")?,
            amount: data.get("amount").and_then(|v| v.as_i64()).ok_or("缺少 amount")?,
            status: 0,
            created_at: chrono::Utc::now().timestamp(),
            ..Default::default()
        };
        Order::insert(pool, &order).await?;
        Ok(order)
    }
}
```

#### 3.2.3 数据库迁移

- **ORM 迁移**：从 ThinkPHP Model → sz-orm Model
- **迁移文件**：使用 `sz migrate:create` 生成 DDL
- **数据同步**：双写 + 校验 + 切换

### 3.3 遇到的问题与解决

| 问题 | 解决方案 |
|------|---------|
| （待填写） | （待填写） |
| （待填写） | （待填写） |
| （待填写） | （待填写） |

---

## 四、性能对比

### 4.1 基准测试

| 指标 | PHP 版本 | Rust 版本 | 提升 |
|------|---------|-----------|------|
| 峰值 QPS | （待填写） | （待填写） | （待填写）x |
| p50 延迟 | （待填写）ms | （待填写）ms | （待填写）% |
| p99 延迟 | （待填写）ms | （待填写）ms | （待填写）% |
| p999 延迟 | （待填写）ms | （待填写）ms | （待填写）% |

### 4.2 资源占用

| 指标 | PHP 版本 | Rust 版本 | 提升 |
|------|---------|-----------|------|
| 单实例内存 | （待填写）MB | （待填写）MB | 降低（待填写）% |
| CPU 使用率 | （待填写）% | （待填写）% | 降低（待填写）% |
| 实例数量 | （待填写） | （待填写） | 减少（待填写）% |

### 4.3 成本节约

| 项目 | 年度成本（PHP） | 年度成本（Rust） | 节约 |
|------|----------------|-----------------|------|
| 服务器 | （待填写）万 | （待填写）万 | （待填写）% |
| 运维 | （待填写）万 | （待填写）万 | （待填写）% |
| **合计** | **（待填写）万** | **（待填写）万** | **（待填写）%** |

---

## 五、稳定性

### 5.1 运行指标

| 指标 | 值 |
|------|-----|
| 上线时间 | （待填写） |
| 可用性 SLA | （待填写）% |
| 故障次数 | （待填写）次 |
| 平均故障恢复时间 | （待填写）分钟 |

### 5.2 典型故障与处理

| 时间 | 故障描述 | 原因 | 处理 | 改进 |
|------|---------|------|------|------|
| （待填写） | （待填写） | （待填写） | （待填写） | （待填写） |

---

## 六、经验总结

### 6.1 迁移收益

1. **性能提升**：（待填写）
2. **成本降低**：（待填写）
3. **稳定性提升**：（待填写）
4. **开发体验**：（待填写）

### 6.2 迁移挑战

1. **学习曲线**：Rust 学习成本较高，团队需要 2-4 周适应
2. **生态差异**：部分 PHP 库无 Rust 等价实现，需要自研
3. **编译时间**：Rust 编译时间较长，需要优化 CI/CD

### 6.3 最佳实践

1. **渐进式迁移**：先非核心后核心，降低风险
2. **双写验证**：迁移期间双写，数据校验后再切换
3. **充分测试**：单元测试 + 集成测试 + 压测
4. **监控告警**：完善监控，及时发现问题

### 6.4 团队反馈

> （待填写：团队成员对迁移的评价）

---

## 七、附录

### 7.1 技术栈对比

| 功能 | PHP 实现 | Rust 实现 |
|------|---------|-----------|
| Web 框架 | ThinkPHP 8 | sz-rust-core |
| ORM | ThinkPHP Model | sz-orm |
| 缓存 | think\facade\Cache | sz-rust-core::cache |
| 队列 | think-queue | sz-orm-queue |
| 事件 | think\Event | sz-rust-core::event |
| 验证 | think\Validate | sz-rust-core::validate |

### 7.2 关键依赖

```toml
[dependencies]
sz-rust-core = "0.2.1"
sz-orm-core = "1.2.1"
sz-orm-auth = "1.2.1"
tokio = { version = "1.40", features = ["full"] }
axum = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
chrono = { version = "0.4", features = ["serde"] }
```

### 7.3 联系方式

- 项目负责人：（待填写）
- 技术负责人：（待填写）
- GitHub：https://github.com/ljclz/sz-rust

---

*本文档由 sz-pay 团队提供，如有问题请联系项目团队。*
