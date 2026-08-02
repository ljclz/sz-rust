//! MySQL 命令处理 — COM_QUERY / COM_QUIT / COM_PING / COM_INIT_DB 等。
//!
//! MySQL 协议命令阶段：客户端发送命令包，服务器响应。
//! 命令包格式：1 字节命令类型 + 命令特定数据。

use thiserror::Error;

/// MySQL 命令类型枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Command {
    /// COM_SLEEP（内部，不应出现）
    Sleep = 0x00,
    /// COM_QUIT：客户端关闭连接
    Quit = 0x01,
    /// COM_INIT_DB：切换数据库
    InitDb = 0x02,
    /// COM_QUERY：执行 SQL 查询
    Query = 0x03,
    /// COM_FIELD_LIST：列出字段
    FieldList = 0x04,
    /// COM_CREATE_DB：创建数据库
    CreateDb = 0x05,
    /// COM_DROP_DB：删除数据库
    DropDb = 0x06,
    /// COM_REFRESH：刷新
    Refresh = 0x07,
    /// COM_SHUTDOWN：关闭服务器
    Shutdown = 0x08,
    /// COM_STATISTICS：服务器统计信息
    Statistics = 0x09,
    /// COM_PROCESS_INFO：进程列表
    ProcessInfo = 0x0A,
    /// COM_CONNECT（内部）
    Connect = 0x0B,
    /// COM_PROCESS_KILL：杀进程
    ProcessKill = 0x0C,
    /// COM_DEBUG：调试
    Debug = 0x0D,
    /// COM_PING：心跳
    Ping = 0x0E,
    /// COM_TIME（内部）
    Time = 0x0F,
    /// COM_DELAYED_INSERT（内部）
    DelayedInsert = 0x10,
    /// COM_CHANGE_USER：切换用户
    ChangeUser = 0x11,
    /// COM_BINLOG_DUMP：binlog 复制
    BinlogDump = 0x12,
    /// COM_TABLE_DUMP：表导出
    TableDump = 0x13,
    /// COM_CONNECT_OUT（内部）
    ConnectOut = 0x14,
    /// COM_REGISTER_SLAVE：注册从库
    RegisterSlave = 0x15,
    /// COM_STMT_PREPARE：预处理语句
    StmtPrepare = 0x16,
    /// COM_STMT_EXECUTE：执行预处理
    StmtExecute = 0x17,
    /// COM_STMT_SEND_LONG_DATA：发送长数据
    StmtSendLongData = 0x18,
    /// COM_STMT_CLOSE：关闭预处理
    StmtClose = 0x19,
    /// COM_STMT_RESET：重置预处理
    StmtReset = 0x1A,
    /// COM_SET_OPTION：设置选项
    SetOption = 0x1B,
    /// COM_STMT_FETCH：获取预处理结果
    StmtFetch = 0x1C,
    /// COM_DAEMON（内部）
    Daemon = 0x1D,
    /// COM_RESET_CONNECTION：重置连接
    ResetConnection = 0x1F,
}

impl Command {
    /// 从字节解析命令类型。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0x00 => Command::Sleep,
            0x01 => Command::Quit,
            0x02 => Command::InitDb,
            0x03 => Command::Query,
            0x04 => Command::FieldList,
            0x05 => Command::CreateDb,
            0x06 => Command::DropDb,
            0x07 => Command::Refresh,
            0x08 => Command::Shutdown,
            0x09 => Command::Statistics,
            0x0A => Command::ProcessInfo,
            0x0B => Command::Connect,
            0x0C => Command::ProcessKill,
            0x0D => Command::Debug,
            0x0E => Command::Ping,
            0x0F => Command::Time,
            0x10 => Command::DelayedInsert,
            0x11 => Command::ChangeUser,
            0x12 => Command::BinlogDump,
            0x13 => Command::TableDump,
            0x14 => Command::ConnectOut,
            0x15 => Command::RegisterSlave,
            0x16 => Command::StmtPrepare,
            0x17 => Command::StmtExecute,
            0x18 => Command::StmtSendLongData,
            0x19 => Command::StmtClose,
            0x1A => Command::StmtReset,
            0x1B => Command::SetOption,
            0x1C => Command::StmtFetch,
            0x1D => Command::Daemon,
            0x1F => Command::ResetConnection,
            _ => return None,
        })
    }

    /// 是否需要关闭连接（COM_QUIT）。
    pub fn is_quit(self) -> bool {
        matches!(self, Command::Quit)
    }

    /// 是否是 SQL 查询类命令。
    pub fn is_query(self) -> bool {
        matches!(
            self,
            Command::Query | Command::InitDb | Command::CreateDb | Command::DropDb
        )
    }
}

/// 命令解析错误。
#[derive(Debug, Error)]
pub enum CommandError {
    /// 命令包为空
    #[error("empty command packet")]
    Empty,
    /// 未知命令类型
    #[error("unknown command type: 0x{0:02X}")]
    UnknownCommand(u8),
    /// SQL 解析错误
    #[error("sql parse error: {0}")]
    SqlParse(String),
    /// 执行错误
    #[error("execution error: {0}")]
    Execution(String),
}

/// 从命令包 payload 解析命令类型和参数。
///
/// 返回 (命令类型, 剩余 payload)。
pub fn parse_command(payload: &[u8]) -> Result<(Command, &[u8]), CommandError> {
    if payload.is_empty() {
        return Err(CommandError::Empty);
    }
    let cmd = Command::from_byte(payload[0]).ok_or(CommandError::UnknownCommand(payload[0]))?;
    Ok((cmd, &payload[1..]))
}

