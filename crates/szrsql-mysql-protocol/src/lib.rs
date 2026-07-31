//! SzRSQL MySQL Wire Protocol 实现 — L2 协议级兼容。
//!
//! 本 crate 实现了 MySQL 客户端/服务端线协议（MySQL Wire Protocol v10），
//! 使标准 MySQL 客户端驱动（mysql / pymysql / mysql-connector-java 等）
//! 可直接连接 SzRSQL 服务并执行 SQL。
//!
//! ## 协议层级
//!
//! - **L2 协议级兼容**：客户端无需修改即可连接
//! - 支持握手 v10（HandshakeV10）+ mysql_native_password 认证
//! - 支持 COM_QUERY（0x03）命令处理
//! - 支持结果集编码（Column Definition + Row Data + EOF/OK）
//! - 支持 Prepared Statement（COM_STMT_PREPARE / EXECUTE / CLOSE / RESET / SEND_LONG_DATA）
//! - 支持二进制协议行格式（Binary Protocol Row）
//!
//! ## 模块组织
//!
//! - [`packet`]：协议帧编解码（3 字节长度 + 1 字节序号）
//! - [`handshake`]：握手协议（Server greeting → Client auth → Server OK）
//! - [`auth`]：mysql_native_password 认证（SHA1 challenge-response）
//! - [`command`]：命令类型枚举与基础命令解析
//! - [`prepared_statement`]：Prepared Statement 状态机与二进制协议值编解码
//! - [`result_set`]：结果集编码
//! - [`types`]：MySQL 类型 OID 映射
//! - [`server`]：TCP 服务器主入口

pub mod auth;
pub mod command;
pub mod handshake;
pub mod mysql_metadata;
pub mod packet;
pub mod prepared_statement;
pub mod result_set;
pub mod server;
pub mod types;

pub use auth::{AuthError, AuthMode, AuthSession};
pub use command::{Command, CommandError};
pub use handshake::{HandshakeError, HandshakeV10, HandshakeResponse41};
pub use packet::{Packet, PacketCodec, PacketError};
pub use prepared_statement::{
    PreparedStatement, PreparedStatementStore, StmtId,
};
pub use result_set::{ColumnDefinition, ResultSetEncoder};
pub use server::{MysqlConfig, MysqlServer, MysqlServerError};
pub use types::MysqlType;
