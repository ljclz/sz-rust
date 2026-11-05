# ADR 0051：数据层 FK/CHECK 补齐（Phase 1 设计方案）

> **状态：已实施（Phase 2 全部完成，2026-11-05）**
>
> 实施记录：
> - 阶段1 CHECK：migration 008~010（26 个 CHECK）
> - 阶段2 NULLABLE：migration 013~019（8 列改 NULL + 模型适配）
> - 阶段3 FK：migration 020~021（14 个 FK）
> - 阶段4 错误处理+测试：db_error.rs + constraint_violation_test.rs（7 场景）
> - 验证：lib 130 passed + 集成 7 passed + 门禁 exit=0

- **状态**：已实施（Phase 2 全部完成，2026-11-05）
- **日期**：2026-11-05
- **范围**：`packages/sz-rust-sz300` 数据层（MySQL 主 schema）
- **关联**：复评报告 `docs/audit/2026-11-04-系统架构师复评报告.md`（数据层 0 FK / 0 CHECK 风险）
- **约束**：本 ADR 仅产出设计，不修改任何代码、不新增 migration 文件、不触碰 sz-orm 源码

---

## 1. 决策背景

### 1.1 现状与风险

复评报告指出 sz-rust-sz300 数据层缺乏外键与检查约束，是从「可演示」到「可承载生产数据」的分水岭。
经 Phase 1 只读核查，实际现状如下（与复评报告略有出入，以代码为准）：

| 约束类型 | 复评报告声称 | 实际核查 | 证据 |
|---------|------------|---------|------|
| FOREIGN KEY | 0 | **0** | `migrations/001_init.sql:5-183` 全部表无 `FOREIGN KEY` 子句 |
| CHECK | 0 | **2** | `migrations/002_order_status_check.sql:26-27`（`chk_order_status`）；`migrations/add_sz_dtx_log_manual_intervention.sql:15-26`（`chk_dtx_state_v19`） |

> 说明：复评报告统计时点可能仅看 `001_init.sql`。实际 002 与 v1.9.0 补丁已加 2 个 CHECK，但 FK 仍为 0。
> 本设计以实际核查为准：**FK 0 个、CHECK 2 个**。

### 1.2 风险

1. **孤儿数据**：无 FK 时，删除商户/订单后子表（good/order/order_item/settlement/merchant_user）可能残留指向已删父行的引用，应用层若未级联清理则产生孤儿。
2. **非法枚举**：除 `order.status` 与 `sz_dtx_log.state` 外，其余 `status`/`role`/`pay_method` 等枚举列无 CHECK，DB 不拒绝越界值（如 `status=99`），依赖应用层校验，一旦绕过即污染数据。
3. **负数金额**：`good.price`、`order_item.price_fen` 等金额列在 schema 多为 `INT UNSIGNED`（DB 层防负），但模型层为 `i64`（可负，见 `models/product.rs:21`、`models/order_item.rs:19`），若经 ORM 写入负值，DB 层 `UNSIGNED` 会报错但错误信息晦涩；加 CHECK 可给出明确约束名 + 文档化语义。
4. **哨兵值 0 滥用**：多处用 `0` 表示「未绑定/未分类/非设备下单/顶级/系统级」（见 §3.2），既阻碍加 FK，又使 `0` 与合法父键混淆，语义模糊。

---

## 2. 现有 schema 摘要

### 2.1 表清单（18 张，MySQL 主 schema）

数据来源：`packages/sz-rust-sz300/migrations/001_init.sql`（13 张核心表）+ 5 个增量 migration。

| # | 表名 | 主键 | 来源 | 模型文件 |
|---|------|------|------|---------|
| 1 | `market` | `market_id` | `001_init.sql:5` | `models/market.rs` |
| 2 | `merchant` | `merchant_id` | `001_init.sql:17` | `models/merchant.rs` |
| 3 | `category` | `cat_id` | `001_init.sql:33` | `models/category.rs` |
| 4 | `good` | `good_id` | `001_init.sql:44` | `models/product.rs`（表名 `good`） |
| 5 | `ai_category` | `ai_class_id` | `001_init.sql:62` | `models/ai_category.rs` |
| 6 | `device` | `device_id` | `001_init.sql:72` | `models/device.rs` |
| 7 | `order` | `order_id` | `001_init.sql:88` | `models/order.rs` |
| 8 | `order_item` | `item_id` | `001_init.sql:109` | `models/order_item.rs` |
| 9 | `settlement` | `settle_id` | `001_init.sql:122` | `models/settlement.rs` |
| 10 | `ota_version` | `ota_id` | `001_init.sql:136` | `models/ota_version.rs` |
| 11 | `operate_log` | `log_id` | `001_init.sql:150` | `models/operate_log.rs` |
| 12 | `merchant_user` | `user_id` | `001_init.sql:164` | `models/merchant_user.rs` |
| 13 | `system_config` | `key_name` | `001_init.sql:178` | （无独立模型文件） |
| 14 | `outbox_event` | `id` | `006_outbox.sql:10` | （services/outbox.rs） |
| 15 | `sz_dtx_log` | `tx_id` | `007_sz_dtx_log.sql:13` | （distributed-tx crate） |
| 16 | `ai_classification_override` | `(product_id, at)` | `add_ai_classification_override.sql:4` | （无模型） |
| 17 | `alert_history` | `id` | `add_alert_history.sql:4` | （无模型） |
| 18 | `config_version` | `version` | `add_config_version.sql:4` | （无模型） |