/// COM_QUERY 命令的参数（SQL 字符串）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryCommand {
    /// SQL 语句（UTF-8 字符串）
    pub sql: String,
}

impl QueryCommand {
    /// 从 payload 解析（已去除命令字节）。
    pub fn parse(payload: &[u8]) -> Self {
        // COM_QUERY 的 payload 是 SQL 字符串（无长度前缀，直到包尾）
        let sql = String::from_utf8_lossy(payload).to_string();
        Self {
            sql: sql.trim_end_matches('\0').to_string(),
        }
    }
}

/// COM_INIT_DB 命令的参数（数据库名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitDbCommand {
    pub database: String,
}

impl InitDbCommand {
    pub fn parse(payload: &[u8]) -> Self {
        let database = String::from_utf8_lossy(payload).to_string();
        Self {
            database: database.trim_end_matches('\0').to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_from_byte_known() {
        assert_eq!(Command::from_byte(0x01), Some(Command::Quit));
        assert_eq!(Command::from_byte(0x02), Some(Command::InitDb));
        assert_eq!(Command::from_byte(0x03), Some(Command::Query));
        assert_eq!(Command::from_byte(0x0E), Some(Command::Ping));
        assert_eq!(Command::from_byte(0x16), Some(Command::StmtPrepare));
        assert_eq!(Command::from_byte(0x17), Some(Command::StmtExecute));
    }

    #[test]
    fn test_command_from_byte_unknown() {
        assert_eq!(Command::from_byte(0xFF), None);
        assert_eq!(Command::from_byte(0x99), None);
    }

    #[test]
    fn test_is_quit() {
        assert!(Command::Quit.is_quit());
        assert!(!Command::Query.is_quit());
        assert!(!Command::Ping.is_quit());
    }

    #[test]
    fn test_is_query() {
        assert!(Command::Query.is_query());
        assert!(Command::InitDb.is_query());
        assert!(Command::CreateDb.is_query());
        assert!(Command::DropDb.is_query());
        assert!(!Command::Ping.is_query());
        assert!(!Command::Quit.is_query());
    }

    #[test]
    fn test_parse_command_query() {
        let payload = [0x03, b'S', b'E', b'L', b'E', b'C', b'T', b' ', b'1'];
        let (cmd, rest) = parse_command(&payload).unwrap();
        assert_eq!(cmd, Command::Query);
        assert_eq!(rest, b"SELECT 1");
    }

    #[test]
    fn test_parse_command_quit() {
        let payload = [0x01];
        let (cmd, rest) = parse_command(&payload).unwrap();
        assert_eq!(cmd, Command::Quit);
        assert!(rest.is_empty());
    }

    #[test]
    fn test_parse_command_ping() {
        let payload = [0x0E];
        let (cmd, _) = parse_command(&payload).unwrap();
        assert_eq!(cmd, Command::Ping);
    }

    #[test]
    fn test_parse_command_empty() {
        let payload = [];
        let result = parse_command(&payload);
        assert!(matches!(result, Err(CommandError::Empty)));
    }

    #[test]
    fn test_parse_command_unknown() {
        let payload = [0x99];
        let result = parse_command(&payload);
        assert!(matches!(result, Err(CommandError::UnknownCommand(0x99))));
    }

    #[test]
    fn test_query_command_parse() {
        let payload = b"SELECT * FROM users";
        let cmd = QueryCommand::parse(payload);
        assert_eq!(cmd.sql, "SELECT * FROM users");
    }

    #[test]
    fn test_query_command_parse_with_trailing_null() {
        let mut payload = b"SELECT 1".to_vec();
        payload.push(0);
        let cmd = QueryCommand::parse(&payload);
        assert_eq!(cmd.sql, "SELECT 1");
    }

    #[test]
    fn test_query_command_parse_utf8() {
        let payload = "SELECT '中文测试'".as_bytes();
        let cmd = QueryCommand::parse(payload);
        assert_eq!(cmd.sql, "SELECT '中文测试'");
    }

    #[test]
    fn test_init_db_command_parse() {
        let payload = b"testdb";
        let cmd = InitDbCommand::parse(payload);
        assert_eq!(cmd.database, "testdb");
    }

    #[test]
    fn test_init_db_command_parse_with_null() {
        let mut payload = b"mydb".to_vec();
        payload.push(0);
        let cmd = InitDbCommand::parse(&payload);
        assert_eq!(cmd.database, "mydb");
    }

    #[test]
    fn test_command_all_variants_have_unique_byte() {
        let commands = [
            Command::Sleep,
            Command::Quit,
            Command::InitDb,
            Command::Query,
            Command::FieldList,
            Command::CreateDb,
            Command::DropDb,
            Command::Refresh,
            Command::Shutdown,
            Command::Statistics,
            Command::ProcessInfo,
            Command::Connect,
            Command::ProcessKill,
            Command::Debug,
            Command::Ping,
            Command::Time,
            Command::DelayedInsert,
            Command::ChangeUser,
            Command::BinlogDump,
            Command::TableDump,
            Command::ConnectOut,
            Command::RegisterSlave,
            Command::StmtPrepare,
            Command::StmtExecute,
            Command::StmtSendLongData,
            Command::StmtClose,
            Command::StmtReset,
            Command::SetOption,
            Command::StmtFetch,
            Command::Daemon,
            Command::ResetConnection,
        ];
        let mut bytes = std::collections::HashSet::new();
        for cmd in &commands {
            let b = *cmd as u8;
            assert!(bytes.insert(b), "duplicate byte: 0x{b:02X}");
        }
    }
}
