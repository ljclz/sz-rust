//! SzRSQL TDS 协议实现 — L2 协议级兼容。
//!
//! 本 crate 实现了 SQL Server Tabular Data Stream（TDS）协议，
//! 使标准 SQL Server 客户端驱动（sqlcmd / pymssql / JDBC for SQL Server 等）
//! 可直接连接 SzRSQL 服务并执行 SQL。
//!
//! ## 协议层级
//!
//! - **L2 协议级兼容**：客户端无需修改即可连接
//! - 支持 Pre-Login 握手（OPTION/VERSION/ENCRYPTION/INSTOPT/THREADID）
//! - 支持 Login7 认证（TDSVersion 0x71000001）
//! - 支持 SQLBatch（0x01）命令处理
//! - 支持结果集编码（ColumnMetaData 0x81 + Row 0xD1 + Done 0xFD）
//!
//! ## 模块组织
//!
//! - [`packet`]：协议帧编解码（4 字节头部：type + status + 2 字节长度，大端序）
//! - [`handshake`]：握手协议（Pre-Login → Login7）
//! - [`auth`]：SQL Server 认证（NTLM / 明文）
//! - [`command`]：命令处理（SQLBatch / RPC / Logout / Attention）
//! - [`result_set`]：结果集编码（ColumnMetaData + Row + Done）
//! - [`types`]：TDS 类型映射（BIT/INT/NVARCHAR/VARCHAR 等）
//! - [`server`]：TCP 服务器主入口

pub mod auth;
pub mod command;
pub mod handshake;
pub mod packet;
pub mod result_set;
pub mod server;
pub mod types;

pub use auth::{AuthError, AuthMode, AuthSession};
pub use command::{Command, CommandError, RpcCommand, RpcParam};
pub use handshake::{HandshakeError, Login7, PreLogin, PreLoginOption, PreLoginOptionType};
pub use packet::{PacketCodec, PacketError, TdsPacket, TdsPacketStatus, TdsPacketType,
    HEADER_LEN, MAX_PACKET_LEN};
pub use result_set::{
    encode_envchange, ColumnMetaData, DoneStatus, EnvChangeType, ResultSetEncoder, TdsRow,
};
pub use server::{TdsConfig, TdsServer, TdsServerError};
pub use types::TdsType;