> 另有 PostgreSQL 版 `001_init_pg.sql`（177 行），表结构/列名与 MySQL 版有差异（如 `good.price` vs `total_fen`、`market.merchant_id` 语义混乱）。本设计**仅针对 MySQL 主 schema**；PG 版若启用需单独评估（见 §8.4）。

### 2.2 现有约束统计

| 类型 | 数量 | 明细 |
|------|------|------|
| PRIMARY KEY | 18 | 每表 1 个（`ai_classification_override` 为复合 PK） |
| UNIQUE | 4 | `order.order_no`（`001_init.sql:90`）、`device.device_sn`（`:75`）、`merchant_user.username`（`:167`）、`(order.device_id, order.offline_seq)`（`005_order_idempotency.sql:24`） |
| CHECK | 2 | `chk_order_status`（`002:26`）、`chk_dtx_state_v19`（`add_sz_dtx_log_manual_intervention.sql:15`） |
| FOREIGN KEY | **0** | — |
| INDEX | 多 | 单列 `idx_merchant`/`idx_device`/`idx_status`/`idx_created`（`001_init`）+ 复合 `idx_merchant_created` 等（`003_indexes.sql`） |

### 2.3 现有 migration 执行顺序（生产）

```
001_init → 002_order_status_check → 003_indexes → 004_good_stock
→ 005_order_idempotency → 006_outbox → 007_sz_dtx_log
→ add_ai_classification_override → add_alert_history → add_config_version
→ add_sz_dtx_log_manual_intervention
```

证据：`tests/common/mod.rs:175-217`（`MIGRATIONS` 数组定义顺序，与生产 CLI 顺序一致）。

---

## 3. 实体关系识别（来自模型层）

### 3.1 实体关系图（业务语义）

```
market (市场)
  └─ merchant (商户)            merchant.market_id → market
       ├─ good (商品)            good.merchant_id → merchant
       │    ├─ good.cat_id → category
       │    └─ good.ai_class_id → ai_category
       ├─ device (设备)          device.merchant_id → merchant (可空，0=未绑定)
       ├─ order (订单)           order.merchant_id → merchant
       │    ├─ order.device_id → device (可空，0=非设备下单)
       │    └─ order_item (订单项)  order_item.order_id → order
       │         └─ order_item.good_id → good (可空，0=手工录入/快照)
       ├─ settlement (结算)      settlement.merchant_id → merchant
       ├─ merchant_user (用户)   merchant_user.merchant_id → merchant
       └─ operate_log (操作日志)  operate_log.merchant_id → merchant (可空，0=系统级)

category (类目，自引用)         category.parent_id → category (可空，0=顶级)
ai_category (AI 分类)           ai_category.cat_id → category (可空，0=无关联)
```

### 3.2 哨兵值 0 的列（加 FK 障碍）

以下列以 `0` 表示「无引用」，直接加 FK 会因 `0` 不存在于父表而失败：

| 表.列 | 哨兵语义 | 证据 |
|-------|---------|------|
| `device.merchant_id` | `0`=未绑定 | `001_init.sql:74` `DEFAULT 0`；`models/device.rs:13` 注释「0 表示未绑定」 |
| `order.device_id` | `0`=非设备下单 | `001_init.sql:92` `DEFAULT 0` |
| `order_item.good_id` | `0`=手工录入 | `001_init.sql:112` `DEFAULT 0` |
| `good.cat_id` | `0`=未分类 | `001_init.sql:47` `DEFAULT 0` |
| `good.ai_class_id` | `0`=无 AI 分类 | `001_init.sql:52` `DEFAULT 0` |
| `category.parent_id` | `0`=顶级 | `001_init.sql:36` `DEFAULT 0`；`models/category.rs:15` 注释「0 表示顶级」 |
| `ai_category.cat_id` | `0`=无关联 | `001_init.sql:65` `DEFAULT 0` |
| `operate_log.merchant_id` | `0`=系统级 | `001_init.sql:152` `DEFAULT 0` |

### 3.3 NOT NULL 可直接加 FK 的列

