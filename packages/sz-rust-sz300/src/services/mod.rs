// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 服务层模块聚合
//!
//! 2026-07-26：提取公共 `row_to_json` 函数至本模块，消除 product/merchant/order/device
//! 四个 service 中的重复定义（DRY）。
//!
//! 2026-07-30：优化 `row_to_json` 性能 — 引入 `value_to_json_ref` 引用版本，
//! 避免 `v.clone()` 整体克隆 Value。对于 Copy 类型（Bool/I32/I64 等数字）零拷贝，
//! 对于 String 类型仍需 clone（但省去了整体 Value clone 的开销）。

use std::collections::HashMap;

use sz_rust_core::orm::Value;

/// v1.9.0 AI 智能分类（spec §5.4）
#[cfg(feature = "v19-ai-classify")]
pub mod ai_classifier;
/// 认证服务模块（对齐 PHP AuthService）
pub mod auth_service;
/// DB 错误分类（2026-11-05 数据层 Phase 2 阶段4 新增 — 识别 MySQL 1452/3819 转业务友好消息）
pub mod db_error;
/// 设备服务模块（封装设备 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod device_service;
/// 文件服务模块（对齐 PHP FileService）
pub mod file_service;
/// 健康检查服务模块（封装 DB 探活逻辑，2026-07-26 新增 — 修复控制器分层违反）
pub mod health_service;
/// 商户服务模块（封装商户 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod merchant_service;
/// MQTT 监听服务模块
pub mod mqtt_listener;
/// MQTT 业务服务模块
pub mod mqtt_service;
/// 订单号生成（2026-10-28 修复秒级订单号碰撞）
pub mod order_no;
/// 订单服务模块（封装订单 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod order_service;
/// 事务性 Outbox 投递端（2026-10-28 修复订单/事件双写不一致）
pub mod outbox;
/// v1.9.0 插件管理器（spec §5.10）
///
/// 2026-11-05 决策记录（P1 N1，doc-debt DB-2026-11-05-07）：本模块保持
/// `v19-plugin-flow` feature gate（**不**收编 default——prod compose 的
/// `SZ300_FEATURES` 刻意不含该 feature，插件安装端点不出现在生产镜像，
/// 「模块不在生产构建里」是比验签更外层的一层防御）。盲区由 CI 承担：
/// `.github/workflows/ci.yml` 的 `plugin-flow-tests` job 以
/// `--features v19-plugin-flow` 显式编译并运行本模块全部测试（含 3 个
/// Ed25519 验签负例）。**修改 feature 策略时必须同步该 job**。
#[cfg(feature = "v19-plugin-flow")]
pub mod plugin_manager;
/// 商品服务模块（封装商品 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod product_service;
/// v1.9.0 Saga 分布式事务订单服务
#[cfg(feature = "v19-saga")]
pub mod saga_order;
/// v1.8.0 SSE 服务
#[cfg(feature = "v18-sse")]
pub mod sse_service;
/// v1.8.0 WebSocket 服务
#[cfg(feature = "v18-websocket")]
pub mod ws_service;

/// 将 DB 行（`HashMap<String, Value>`）转换为 JSON 对象（供控制器使用）
///
/// ## 参数
///
/// - `row`：DB 行数据（字段名 → 字段值）
///
/// ## 返回
///
/// `serde_json::Value::Object`，字段顺序保持 HashMap 迭代顺序。
///
/// ## 性能优化（2026-07-30）
///
/// 使用 `value_to_json_ref`（引用版本）替代 `value_to_json(v.clone())`，
/// 避免 `v.clone()` 整体克隆 Value 对象。对于数字类型字段（Copy 类型）零拷贝，
/// 对于 String 类型仍需 clone 字符串内容（但省去了 Value enum 包装的开销）。
///
/// ## 重复消除说明
///
/// 2026-07-26 之前，`product_service` / `merchant_service` / `order_service` /
/// `device_service` 各自定义了完全相同的 `row_to_json`，违反 DRY 原则。
/// 现统一提取至本模块，子模块通过 `super::row_to_json` 或 `crate::services::row_to_json` 引用。
pub fn row_to_json(row: &HashMap<String, Value>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in row {
        obj.insert(k.clone(), value_to_json_ref(v));
    }
    serde_json::Value::Object(obj)
}

