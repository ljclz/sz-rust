//! SzRSQL Oracle 桥接 — L2 协议级兼容。
//!
//! 本 crate 实现 Oracle Net（TNS）协议的包格式编解码与握手流程，
//! 使 Oracle 客户端驱动可直接连接 SzRSQL 服务器，达到 L2 协议级兼容。
//! 同时提供 SQL 方言转换（PL/SQL → PG SQL）与数据类型映射，
//! 为 L3 行为级兼容奠定基础。
//!
//! # L2 协议级兼容
//!
//! - **TNS 包格式**：实现 Oracle Net 的包头部编解码（[`tns_packet`] 模块）
//! - **TNS 握手**：实现 Connect Request / Accept Response 编解码与版本协商（[`tns_handshake`] 模块）
//! - **异步 IO**：基于 tokio 的异步包读写（`TnsPacketCodec`）
//!
//! # SQL 方言转换
//!
//! - **导入**：解析 Oracle 导出的 SQL 脚本（DDL + DML），转换为 SzRSQL 表数据
//! - **导出**：将 SzRSQL 表数据生成 Oracle 兼容的 SQL 脚本
//! - **SQL 转换**：将 Oracle PL/SQL 方言转换为 PG 兼容 SQL（文本级转换）
//!
//! # 模块组织
//!
//! - [`tns_packet`]：TNS 包格式编解码（包类型/标志位/异步编解码器）
//! - [`tns_handshake`]：TNS 握手协议（Connect Request / Accept Response / 版本协商）
//! - [`types`]：Oracle 类型系统与 SzRSQL `Value` 的映射
//! - [`sql_dialect`]：Oracle SQL 方言转换（PL/SQL → PG SQL）
//! - [`adapter`]：Oracle 适配器主入口（导入/导出/SQL 方言转换）
//!
//! # 设计原则
//!
//! - **零 Oracle 客户端依赖**：不依赖 libclntsh 等专有库，纯 Rust 实现 TNS 协议
//! - **文本级转换**：通过正则与字符串匹配实现 SQL 方言转换，不依赖 AST 重写
//! - **错误透明**：使用 `thiserror` 提供结构化错误
//! - **复用方言解析**：通过 `szrsql_sql::dialect::parse_with_dialect` 验证转换结果
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_oracle_bridge::OracleAdapter;
//!
//! let adapter = OracleAdapter::new();
//!
//! // 将 Oracle SQL 脚本导入为 SzRSQL 表数据
//! let script = "CREATE TABLE users (id NUMBER, name VARCHAR2(100));
//!               INSERT INTO users VALUES (1, 'Alice');";
//! let tables = adapter.import_from_oracle(script).unwrap();
//!
//! // 将 SzRSQL 表数据导出为 Oracle 兼容 SQL 脚本
//! let oracle_sql = adapter.export_to_oracle(&tables).unwrap();
//!
//! // 转换 Oracle 方言 SQL 为 PG 兼容 SQL
//! let pg_sql = adapter.convert_sql("SELECT NVL(name, 'N/A') FROM dual").unwrap();
//! ```

pub mod adapter;
pub mod server;
pub mod sql_dialect;
pub mod tns_handshake;
pub mod tns_packet;
pub mod ttc;
pub mod types;

pub use adapter::{AdapterError, OracleAdapter};
pub use server::{OracleConfig, OracleServer, OracleServerError};
pub use sql_dialect::{OracleDialect, OracleDialectError};
pub use tns_handshake::{
    AcceptResponse, ConnectRequest, HandshakeError, ACCEPT_FIXED_LEN, CONNECT_DATA_OFFSET,
    CONNECT_FIXED_LEN, DEFAULT_MAX_RECEIVE, DEFAULT_SDU, DEFAULT_TDU, TNS_VERSION_312,
    TNS_VERSION_314, TNS_VERSION_315,
};
pub use tns_packet::{
    PacketFlags, PacketType, TnsPacket, TnsPacketCodec, TnsPacketError, TNS_HEADER_LEN,
    TNS_MAX_PACKET_LEN,
};
pub use ttc::{TtcError, TtcFunction, TtcPacket, TTC_HEADER_LEN};
pub use types::{OracleType, OracleTypeError};

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn version_matches_cargo_manifest() {
        // 严格校验：version() 必须与 CARGO_PKG_VERSION 一致
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_is_valid_semver() {
        // version() 应符合 semver 格式 X.Y.Z（可含预发布段，如 1.0.0-rc.1）
        let v = version();
        let main = v.split('-').next().unwrap_or(v);
        let parts: Vec<&str> = main.split('.').collect();
        assert!(
            parts.len() >= 3,
            "version '{v}' is not semver (expected X.Y.Z, got main='{main}')"
        );
        for part in &parts[..3] {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "version part '{part}' is not numeric (in '{v}')"
            );
        }
    }

    #[test]
    fn public_api_re_exports_compile() {
        // L10 修复：原注释"占位调用以避免 unused 警告"误导审计为占位实现。
        // 实际上此测试验证公共 API 类型可被构造和引用（编译期 + 运行期检查），
        // format! 调用强制所有类型实例化并使用 Debug trait，确保 re-export 链路完整。
        let _adapter: OracleAdapter = OracleAdapter::new();
        let _dialect: OracleDialect = OracleDialect::new();
        // TNS 协议层 API
        let _packet: TnsPacket = TnsPacket::data_packet(vec![1, 2, 3]);
        let _ptype: PacketType = PacketType::Data;
        let _flags: PacketFlags = PacketFlags::new();
        let _req: ConnectRequest = ConnectRequest::new("ORCL").unwrap();
        let _resp: AcceptResponse = AcceptResponse::new(TNS_VERSION_314);
        // 强制所有类型实例化并使用 Debug trait，验证 re-export 链路完整
        let _ = format!(
            "{_adapter:?} {_dialect:?} {_packet:?} {_ptype:?} {_flags:?} {_req:?} {_resp:?} {}",
            version()
        );
    }

    #[test]
    fn tns_module_exports_accessible() {
        // 验证 TNS 模块的常量和函数可被访问
        assert_eq!(TNS_HEADER_LEN, 8);
        assert!(TNS_MAX_PACKET_LEN > 0);
        assert_eq!(TNS_VERSION_314, 314);
        assert_eq!(DEFAULT_SDU, 8192);
        assert_eq!(CONNECT_FIXED_LEN, 24);
        assert_eq!(ACCEPT_FIXED_LEN, 12);
    }
}