| 表.列 | 证据 |
|-------|------|
| `merchant.market_id` | `001_init.sql:19` `NOT NULL` |
| `good.merchant_id` | `001_init.sql:46` `NOT NULL` |
| `order.merchant_id` | `001_init.sql:91` `NOT NULL` |
| `order_item.order_id` | `001_init.sql:111` `NOT NULL` |
| `settlement.merchant_id` | `001_init.sql:124` `NOT NULL` |
| `merchant_user.merchant_id` | `001_init.sql:166` `NOT NULL` |

---

## 4. FK 补齐清单

### 4.1 设计原则

1. **哨兵值 0 处理**：对 §3.2 的列，采用「改 NULLABLE + 存量 0→NULL + ON DELETE SET NULL」方案（方案 A）。理由：保留哨兵值不加 FK（方案 B）违背本任务目标；引入 id=0 哑父行（方案 C）污染主键空间且治标不治本。
2. **级联策略**：
   - `ON DELETE RESTRICT`：用于核心父实体（merchant），防止误物理删除导致级联丢失。配合现有「软删除」（`merchant_service.rs:251` `UPDATE status=0`），RESTRICT 不影响软删，仅拦截物理 DELETE。
   - `ON DELETE CASCADE`：用于强从属子表（order_item→order、merchant_user→merchant），父删子必删。
   - `ON DELETE SET NULL`：用于可空弱引用（device.merchant_id 等），父删则子表引用置 NULL，保留子行。
3. **先父后子**：migration 先对父表加被引用的 UNIQUE/PK（已有），再对子表加 FK。
4. **命名**：`fk_<子表>_<列>`，便于排查。

### 4.2 FK 清单（共 14 个）

#### A. 可直接加（NOT NULL，无哨兵）— 6 个

| # | 子表.列 | → 父表.列 | 级联策略 | 理由 |
|---|--------|----------|---------|------|
| 1 | `merchant.market_id` | `market.market_id` | `ON DELETE RESTRICT` | 商户必须属于市场；删市场前应先迁走/禁用商户，RESTRICT 强制人工处理 |
| 2 | `good.merchant_id` | `merchant.merchant_id` | `ON DELETE RESTRICT` | 商品归属商户；商户软删（status=0）不触发，物理删商户前应先处理商品 |
| 3 | `order.merchant_id` | `merchant.merchant_id` | `ON DELETE RESTRICT` | 订单归属商户；订单是审计/结算依据，禁止因删商户而丢订单 |
| 4 | `order_item.order_id` | `order.order_id` | `ON DELETE CASCADE` | 订单项强从属于订单；订单物理删则项无意义。现有 saga 补偿已手动先删 item（`saga_order.rs:269`），CASCADE 为双保险 |
| 5 | `settlement.merchant_id` | `merchant.merchant_id` | `ON DELETE RESTRICT` | 结算记录是财务凭证，禁止级联删 |
| 6 | `merchant_user.merchant_id` | `merchant.merchant_id` | `ON DELETE CASCADE` | 用户强从属于商户；商户物理删则用户失效 |

#### B. 需先改 NULLABLE + 存量 0→NULL 再加 — 8 个

| # | 子表.列 | → 父表.列 | 级联策略 | 哨兵语义 | 改造步骤 |
|---|--------|----------|---------|---------|---------|
| 7 | `device.merchant_id` | `merchant.merchant_id` | `ON DELETE SET NULL` | 0=未绑定 | `MODIFY ... NULL DEFAULT NULL` + `UPDATE SET merchant_id=NULL WHERE merchant_id=0` + 加 FK |
| 8 | `order.device_id` | `device.device_id` | `ON DELETE SET NULL` | 0=非设备下单 | 同上模式 |
| 9 | `order_item.good_id` | `good.good_id` | `ON DELETE SET NULL` | 0=手工录入 | 保留 `good_name` 快照，good 删则 good_id 置 NULL 但快照名保留 |
| 10 | `good.cat_id` | `category.cat_id` | `ON DELETE SET NULL` | 0=未分类 | 同上模式 |
| 11 | `good.ai_class_id` | `ai_category.ai_class_id` | `ON DELETE SET NULL` | 0=无 AI 分类 | 同上模式 |
| 12 | `category.parent_id` | `category.cat_id` | `ON DELETE RESTRICT` | 0=顶级 | 自引用；RESTRICT 防止删有子类的类目。改 NULL 后顶级类目 parent_id=NULL |
| 13 | `ai_category.cat_id` | `category.cat_id` | `ON DELETE SET NULL` | 0=无关联 | 同上模式 |
| 14 | `operate_log.merchant_id` | `merchant.merchant_id` | `ON DELETE SET NULL` | 0=系统级 | 系统级操作日志保留，merchant_id=NULL |

> **注意**：`category.parent_id` 自引用 FK 加 `ON DELETE RESTRICT`，需确保无循环引用（类目树无环）。存量数据需校验 `parent_id` 不形成环（Phase 2 实施前用递归 CTE 校验）。

