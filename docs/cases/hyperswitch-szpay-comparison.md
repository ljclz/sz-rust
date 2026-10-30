# 行业观察：Hyperswitch（开源支付编排平台）与 sz-pay 对比

> **编写日期**：2026-08-13  
> **性质**：行业观察笔记（非方向变更），对照 `docs/product-technical-plan.md` 记录分析结论  
> **来源**：[CSDN 文章](https://blog.csdn.net/coderroad/article/details/149066439)（2026-08-10）+ sz-pay 源码盘点（E:\vue\test\sz-pay）

---

## 一、Hyperswitch 是什么

```
Hyperswitch = Juspay 开源的全球支付编排平台（github.com/juspay/hyperswitch）
├── 21,111 stars / 3,514 forks（2026-08）
├── 技术栈：Rust 核心 + 微服务架构（Router / Scheduler 组件）
├── 定位：统一 API 接入多个支付处理器（Adyen/Braintree/PayPal/Worldpay/
│        Fiserv/Stripe/Authorize.net/Checkout）
├── 核心卖点：
│   ├── 单一 API 集成多处理器，减少重复集成
│   ├── 智能路由：按成本、性能、业务规则优化支付成功率
│   ├── 成本优化：掌握各处理器成本，策略性路由降低支出
│   ├── 全球覆盖：广泛支付方式和国际交易
│   └── 增强安全性：PCI 合规，保护敏感支付数据
└── 商业模式：开源核心 + Juspay 企业服务
```

---

## 二、sz-pay 实际能力盘点（E:\vue\test\sz-pay）

```
sz-pay = 基于 sz-rust 的国内支付中台业务应用（server/sz-rust/）

技术栈：
├── Rust（sz-rust-core 0.6.7 + sz-orm-sqlx 2.3.0 + axum 0.8）
├── MySQL + sqlx
└── 单体应用（controllers / services / repositories / models 分层）

通道 SDK（src/sdk/，24 个国内支付通道）：
  alipay(支付宝) / wxpay(微信) / unionpay(银联) / allinpay(通联) /
  chinaums(银联商务) / dinpay(鼎付) / duolabao(朵拉宝) / easypay /
  fubei(富贝) / fuiou(富友) / haipay(海科) / heepay(汇付) / hlpay(和利通) /
  hnapay(海联) / huifu(汇付) / huolian(汇联) / jlpay(嘉联) / kuaiqian(快钱) /
  lakala(拉卡拉) / leshua(乐刷) / sandpay(杉德) / shengpay(盛付通) /
  swiftpass(威富通) / yeepay(易宝)

核心服务（src/services/，60+ 个）：
├── 路由：route_service / merchant_route_service / pay_order_channel_dispatch_service
├── 插件：payment_plugin_manager / payment_plugin_service / payment_plugin_conf_service
├── 订单：pay_order_service / pay_order_attempt_service / pay_order_lifecycle_service
├── 退款：refund_order_service / refund_creation_service / refund_dispatch_service
├── 结算：settlement_order_service / merchant_account_ledger_service
├── 商户：merchant_account_service / merchant_group_service / merchant_policy_service
├── 风控：pay_order_risk_control_service
├── 限流：rate_limit_service
├── 通知：notify_service / notify_task_service / merchant_notify_dispatcher_service
├── 渠道：payment_channel_service / channel_daily_stat_service
├── 对账：pay_callback_log_service / channel_notify_log_service
└── 协议：epay_v2_protocol_service（聚合支付协议）
```

---

## 三、对比结论：概念同构，定位不同

| 维度 | Hyperswitch | sz-pay |
|------|-------------|--------|
| 本质 | 全球支付编排平台 | 国内支付中台（业务应用） |
| 覆盖通道 | 8+ 全球处理器 | 24 个国内通道 |
| 智能路由 | 成本/性能/规则驱动 | 已有通道分发路由，可深化 |
| 架构 | 微服务（Router/Scheduler） | 单体 Rust 应用（sz-rust） |
| 订单模型 | PaymentIntent + PaymentAttempt + Connector | pay_order + pay_order_attempt + payment_plugin ✅ 同构 |
| 形态 | 独立开源产品，可商业化 | 鲜视达生态业务应用 |
| 对 sz-rust 的意义 | 验证 Rust 支付可行性 | **sz-rust 第一个复杂业务案例** |

**关键发现**：sz-pay 的 `pay_order_attempt_service`（订单尝试级追踪）+ `payment_plugin_manager`（插件化管理通道）与 Hyperswitch 的核心数据模型（PaymentIntent / PaymentAttempt / Connector）是**同一套思想**——支付领域"订单 vs 尝试 vs 通道"三级建模是标准范式，sz-pay 的建模直觉正确。

---

## 四、可借鉴的 3 点（不改变方向）

### 4.1 成本感知智能路由（P1 潜力）

```
Hyperswitch 核心卖点：按成本路由降本

sz-pay 现状：
├── route_service（路由分发）
├── merchant_route_service（商户级路由配置）
└── pay_order_channel_dispatch_service（通道分发）

升级方向：
├── 每通道费率表（手续费率、结算周期）
├── 成功率统计（通道实时成功率）
├── 路由策略：成本优先 / 成功率优先 / 平衡
└── 收益：从"固定分发"升级为"智能路由"
    （这是国内聚合支付 ping++ 模式的核心价值）
```

### 4.2 统一 API 抽象对照（P2）

```
Hyperswitch：一套 API 接所有处理器
sz-pay：payment_plugin 抽象已类似

对照方向：
├── 参考 Hyperswitch 的 REST API 设计规范
├── 补齐支付 API 的国际标准字段（金额、币种、元数据）
└── 为未来开放"支付能力"给其他 sz-rust 应用做准备
```

### 4.3 Rust 支付背书（已成立）

```
Hyperswitch = 全球最大 Rust 支付项目之一（21K stars）
sz-pay = 基于 sz-rust 的国内支付中台

共同证明：
├── Rust 完全适合支付系统（安全、性能、类型安全）
├── sz-rust 能承载支付中台级别的复杂业务（60+ 服务、24 通道）
└── sz-rust 的"业务应用案例"库 +1（sz-pay 是第一个复杂案例）
```

---

## 五、结论

```
1. sz-pay 与 Hyperswitch 概念同构，定位不同
   同构：支付编排（订单/尝试/通道三级模型 + 统一抽象 + 路由分发）
   不同：全球通用平台 vs 国内业务应用；微服务 vs sz-rust 单体

2. sz-pay 是 sz-rust 方向的最佳案例
   一个 24 通道、60+ 服务的支付中台跑在 sz-rust 上
   → 产品方案中"业务应用案例库"的有力支撑

3. 借鉴点已记录，不改变产品路线图
   成本感知路由（P1 潜力）→ 由 sz-pay 业务侧决定是否实施
   统一 API 对照（P2）→ 支付能力开放时评估
```

> **一句话**：Hyperswitch 证明了"Rust 支付编排"在全球市场的价值，sz-pay 正在国内做同样的事——而且它证明了 sz-rust 能承载真实复杂的业务系统，这正是产品方案需要的案例。