/// 将 `&Value` 转换为 `serde_json::Value`（引用版本，避免整体 clone）
///
/// 与 `sz_orm_core::model::value_to_json(v: Value)` 的区别：
/// - 原版接收 `Value`（所有权），调用方需 `v.clone()`
/// - 本版接收 `&Value`，对 Copy 类型（数字/布尔）零拷贝，对 String 类型仅 clone 字符串
///
/// ## 性能对比
///
/// | 值类型 | 原版 clone 次数 | 本版 clone 次数 |
/// |--------|----------------|----------------|
/// | Null/Bool/I32/I64 等数字 | 1（v.clone()） | 0（Copy） |
/// | String/Decimal/Uuid/Date 等 | 1（v.clone()） | 1（s.clone()） |
/// | Bytes | 1（v.clone()） | 1（b.clone()） |
/// | Array | 1 + 递归 | 递归（无整体 clone） |
/// | Object | 1 + 递归 | 递归（无整体 clone） |
fn value_to_json_ref(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::I8(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::I16(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::I32(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::I64(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U8(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U16(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U32(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::U64(n) => serde_json::Value::Number(serde_json::Number::from(*n)),
        Value::F32(n) => serde_json::Number::from_f64(*n as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::F64(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        // String 类变体：需 clone 字符串内容（无法避免，serde_json 需要 String 所有权）
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Decimal(s) => serde_json::Value::String(s.clone()),
        Value::Uuid(s) => serde_json::Value::String(s.clone()),
        Value::Date(s) => serde_json::Value::String(s.clone()),
        Value::DateTime(s) => serde_json::Value::String(s.clone()),
        Value::Time(s) => serde_json::Value::String(s.clone()),
        Value::Json(s) => serde_json::Value::String(s.clone()),
        // Bytes：转十六进制字符串（对齐 sz-orm-core value_to_json 逻辑）
        Value::Bytes(b) => {
            const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";
            let mut s = String::with_capacity(b.len() * 2);
            for byte in b {
                s.push(HEX_LOWER[(byte >> 4) as usize] as char);
                s.push(HEX_LOWER[(byte & 0x0f) as usize] as char);
            }
            serde_json::Value::String(s)
        }
        // Array：递归转换，无需整体 clone
        Value::Array(arr) => serde_json::Value::Array(arr.iter().map(value_to_json_ref).collect()),
        // Object：递归转换，无需整体 clone
        Value::Object(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                obj.insert(k.clone(), value_to_json_ref(v));
            }
            serde_json::Value::Object(obj)
        }
        // non_exhaustive 通配符：未来新增变体回退为 Null（对齐 sz-orm-core 行为）
        _ => serde_json::Value::Null,
    }
}

/// 敏感信息脱敏（operate_log.detail 等自由文本落库前调用）
///
/// 掩码规则：中国大陆手机号（11 位数字）→ `138****5678`；
/// 18 位身份证 → `110***********1234`。
/// 2026-10-28 从 `mqtt_service` 提取到服务层公共位置，供审计中间件等复用。
pub fn redact_sensitive(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // 手机号：1[3-9] 开头共 11 位
        if i + 11 <= chars.len()
            && chars[i] == '1'
            && chars.get(i + 1).is_some_and(|c| matches!(c, '3'..='9'))
            && chars[i..i + 11].iter().all(|c| c.is_ascii_digit())
        {
            out.push_str(&chars[i..i + 3].iter().collect::<String>());
            out.push_str("****");
            out.push_str(&chars[i + 7..i + 11].iter().collect::<String>());
            i += 11;
            continue;
        }
        // 身份证：18 位（前 17 位数字 + 末位数字/X）
        if i + 18 <= chars.len()
            && chars[i..i + 17].iter().all(|c| c.is_ascii_digit())
            && matches!(chars[i + 17], '0'..='9' | 'X' | 'x')
        {
            out.push_str(&chars[i..i + 3].iter().collect::<String>());
            out.push_str("***********");
            out.push_str(&chars[i + 14..i + 18].iter().collect::<String>());
            i += 18;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;

    // ─────────────────────────────────────────────
    // redact_sensitive — 手机号 / 身份证脱敏纯逻辑
    // ─────────────────────────────────────────────

    #[test]
    fn redact_sensitive_preserves_plain_text_without_pii() {
        assert_eq!(redact_sensitive("普通文本无 PII"), "普通文本无 PII");
        assert_eq!(redact_sensitive(""), "");
        assert_eq!(redact_sensitive("订单号 ORD12345"), "订单号 ORD12345");
    }

    #[test]
    fn redact_sensitive_masks_china_mobile_11_digits() {
        let out = redact_sensitive("联系 13812345678 完成");
        assert!(out.contains("138****5678"), "脱敏后: {out}");
        assert!(!out.contains("1234"), "中间 4 位不得明文: {out}");
    }

    #[test]
    fn redact_sensitive_masks_mobile_at_string_start() {
        let out = redact_sensitive("13812345678");
        assert_eq!(out, "138****5678");
    }

    #[test]
    fn redact_sensitive_masks_multiple_mobiles_in_one_pass() {
        let out = redact_sensitive("13812345678 和 15987654321");
        assert!(out.contains("138****5678"));
        assert!(out.contains("159****4321"));
        assert!(!out.contains("1234"));
        assert!(!out.contains("8765"));
    }

    #[test]
    fn redact_sensitive_does_not_mask_non_mobile_11_digits() {
        // 第二位非 3-9，不构成手机号
        let out = redact_sensitive("121234567890");
        assert_eq!(out, "121234567890", "非手机号 11 位数字应原样保留");
    }

    #[test]
    fn redact_sensitive_masks_18_digit_id_card() {
        let out = redact_sensitive("身份证 110101199003071234");
        assert!(out.contains("110***********1234"), "脱敏后: {out}");
        assert!(!out.contains("19900307"), "出生日期段不得明文: {out}");
    }

    #[test]
    fn redact_sensitive_masks_id_card_with_trailing_x() {
        let out = redact_sensitive("11010119900307123X");
        assert!(out.contains("110***********123X"), "脱敏后: {out}");
    }

    #[test]
    fn redact_sensitive_masks_id_card_with_lowercase_x() {
        let out = redact_sensitive("11010119900307123x");
        assert!(out.contains("110***********123x"), "脱敏后: {out}");
    }

    #[test]
    fn redact_sensitive_does_not_mask_17_digit_id_card() {
        // 17 位不构成身份证（需 18 位）；同时选用不含 1[3-9] 开头 11 位子串的数字，
        // 避免 redact_sensitive 的手机号子串扫描命中（redact_sensitive 按字符流扫描，
        // 任意位置的 1[3-9]+10 位数字子串都会被脱敏，这是其设计语义）。
        let out = redact_sensitive("11010111010110101");
        assert_eq!(out, "11010111010110101", "17 位且无手机号子串应原样保留");
    }

    #[test]
    fn redact_sensitive_masks_both_mobile_and_id_card() {
        let out = redact_sensitive("手机 13812345678 身份证 110101199003071234");
        assert!(out.contains("138****5678"));
        assert!(out.contains("110***********1234"));
    }

    #[test]
    fn redact_sensitive_preserves_text_between_pii() {
        let out = redact_sensitive("用户 13812345678 下单 110101199003071234 完成");
        assert!(out.contains("用户"));
        assert!(out.contains("下单"));
        assert!(out.contains("完成"));
        assert!(out.contains("138****5678"));
        assert!(out.contains("110***********1234"));
    }

    #[test]
    fn redact_sensitive_handles_utf8_boundary_around_digits() {
        // 中文字符紧贴数字边界，确认 char 索引不越界
        let out = redact_sensitive("联系13812345678");
        assert!(out.contains("138****5678"));
        let out2 = redact_sensitive("13812345678联系");
        assert!(out2.contains("138****5678"));
    }

    // ─────────────────────────────────────────────
    // row_to_json / value_to_json_ref 纯逻辑补充
    // ─────────────────────────────────────────────

    #[test]
    fn row_to_json_preserves_string_value_exact() {
        let mut row = HashMap::new();
        row.insert("k".into(), Value::String("中文值".into()));
        let json = row_to_json(&row);
        assert_eq!(json["k"], "中文值");
    }

    #[test]
    fn row_to_json_i8_i16_u8_u16_u32_u64_all_numeric() {
        let mut row = HashMap::new();
        row.insert("a".into(), Value::I8(-1));
        row.insert("b".into(), Value::I16(-2));
        row.insert("c".into(), Value::U8(3));
        row.insert("d".into(), Value::U16(4));
        row.insert("e".into(), Value::U32(5));
        row.insert("f".into(), Value::U64(6));
        let json = row_to_json(&row);
        assert_eq!(json["a"], -1);
        assert_eq!(json["b"], -2);
        assert_eq!(json["c"], 3);
        assert_eq!(json["d"], 4);
        assert_eq!(json["e"], 5);
        assert_eq!(json["f"], 6);
    }

    #[test]
    fn row_to_json_decimal_and_json_string_variants() {
        let mut row = HashMap::new();
        row.insert("dec".into(), Value::Decimal("123.45".into()));
        row.insert("js".into(), Value::Json("{\"k\":1}".into()));
        let json = row_to_json(&row);
        assert_eq!(json["dec"], "123.45");
        assert_eq!(json["js"], "{\"k\":1}");
    }

    #[test]
    fn row_to_json_nested_array_of_objects() {
        let mut inner = HashMap::new();
        inner.insert("k".into(), Value::I64(1));
        let mut row = HashMap::new();
        row.insert(
            "list".into(),
            Value::Array(vec![Value::Object(inner.clone()), Value::Object(inner)]),
        );
        let json = row_to_json(&row);
        assert_eq!(json["list"][0]["k"], 1);
        assert_eq!(json["list"][1]["k"], 1);
    }
}