### 4.3 不加 FK 的关系（仅记录）

| 关系 | 原因 |
|------|------|
| `ai_classification_override.product_id` → `good.good_id` | `ai_classification_override` 表用 `product_id` 命名但 `good` 表主键是 `good_id`（`add_ai_classification_override.sql:5`），命名歧义；且该表无模型层引用，建议 Phase 2 先澄清命名再加 FK |
| `outbox_event.aggregate_id` → 各聚合表 | `aggregate_id` 为 `VARCHAR(64)` 多态引用（`006_outbox.sql:13`），非单表 FK，属多态外键，DB 层无法表达，靠应用层校验 |
| `alert_history.rule_id` → 规则表 | 无规则表实体（规则在配置中），不加 |
| `config_version.updated_by` → 用户表 | `updated_by` 为 `VARCHAR(64)` 操作者标识，可能是系统账号非 FK |

---

## 5. CHECK 补齐清单

### 5.1 设计原则

1. **DB 层 + 应用层双保险**：即使 schema 已 `INT UNSIGNED`（DB 防负），仍加 `CHECK (col >= 0)` 以明确语义、统一错误来源。
2. **枚举列必加 CHECK**：所有 `status`/`role`/`pay_method`/`forced` 等枚举列加 `IN (...)` 约束。
3. **语义冲突先对齐**：发现 2 处 schema 注释与模型注释冲突，**必须在加 CHECK 前先统一语义 + 存量数据迁移**（见 §5.3）。
4. **命名**：`chk_<表>_<列>`，已有 `chk_order_status`/`chk_dtx_state_v19` 保持不变。

### 5.2 CHECK 清单（共 26 个，含已有 2 个）

#### A. 已存在（2 个，保持不变）

| # | 表 | 约束 | 证据 |
|---|---|------|------|
| 1 | `order` | `chk_order_status CHECK (status IN (0,1,2,3))` | `002_order_status_check.sql:26` |
| 2 | `sz_dtx_log` | `chk_dtx_state_v19 CHECK (state IN ('init','running','committed','aborted','compensated','compensating','manual_intervention','timeout'))` | `add_sz_dtx_log_manual_intervention.sql:15` |

#### B. 金额非负（8 个）

| # | 表 | 约束 | 理由 |
|---|---|------|------|
| 3 | `good` | `CHECK (price >= 0)` | 单价非负；schema `INT UNSIGNED`（`:50`）已防负，CHECK 双保险 |
| 4 | `good` | `CHECK (stock >= 0)` | 库存非负（`004_good_stock.sql:15` 新增列） |
| 5 | `order` | `CHECK (total_fen >= 0)` | 订单总额非负 |
| 6 | `order` | `CHECK (total_weight_g >= 0)` | 总重量非负 |
| 7 | `order` | `CHECK (item_count >= 0)` | 商品种数非负 |
| 8 | `order_item` | `CHECK (price_fen >= 0)` | 单价快照非负 |
| 9 | `order_item` | `CHECK (weight_g >= 0)` | 重量非负 |
| 10 | `order_item` | `CHECK (total_fen >= 0)` | 小计非负 |
| 11 | `settlement` | `CHECK (total_fen >= 0)` | 结算金额非负 |
| 12 | `settlement` | `CHECK (fee_fen >= 0)` | 手续费非负 |
| 13 | `settlement` | `CHECK (order_count >= 0)` | 订单数非负 |

#### C. 数量正数（1 个）

| # | 表 | 约束 | 理由 |
|---|---|------|------|
| 14 | `order_item` | `CHECK (quantity > 0)` | 购买数量必须 >0；`quantity=0` 的订单项无业务意义。注意：schema `DEFAULT 1`（`001_init.sql:117`），存量需校验无 `quantity=0` 行 |

#### D. 状态枚举（13 个）

| # | 表 | 约束 | 语义 | 证据 |
|---|---|------|------|------|
| 15 | `market` | `CHECK (status IN (0,1))` | 0=禁用 1=启用 | `models/market.rs:21` |
| 16 | `merchant` | `CHECK (status IN (0,1))` | 0=禁用 1=启用 | `models/merchant.rs:23` |
| 17 | `good` | `CHECK (status IN (0,1))` | 0=下架 1=上架 | `models/product.rs:29` |
| 18 | `device` | `CHECK (status IN (0,1,2,3))` | 0=离线 1=在线 2/3=设备上报扩展状态（故障/告警） | `models/device.rs:21`；2026-11-05 修复：原 `IN (0,1)` 与 MQTT 设备状态上报业务冲突（`tests/mqtt_dispatch_test.rs:199`、`tests/e2e_device_test.rs:355`、`tests/e2e_mqtt_test.rs:235`、`tests/service_coverage_test.rs:331/408`），放宽至 `IN (0,1,2,3)` |
| 19 | `category` | `CHECK (status IN (0,1))` | — | `001_init.sql:38` |
| 20 | `ai_category` | `CHECK (status IN (0,1))` | — | `001_init.sql:67` |
| 21 | `settlement` | `CHECK (status IN (0,1,2))` | 0=待结算 1=已结算 2=已失败 | `models/settlement.rs:23` |
| 22 | `ota_version` | `CHECK (forced IN (0,1))` | 0=否 1=是 | `models/ota_version.rs:25` |
| 23 | `ota_version` | `CHECK (status IN (0,1))` | 0=禁用 1=启用 | `models/ota_version.rs:27` |
| 24 | `merchant_user` | `CHECK (status IN (0,1))` | — | `001_init.sql:171` |
| 25 | `merchant_user` | `CHECK (role IN (0,1))` | **见 §5.3 语义冲突** | — |
| 26 | `order` | `CHECK (pay_method IN (0,1,2,3))` | **见 §5.3 语义冲突** | — |
| 27 | `outbox_event` | `CHECK (status IN (0,1,2))` | 0=待投递 1=已投递 2=投递失败 | `006_outbox.sql:16` |

#### E. 其他（2 个）

| # | 表 | 约束 | 理由 |
|---|---|------|------|
| 28 | `outbox_event` | `CHECK (attempts >= 0)` | 投递次数非负 |
| 29 | `order_item` | `CHECK (order_id > 0)` | 订单项必须归属有效订单（配合 FK） |

### 5.3 语义冲突（必须在加 CHECK 前先对齐）

| 列 | schema 注释 | 模型注释 | 冲突处理 |
|----|-----------|---------|---------|
| `order.pay_method` | `001_init.sql:97`「0扫码 1现金 2其他」 | `models/order.rs:27`「0=未支付 1=微信 2=支付宝 3=现金」 | **以模型为准**（4 值枚举）。Phase 2 需先 `UPDATE` 存量数据对齐到 4 值词表，再加 `CHECK (pay_method IN (0,1,2,3))`。同步修正 schema 注释 |
| `merchant_user.role` | `001_init.sql:170`「1管理员 2操作员」 | `models/merchant_user.rs:22`「0=普通用户 1=管理员」 | **以模型为准**（0/1 二值）。Phase 2 需先 `UPDATE merchant_user SET role=0 WHERE role=2`（操作员→普通）或按业务重新映射，再加 `CHECK (role IN (0,1))`。同步修正 schema 注释 |

> **风险**：语义对齐涉及存量数据 `UPDATE`，若生产已有按 schema 语义写入的 `role=2` 行，简单 `role=0` 会丢失「操作员」角色信息。Phase 2 实施前需先 `SELECT role, COUNT(*) FROM merchant_user GROUP BY role` 核对分布，与业务方确认映射规则。

---

## 6. sz-orm migration runner 支持评估

### 6.1 runner 架构

| 组件 | 位置 | 作用 |
|------|------|------|
| `Migrator` / `FileMigrationResolver` | sz-orm-core（外部，commit `74d0ef4`，见 `.sz-orm-commit`），经 `sz-rust-orm-facade/src/lib.rs:96-100` 重导出 | 解析 migrations 目录、按版本排序、记录 `__migrations` 表 |
| CLI `migrate` 命令 | `packages/sz-rust-cli/src/cmd/migrate.rs` | 在线模式 `run_migrations_locked`（`:162`）逐条 `conn.execute(&sql)`（`:236`）原样执行 sql_up |
| 测试夹具 | `packages/sz-rust-sz300/tests/common/mod.rs:281-290` | `split_sql_statements` 按分号切分 → 逐条 `conn.execute` |

### 6.2 DDL 支持结论

**runner 把 .sql 文件的 `sql_up` 原样 `conn.execute`，不解析 DDL 语法树**（证据：`migrate.rs:236` `conn.execute(&sql)`；`tests/common/mod.rs:285` `conn.execute(&stmt)`）。
因此 **支持任意 DDL 语法**，包括：
- `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY ...` ✅
- `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)` ✅
- `ALTER TABLE ... MODIFY COLUMN ... NULL` ✅
- `UPDATE ... SET col = NULL WHERE col = 0` ✅

**已有实证**：`002_order_status_check.sql:26` 与 `add_sz_dtx_log_manual_intervention.sql:15` 已成功使用 `ALTER TABLE ... ADD CONSTRAINT ... CHECK (...)`，证明 runner 完整支持 ADD CHECK。ADD FOREIGN KEY 为同类 ALTER 语法，runner 同样支持（MySQL 8.0+ InnoDB 支持 FK）。

### 6.3 migration 应用方式

| 场景 | 方式 | 证据 |
|------|------|------|
| 生产 | 手动执行 `sz-rust-cli migrate`（在线模式连库真实执行） | `migrate.rs:142` `execute_migrate_online` |
| 测试 | `tests/common/mod.rs:242` `migrate_schema` 编译期内嵌 `include_str!` 顺序执行 | `tests/common/mod.rs:176-216` |
| 生产启动 | **不自动跑 migration** | `src/main.rs` 无 `run_migrations` 调用（grep 无结果） |

### 6.4 限制与注意事项

1. **MySQL 不支持事务性 DDL**：`migrate.rs:230` `supports_transactional_ddl(db_type)` 对 MySQL 返回 false → MySQL 下 DDL 失败无法回滚已执行的语句。Phase 2 每个 migration 文件应只含一个 ALTER，失败时人工修复。
2. **ADD FK 需校验存量数据**：MySQL 加 FK 时会全表扫描校验每行引用是否存在于父表，若存在孤儿行则整个 ALTER 失败。Phase 2 必须先跑数据清理 SQL（见 §8.1）。
3. **锁表风险**：`ADD FOREIGN KEY` 在 MySQL 8.0 走 `ALGORITHM=INPLACE, LOCK=NONE`（不阻塞读写），但需父表被引用列有索引/PK（已有 PK 满足）。`MODIFY COLUMN ... NULL` 可能走 `ALGORITHM=COPY`（重建表，锁），需低峰执行。
4. **方言适配**：`migrate.rs:227` `prepare_sql_for_db` 做方言转换。本设计 SQL 需同时兼容 MySQL（主）与 PostgreSQL（备），CHECK/FK 语法两者兼容，但 `MODIFY COLUMN` 需用 PG 的 `ALTER COLUMN ... DROP NOT NULL`——建议 Phase 2 拆分 MySQL/PG 两份 migration。

---

## 7. 对现有服务层代码的影响

### 7.1 INSERT 顺序（天然正确，无需改）

| 操作 | 现有顺序 | 加 FK 后 | 证据 |
|------|---------|---------|------|
| 创建订单 | 先 `INSERT order`（父）→ `INSERT order_item`（子）→ `INSERT outbox_event` | ✅ 父先子后，FK 不冲突 | `order_service.rs:373`→`:423`→`:454` |

### 7.2 DELETE 顺序（天然正确，无需改）

| 操作 | 现有顺序 | 加 FK 后 | 证据 |
|------|---------|---------|------|
| Saga 补偿删订单 | 先 `DELETE order_item`（子）→ `DELETE order`（父） | ✅ 子先父后，FK 不冲突 | `saga_order.rs:269`→`:276` |
| 删商户 | 软删除 `UPDATE merchant SET status=0` | ✅ 不触发 ON DELETE | `merchant_service.rs:251` |
| 删商品 | 软删除 `UPDATE good SET status=0` | ✅ 不触发 ON DELETE | `product_service.rs:287` |

> **结论**：现有 INSERT/DELETE 顺序天然满足 FK 约束，**无需调整服务层代码**。但需新增对约束违反错误的处理（§7.3）。

### 7.3 错误处理适配（需新增）

加 FK/CHECK 后，DB 会返回约束违反错误，服务层需识别并转为业务友好错误：

| DB 错误 | 触发场景 | 建议处理 |
|---------|---------|---------|
| `Cannot add or update a child row: a foreign key constraint fails` | INSERT/UPDATE 引用不存在的父键 | 捕获 → 返回「关联的{商户/订单/商品}不存在」 |
| `Check constraint ... is violated` | INSERT/UPDATE 不满足 CHECK | 捕获 → 返回「{字段}取值非法」 |

现有服务层错误处理多为 `map_err(|e| "操作失败".to_string())`（如 `merchant_service.rs:237`），丢失了 DB 错误细节。Phase 2 建议引入错误分类层，识别 MySQL error code（1452 FK、3819 CHECK）转业务错误。

### 7.4 哨兵值 0 → NULL 的代码影响

§4.2-B 把 8 个哨兵列改为 NULLABLE 后，模型层对应字段需从 `i64` 改为 `Option<i64>`：

| 模型字段 | 现有类型 | 改后 | 证据 |
|---------|---------|------|------|
| `Device.merchant_id` | `i64` | `Option<i64>` | `models/device.rs:13` |
| `Order.device_id` | `i64` | `Option<i64>` | `models/order.rs:17` |
| `OrderItem.good_id` | `i64` | `Option<i64>` | `models/order_item.rs:15` |
| `Product.cat_id` | `i64` | `Option<i64>` | `models/product.rs:15` |
| `Product.ai_class_id` | `i64` | `Option<i64>` | `models/product.rs:25` |
| `Category.parent_id` | `i64` | `Option<i64>` | `models/category.rs:15` |
| `AiCategory.cat_id` | `i64` | `Option<i64>` | `models/ai_category.rs:15` |
| `OperateLog.merchant_id` | `i64` | `Option<i64>` | `models/operate_log.rs:13` |

> **影响范围大**：涉及 8 个模型 + 所有读写这些字段的 service/controller。Phase 2 需配套修改 `get_column_value`/`from_value`（处理 `Value::Null`）+ 调用方。这是本设计最大的实施成本项，建议分表分批 PR。

---

## 8. 风险评估

### 8.1 孤儿数据导致 ADD FK 失败

加 FK 前 MySQL 全表校验引用完整性，存在孤儿则失败。**Phase 2 每个 FK 前必须先跑数据清理 SQL**，模板：

```sql
-- 加 fk_merchant_market 前的孤儿检查
SELECT COUNT(*) AS orphan_count
FROM merchant m LEFT JOIN market t ON m.market_id = t.market_id
WHERE t.market_id IS NULL;
-- 若 >0，先人工处理：DELETE 或 UPDATE 引用
```

8 个哨兵列改 NULL 前需校验存量 `0` 行的语义正确性（确认 `0` 确实表示「无引用」而非脏数据）。

### 8.2 锁表与性能

| 操作 | MySQL 8.0 算法 | 锁 | 风险 |
|------|--------------|-----|------|
| `ADD FOREIGN KEY` | INPLACE | NONE（不阻塞读写） | 低；但大表建索引有 IO 抖动 |
| `ADD CHECK` | INSTANT | NONE | 极低（仅元数据） |
| `MODIFY COLUMN ... NULL`（改可空） | 可能 COPY | SHARED（阻塞写） | **中高**；`order`/`order_item` 大表需低峰执行，预估停机窗口 |
| `UPDATE ... SET col=NULL WHERE col=0` | — | 行锁 | 中；大表批量 UPDATE 需分批 `LIMIT` 避免长事务 |

### 8.3 语义对齐的数据丢失风险

§5.3 两处语义冲突（`pay_method`、`role`）的存量 `UPDATE` 可能丢失角色/支付方式信息。**必须先与业务方确认映射规则**，并备份原值（如新增 `role_legacy` 列暂存）。

### 8.4 PostgreSQL schema 分叉

`001_init_pg.sql` 表结构与 MySQL 版差异大（列名、语义不同，如 `market.merchant_id` 在 PG 版语义混乱）。本设计仅覆盖 MySQL。若 PG 版启用，需单独设计 PG 版 FK/CHECK migration，不能复用。

### 8.5 自引用环风险

`category.parent_id` 自引用 FK 需确保类目树无环。Phase 2 实施前用递归 CTE 校验：

```sql
-- MySQL 8.0+ 递归 CTE 检测环
WITH RECURSIVE cycle_check AS (
  SELECT cat_id, parent_id, cat_id AS root FROM category WHERE parent_id IS NOT NULL
  UNION ALL
  SELECT c.cat_id, c.parent_id, ck.root
  FROM category c JOIN cycle_check ck ON c.cat_id = ck.parent_id
  WHERE c.cat_id <> ck.root
)
SELECT * FROM cycle_check WHERE cat_id = root;  -- 有结果则存在环
```

---

## 9. 实施步骤建议（Phase 2 分步计划）

> Phase 2 由后续任务实施，本节仅规划。每步独立 migration 文件 + 独立 PR，便于回滚。

### 阶段 0：前置准备（不改 schema）

1. 与业务方对齐 `pay_method`、`role` 语义映射规则（§5.3）。
2. 生产库跑全量孤儿数据检查 SQL（§8.1 模板 × 14 个 FK），产出清理清单。
3. 跑 `category` 环检测（§8.5）。
4. 备份生产库。

### 阶段 1：CHECK 约束（低风险，先做）

5. 新 migration `008_check_non_negative.sql`：加金额非负 CHECK（§5.2-B，11 个）。
6. 新 migration `009_check_quantity.sql`：`order_item.quantity > 0`（先校验存量无 `quantity=0`）。
7. 新 migration `010_check_status_enum.sql`：加无冲突的状态枚举 CHECK（§5.2-D 中无冲突项）。
8. 新 migration `011_align_pay_method.sql`：对齐 `pay_method` 语义 + 加 CHECK（§5.3）。
9. 新 migration `012_align_role.sql`：对齐 `role` 语义 + 加 CHECK（§5.3）。

### 阶段 2：哨兵列改 NULLABLE（中风险，逐表做）

10. 新 migration `013_nullable_device_merchant.sql`：`device.merchant_id` 改 NULL + 存量 0→NULL。
11. 新 migration `014_nullable_order_device.sql`：`order.device_id` 同上。
12. 新 migration `015_nullable_order_item_good.sql`：`order_item.good_id` 同上。
13. 新 migration `016_nullable_good_cats.sql`：`good.cat_id` + `good.ai_class_id` 同上。
14. 新 migration `017_nullable_category_parent.sql`：`category.parent_id` 同上（含环校验）。
15. 新 migration `018_nullable_ai_category_cat.sql`：`ai_category.cat_id` 同上。
16. 新 migration `019_nullable_operate_log_merchant.sql`：`operate_log.merchant_id` 同上。
17. **配套代码**：8 个模型字段 `i64` → `Option<i64>`（§7.4），分 8 个 PR。

### 阶段 3：FK 约束（高风险，最后做）

18. 新 migration `020_fk_direct.sql`：加 6 个 NOT NULL 直接 FK（§4.2-A）。
19. 新 migration `021_fk_nullable.sql`：加 8 个可空 FK（§4.2-B）。

### 阶段 4：错误处理 + 测试

20. 服务层引入 DB 错误分类（§7.3），识别 1452/3819 error code。
21. 新增集成测试：约束违反场景（插孤儿、插负数、插非法枚举）应被 DB 拒绝。
22. 更新 `tests/common/mod.rs:175` `MIGRATIONS` 数组追加新 migration。

### 阶段 5：文档同步

23. 更新 `CHANGELOG.md`、`docs/audit/` 复评报告状态、本 ADR 状态→已实施。

---

## 10. 验收标准

### 10.1 Phase 1（本设计文档）验收

- [x] 读完所有 sz300 migrations/*.sql（12 个）
- [x] 读完所有 sz300 models/*.rs（14 个）
- [x] 识别全部实体关系（§3）
- [x] 产出 FK 清单（14 个，§4）
- [x] 产出 CHECK 清单（26 个新增 + 2 个已有，§5）
- [x] 评估 sz-orm migration runner 支持（§6）
- [x] 评估对服务层影响（§7）
- [x] 风险评估（§8）
- [x] 实施步骤（§9）
- [x] 所有结论附 file:line 证据

### 10.2 Phase 2（后续实施）验收标准

1. `SHOW CREATE TABLE` 每张表包含预期 FK + CHECK 子句。
2. `SELECT COUNT(*) FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA='sz300' AND REFERENCED_TABLE_NAME IS NOT NULL` ≥ 14。
3. `SELECT COUNT(*) FROM information_schema.CHECK_CONSTRAINTS WHERE CONSTRAINT_SCHEMA='sz300'` ≥ 28。
4. 集成测试：插入孤儿行被 FK 拒绝（error code 1452）；插入负金额被 CHECK 拒绝（3819）；插入非法枚举被拒绝。
5. 现有全部测试 `cargo test -p sz-rust-sz300` 通过。
6. 8 个哨兵列在 DB 为 NULLABLE，对应模型字段为 `Option<i64>`。
7. 生产 migration 顺序执行无失败。

---

## 11. 数字汇总

| 指标 | 值 |
|------|---|
| 现有表数量 | 18 |
| 现有 FK | 0 |
| 现有 CHECK | 2 |
| 拟新增 FK | 14（6 直接 + 8 需改 NULL） |
| 拟新增 CHECK | 26 |
| 拟新增 migration 文件（Phase 2） | 14（008~021） |
| 需改模型字段类型 | 8（`i64` → `Option<i64>`） |
| 语义冲突待对齐 | 2（`pay_method`、`role`） |

---

## 12. 证据索引

| 结论 | 证据 |
|------|------|
| 0 FK | `migrations/001_init.sql:5-183` 无 FK 子句 |
| 2 CHECK 已存在 | `002_order_status_check.sql:26`；`add_sz_dtx_log_manual_intervention.sql:15` |
| 18 张表 | `001_init.sql`（13）+ `006_outbox.sql:10`+ `007_sz_dtx_log.sql:13`+ `add_ai_classification_override.sql:4`+ `add_alert_history.sql:4`+ `add_config_version.sql:4` |
| 8 个哨兵列 | 见 §3.2 表，各附 `001_init.sql` 行号 |
| runner 原样执行 DDL | `sz-rust-cli/src/cmd/migrate.rs:236`；`tests/common/mod.rs:285` |
| 生产不自动跑 migration | `src/main.rs` 无 run_migrations 调用 |
| INSERT 父先子后 | `order_service.rs:373`→`:423` |
| DELETE 子先父后 | `saga_order.rs:269`→`:276` |
| 软删除不触发 ON DELETE | `merchant_service.rs:251`；`product_service.rs:287` |
| pay_method 语义冲突 | `001_init.sql:97` vs `models/order.rs:27` |
| role 语义冲突 | `001_init.sql:170` vs `models/merchant_user.rs:22` |
| Migrator 来自 sz-orm | `sz-rust-orm-facade/src/lib.rs:96-100`；`.sz-orm-commit`=`74d0ef4` |