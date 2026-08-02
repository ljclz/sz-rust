//! TDS 握手协议 — Pre-Login + Login7。
//!
//! 握手流程：
//! ```text
//! Client → Server: Pre-Login Request（OPTION/VERSION/ENCRYPTION/INSTOPT/THREADID）
//! Server → Client: Pre-Login Response（相同结构，回填服务器支持的选项）
//! Client → Server: Login7（用户名 + 混淆密码 + 数据库 + 客户端信息）
//! Server → Client: Token Stream（LOGINACK / ERROR）
//! ```
//!
//! 详见 MS-TDS 文档 "Pre-Login Packet" 和 "LOGIN7"。

use crate::auth::{deobfuscate_password, deobfuscated_to_utf16, encode_utf16_le};
use thiserror::Error;

/// TDS 协议版本：TDS 7.1（SQL Server 2000+）。
pub const TDS_VERSION_71: u32 = 0x71000001;

/// 默认协商 packet size（4KB）。
pub const DEFAULT_PACKET_SIZE: u32 = 4096;

/// Login7 OptionFlags1 默认值（0x00）。
pub const DEFAULT_OPTION_FLAGS1: u8 = 0x00;

/// Login7 OptionFlags2 默认值（0x00，不启用加密）。
pub const DEFAULT_OPTION_FLAGS2: u8 = 0x00;

/// Login7 TypeFlags 默认值（0x00）。
pub const DEFAULT_TYPE_FLAGS: u8 = 0x00;

/// Login7 OptionFlags3 默认值（0x00）。
pub const DEFAULT_OPTION_FLAGS3: u8 = 0x00;

/// Pre-Login 选项类型枚举（按任务约定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PreLoginOptionType {
    /// VERSION（0x00）
    Version = 0x00,
    /// OPTION（0x01，任务约定的非标准码位）
    Option = 0x01,
    /// ENCRYPTION（0x02，任务约定的非标准码位）
    Encryption = 0x02,
    /// INSTOPT（0x03，任务约定的非标准码位）
    InstOpt = 0x03,
    /// THREADID（0x04，任务约定的非标准码位）
    ThreadId = 0x04,
}

impl PreLoginOptionType {
    /// 从字节解析选项类型。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0x00 => PreLoginOptionType::Version,
            0x01 => PreLoginOptionType::Option,
            0x02 => PreLoginOptionType::Encryption,
            0x03 => PreLoginOptionType::InstOpt,
            0x04 => PreLoginOptionType::ThreadId,
            _ => return None,
        })
    }
}

/// ENCRYPTION 选项取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EncryptionValue {
    /// 不支持加密
    Off = 0x00,
    /// 支持加密但可选
    On = 0x01,
    /// 强制加密
    Required = 0x02,
    /// 服务器不支持
    NotSupported = 0x03,
}

impl EncryptionValue {
    /// 从字节解析。
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0x00 => EncryptionValue::Off,
            0x01 => EncryptionValue::On,
            0x02 => EncryptionValue::Required,
            0x03 => EncryptionValue::NotSupported,
            _ => return None,
        })
    }
}

/// Pre-Login 单个选项（type + offset + length + data）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreLoginOption {
    /// 选项类型
    pub option_type: PreLoginOptionType,
    /// 选项数据（选项类型决定语义）
    pub data: Vec<u8>,
}

impl PreLoginOption {
    /// 创建 VERSION 选项（6 字节：4 字节版本 + 2 字节构建号）。
    pub fn version(major: u8, minor: u8, build: u16) -> Self {
        let mut data = Vec::with_capacity(6);
        data.push(major);
        data.push(minor);
        data.push(0); // 子版本
        data.push(0);
        data.extend_from_slice(&build.to_be_bytes());
        Self {
            option_type: PreLoginOptionType::Version,
            data,
        }
    }

    /// 创建 ENCRYPTION 选项（1 字节）。
    pub fn encryption(value: EncryptionValue) -> Self {
        Self {
            option_type: PreLoginOptionType::Encryption,
            data: vec![value as u8],
        }
    }

    /// 创建 INSTOPT 选项（1 字节，SQL Server 实例选项）。
    pub fn inst_opt(value: u8) -> Self {
        Self {
            option_type: PreLoginOptionType::InstOpt,
            data: vec![value],
        }
    }

    /// 创建 THREADID 选项（4 字节，BE）。
    pub fn thread_id(tid: u32) -> Self {
        Self {
            option_type: PreLoginOptionType::ThreadId,
            data: tid.to_be_bytes().to_vec(),
        }
    }

    /// 创建 OPTION 选项（任务约定的扩展字段）。
    pub fn option(value: u8) -> Self {
        Self {
            option_type: PreLoginOptionType::Option,
            data: vec![value],
        }
    }
}

/// Pre-Login 握手包（含若干选项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreLogin {
    /// 选项列表（按选项类型递增排序）
    pub options: Vec<PreLoginOption>,
}

/// Pre-Login 编码格式：每个选项头部 5 字节（1 字节 type + 2 字节 offset BE + 2 字节 length BE），
/// 头部列表以 0xFF 终止，随后是选项数据按 offset 排列。
const OPTION_HEADER_LEN: usize = 5;
const OPTION_TERMINATOR: u8 = 0xFF;

impl PreLogin {
    /// 创建空 Pre-Login。
    pub fn new() -> Self {
        Self {
            options: Vec::new(),
        }
    }

    /// 添加选项。
    pub fn with_option(mut self, option: PreLoginOption) -> Self {
        self.options.push(option);
        self
    }

    /// 编码为字节序列（payload，不含 TDS 包头）。
    pub fn encode(&self) -> Vec<u8> {
        let header_count = self.options.len();
        let header_total = header_count * OPTION_HEADER_LEN + 1; // +1 for 0xFF
        let mut buf = Vec::with_capacity(header_total + 64);

        // 第一阶段：写入头部（type + offset + length），offset 相对数据段起点
        let mut data_offset = header_total;
        for opt in &self.options {
            buf.push(opt.option_type as u8);
            buf.extend_from_slice(&(data_offset as u16).to_be_bytes());
            buf.extend_from_slice(&(opt.data.len() as u16).to_be_bytes());
            data_offset += opt.data.len();
        }
        // 终止符
        buf.push(OPTION_TERMINATOR);

        // 第二阶段：写入数据
        for opt in &self.options {
            buf.extend_from_slice(&opt.data);
        }
        buf
    }

    /// 从 payload 字节序列解析。
    pub fn decode(payload: &[u8]) -> Result<Self, HandshakeError> {
        if payload.is_empty() {
            return Ok(Self {
                options: Vec::new(),
            });
        }

        // 第一阶段：解析头部
        let mut headers: Vec<(PreLoginOptionType, usize, usize)> = Vec::new();
        let mut pos = 0;
        loop {
            if pos >= payload.len() {
                return Err(HandshakeError::Protocol(
                    "pre-login header missing terminator".to_string(),
                ));
            }
            let byte = payload[pos];
            if byte == OPTION_TERMINATOR {
                break;
            }
            if pos + OPTION_HEADER_LEN > payload.len() {
                return Err(HandshakeError::Protocol(
                    "pre-login option header truncated".to_string(),
                ));
            }
            let option_type = PreLoginOptionType::from_byte(byte).ok_or_else(|| {
                HandshakeError::Protocol(format!("unknown pre-login option type: 0x{byte:02X}"))
            })?;
            let offset = u16::from_be_bytes([payload[pos + 1], payload[pos + 2]]) as usize;
            let length = u16::from_be_bytes([payload[pos + 3], payload[pos + 4]]) as usize;
            headers.push((option_type, offset, length));
            pos += OPTION_HEADER_LEN;
        }

        // 第二阶段：按 offset 提取数据
        let mut options = Vec::with_capacity(headers.len());
        for (option_type, offset, length) in headers {
            let abs_start = offset;
            let abs_end = offset + length;
            if abs_end > payload.len() {
                return Err(HandshakeError::Protocol(format!(
                    "pre-login option {option_type:?} data out of bounds: offset={offset}, len={length}, payload={}",
                    payload.len()
                )));
            }
            let data = payload[abs_start..abs_end].to_vec();
            options.push(PreLoginOption { option_type, data });
        }

        Ok(Self { options })
    }

    /// 查找指定类型的选项。
    pub fn find(&self, opt_type: PreLoginOptionType) -> Option<&PreLoginOption> {
        self.options.iter().find(|o| o.option_type == opt_type)
    }
}

impl Default for PreLogin {
    fn default() -> Self {
        Self::new()
    }
}

/// Login7：客户端发送给服务器的登录请求包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login7 {
    /// TDS 协议版本（默认 0x71000001）
    pub tds_version: u32,
    /// 客户端请求的 packet 大小
    pub packet_size: u32,
    /// 客户端程序版本
    pub client_prog_ver: u32,
    /// 客户端 PID
    pub client_pid: u32,
    /// 连接 ID
    pub connection_id: u32,
    /// OptionFlags1
    pub option_flags1: u8,
    /// OptionFlags2
    pub option_flags2: u8,
    /// TypeFlags
    pub type_flags: u8,
    /// OptionFlags3
    pub option_flags3: u8,
    /// 客户端时区偏移（分钟）
    pub time_zone: i32,
    /// 客户端 LCID
    pub client_lcid: u32,
    /// 主机名（UTF-16LE）
    pub host_name: String,
    /// 用户名（UTF-16LE）
    pub user_name: String,
    /// 密码（已混淆，UTF-16LE 字节）
    pub password: Vec<u8>,
    /// 应用名（UTF-16LE）
    pub app_name: String,
    /// 服务器名（UTF-16LE）
    pub server_name: String,
    /// 库名（UTF-16LE，可选）
    pub database: String,
    /// 库名（扩展，可选）
    pub library_name: String,
    /// 语言（UTF-16LE，可选）
    pub language: String,
    /// 客户端 ID（6 字节，通常为 MAC）
    pub client_id: [u8; 6],
}

impl Login7 {
    /// 创建新 Login7。
    pub fn new(host_name: impl Into<String>, user_name: impl Into<String>) -> Self {
        Self {
            tds_version: TDS_VERSION_71,
            packet_size: DEFAULT_PACKET_SIZE,
            client_prog_ver: 0,
            client_pid: std::process::id(),
            connection_id: 0,
            option_flags1: DEFAULT_OPTION_FLAGS1,
            option_flags2: DEFAULT_OPTION_FLAGS2,
            type_flags: DEFAULT_TYPE_FLAGS,
            option_flags3: DEFAULT_OPTION_FLAGS3,
            time_zone: 0,
            client_lcid: 0,
            host_name: host_name.into(),
            user_name: user_name.into(),
            password: Vec::new(),
            app_name: String::new(),
            server_name: String::new(),
            database: String::new(),
            library_name: String::new(),
            language: String::new(),
            client_id: [0u8; 6],
        }
    }

    /// 设置混淆密码（XOR 0xA5 + nibble swap，UTF-16LE 字节）。
    pub fn with_obfuscated_password(mut self, pwd: Vec<u8>) -> Self {
        self.password = pwd;
        self
    }

    /// 设置明文密码（自动混淆）。
    pub fn with_plain_password(mut self, pwd: &str) -> Self {
        let plain = encode_utf16_le(pwd);
        self.password = crate::auth::obfuscate_password(&plain);
        self
    }

    /// 设置数据库名。
    pub fn with_database(mut self, db: impl Into<String>) -> Self {
        self.database = db.into();
        self
    }

    /// 设置应用名。
    pub fn with_app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = name.into();
        self
    }

    /// 设置服务器名。
    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = name.into();
        self
    }

    /// 设置客户端 ID（6 字节）。
    pub fn with_client_id(mut self, id: [u8; 6]) -> Self {
        self.client_id = id;
        self
    }

    /// 返回混淆密码字段的引用（用于服务端 verify）。
    pub fn obfuscated_password(&self) -> &[u8] {
        &self.password
    }

    /// 返回反混淆后的明文密码（仅用于调试/验证）。
    pub fn plaintext_password(&self) -> String {
        if self.password.is_empty() {
            return String::new();
        }
        let deob = deobfuscate_password(&self.password);
        let units = deobfuscated_to_utf16(&deob);
        String::from_utf16_lossy(&units)
    }

    /// 编码为字节序列（payload）。
    ///
    /// Login7 布局：
    /// - Length(4, BE) | TDSVersion(4, BE) | PacketSize(4, BE) | ClientProgVer(4, BE)
    /// - ClientPID(4, BE) | ConnectionID(4, BE) | OptionFlags1(1) | OptionFlags2(1)
    /// - TypeFlags(1) | OptionFlags3(1) | TimeZone(4, BE) | ClientLCID(4, BE)
    /// - 变长字段偏移表：HostName/UserName/Password/AppName/ServerName/Unused/LibraryName/Language/Database
    ///   (9 个字段 × 4 字节 = 36 字节)
    /// - ClientID(6) | SSPI(0) | AtchDBFile(0) | ChangePassword(0) | cbLen(0)
    /// - 变长数据：HostName | UserName | Password | AppName | ServerName | LibraryName | Language | Database
    pub fn encode(&self) -> Vec<u8> {
        // 固定字段布局（共 36 字节）：
        // Length(4) + TDSVersion(4) + PacketSize(4) + ClientProgVer(4)
        // + ClientPID(4) + ConnectionID(4) + OptionFlags1(1) + OptionFlags2(1)
        // + TypeFlags(1) + OptionFlags3(1) + TimeZone(4) + ClientLCID(4)
        let fixed_len = 4 + 5 * 4 + 4 + 4 + 4; // = 36
        let offsets_count = 9; // 9 个变长字段（不含 ClientID/SSPI/AtchDBFile/ChangePassword/cbLen）
        let offsets_table_len = offsets_count * 4;
        let client_id_len = 6;
        let sspi_len = 0;
        let atch_db_len = 0;
        let change_pwd_len = 0;
        let cb_len_len = 0;

        let data_start = fixed_len
            + offsets_table_len
            + client_id_len
            + sspi_len
            + atch_db_len
            + change_pwd_len
            + cb_len_len;

        let host_bytes = encode_utf16_le(&self.host_name);
        let user_bytes = encode_utf16_le(&self.user_name);
        let pwd_bytes = self.password.clone();
        let app_bytes = encode_utf16_le(&self.app_name);
        let server_bytes = encode_utf16_le(&self.server_name);
        let lib_bytes = encode_utf16_le(&self.library_name);
        let lang_bytes = encode_utf16_le(&self.language);
        let db_bytes = encode_utf16_le(&self.database);

        let total_len = data_start
            + host_bytes.len()
            + user_bytes.len()
            + pwd_bytes.len()
            + app_bytes.len()
            + server_bytes.len()
            + lib_bytes.len()
            + lang_bytes.len()
            + db_bytes.len();

        let mut buf = Vec::with_capacity(total_len);
        // Length（含此字段，BE）
        buf.extend_from_slice(&(total_len as u32).to_be_bytes());
        // TDSVersion
        buf.extend_from_slice(&self.tds_version.to_be_bytes());
        // PacketSize
        buf.extend_from_slice(&self.packet_size.to_be_bytes());
        // ClientProgVer
        buf.extend_from_slice(&self.client_prog_ver.to_be_bytes());
        // ClientPID
        buf.extend_from_slice(&self.client_pid.to_be_bytes());
        // ConnectionID
        buf.extend_from_slice(&self.connection_id.to_be_bytes());
        // OptionFlags1
        buf.push(self.option_flags1);
        // OptionFlags2
        buf.push(self.option_flags2);
        // TypeFlags
        buf.push(self.type_flags);
        // OptionFlags3
        buf.push(self.option_flags3);
        // TimeZone
        buf.extend_from_slice(&self.time_zone.to_be_bytes());
        // ClientLCID
        buf.extend_from_slice(&self.client_lcid.to_be_bytes());

        // 变长字段偏移表（offset = 相对 Login7 起点的字节数，length = 字节数）
        let mut cur_offset = data_start;
        let push_field = |buf: &mut Vec<u8>, offset: &mut usize, bytes: &[u8]| {
            buf.extend_from_slice(&(*offset as u16).to_be_bytes());
            buf.extend_from_slice(&((bytes.len() as u16) >> 1).to_be_bytes()); // length 单位是 u16
            *offset += bytes.len();
        };
        push_field(&mut buf, &mut cur_offset, &host_bytes);
        push_field(&mut buf, &mut cur_offset, &user_bytes);
        push_field(&mut buf, &mut cur_offset, &pwd_bytes);
        push_field(&mut buf, &mut cur_offset, &app_bytes);
        push_field(&mut buf, &mut cur_offset, &server_bytes);
        // Unused (extension offset)
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        push_field(&mut buf, &mut cur_offset, &lib_bytes);
        push_field(&mut buf, &mut cur_offset, &lang_bytes);
        push_field(&mut buf, &mut cur_offset, &db_bytes);

        // ClientID（6 字节）
        buf.extend_from_slice(&self.client_id);

        // 变长数据
        buf.extend_from_slice(&host_bytes);
        buf.extend_from_slice(&user_bytes);
        buf.extend_from_slice(&pwd_bytes);
        buf.extend_from_slice(&app_bytes);
        buf.extend_from_slice(&server_bytes);
        buf.extend_from_slice(&lib_bytes);
        buf.extend_from_slice(&lang_bytes);
        buf.extend_from_slice(&db_bytes);

        debug_assert_eq!(buf.len(), total_len, "Login7 编码长度不匹配");
        buf
    }

    /// 从 payload 字节序列解析。
    pub fn decode(payload: &[u8]) -> Result<Self, HandshakeError> {
        if payload.len() < 36 {
            return Err(HandshakeError::Protocol(format!(
                "login7 too short: {} bytes",
                payload.len()
            )));
        }
        let total_len =
            u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
        if total_len != payload.len() {
            return Err(HandshakeError::Protocol(format!(
                "login7 length mismatch: declared {total_len}, actual {}",
                payload.len()
            )));
        }
        let tds_version = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let packet_size = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
        let client_prog_ver =
            u32::from_be_bytes([payload[12], payload[13], payload[14], payload[15]]);
        let client_pid = u32::from_be_bytes([payload[16], payload[17], payload[18], payload[19]]);
        let connection_id =
            u32::from_be_bytes([payload[20], payload[21], payload[22], payload[23]]);
        let option_flags1 = payload[24];
        let option_flags2 = payload[25];
        let type_flags = payload[26];
        let option_flags3 = payload[27];
        let time_zone = i32::from_be_bytes([payload[28], payload[29], payload[30], payload[31]]);
        let client_lcid = u32::from_be_bytes([payload[32], payload[33], payload[34], payload[35]]);

        // 9 个变长字段偏移表（每个 4 字节：offset u16 + length u16，单位 u16 字符）
        let offsets_table_start = 36;
        const FIELDS_COUNT: usize = 9;
        let mut field_offsets: [(usize, usize); FIELDS_COUNT] = [(0, 0); FIELDS_COUNT];
        for (i, item) in field_offsets.iter_mut().enumerate() {
            let pos = offsets_table_start + i * 4;
            if pos + 4 > payload.len() {
                return Err(HandshakeError::Protocol(
                    "login7 variable field offset table truncated".to_string(),
                ));
            }
            let offset = u16::from_be_bytes([payload[pos], payload[pos + 1]]) as usize;
            let char_len = u16::from_be_bytes([payload[pos + 2], payload[pos + 3]]) as usize;
            *item = (offset, char_len * 2); // 字符 → 字节
        }

        // ClientID（6 字节）
        let client_id_pos = offsets_table_start + FIELDS_COUNT * 4;
        if client_id_pos + 6 > payload.len() {
            return Err(HandshakeError::Protocol(
                "login7 client_id truncated".to_string(),
            ));
        }
        let mut client_id = [0u8; 6];
        client_id.copy_from_slice(&payload[client_id_pos..client_id_pos + 6]);

        // 提取变长字段（按 offset/length 直接读取字节，UTF-16LE 解码为 String）
        let extract =
            |offset: usize, byte_len: usize, name: &str| -> Result<Vec<u8>, HandshakeError> {
                if byte_len == 0 {
                    return Ok(Vec::new());
                }
                let end = offset + byte_len;
                if end > payload.len() {
                    return Err(HandshakeError::Protocol(format!(
                        "login7 {name} out of bounds: offset={offset}, len={byte_len}, payload={}",
                        payload.len()
                    )));
                }
                Ok(payload[offset..end].to_vec())
            };
        let decode_utf16 = |bytes: &[u8]| -> String {
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        };

        let host_bytes = extract(field_offsets[0].0, field_offsets[0].1, "host_name")?;
        let user_bytes = extract(field_offsets[1].0, field_offsets[1].1, "user_name")?;
        let pwd_bytes = extract(field_offsets[2].0, field_offsets[2].1, "password")?;
        let app_bytes = extract(field_offsets[3].0, field_offsets[3].1, "app_name")?;
        let server_bytes = extract(field_offsets[4].0, field_offsets[4].1, "server_name")?;
        // field_offsets[5] = unused extension
        let lib_bytes = extract(field_offsets[6].0, field_offsets[6].1, "library_name")?;
        let lang_bytes = extract(field_offsets[7].0, field_offsets[7].1, "language")?;
        let db_bytes = extract(field_offsets[8].0, field_offsets[8].1, "database")?;

        Ok(Self {
            tds_version,
            packet_size,
            client_prog_ver,
            client_pid,
            connection_id,
            option_flags1,
            option_flags2,
            type_flags,
            option_flags3,
            time_zone,
            client_lcid,
            host_name: decode_utf16(&host_bytes),
            user_name: decode_utf16(&user_bytes),
            password: pwd_bytes,
            app_name: decode_utf16(&app_bytes),
            server_name: decode_utf16(&server_bytes),
            database: decode_utf16(&db_bytes),
            library_name: decode_utf16(&lib_bytes),
            language: decode_utf16(&lang_bytes),
            client_id,
        })
    }
}

/// LOGINACK token：服务器回复认证成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginAck {
    /// 接口类型（1 = SQL Server）
    pub interface: u8,
    /// TDS 版本
    pub tds_version: u32,
    /// 服务器程序名
    pub prog_name: String,
    /// 服务器程序版本（4 字节）
    pub prog_version: [u8; 4],
}

impl LoginAck {
    /// 创建新 LoginAck。
    pub fn new(prog_name: impl Into<String>) -> Self {
        Self {
            interface: 1,
            tds_version: TDS_VERSION_71,
            prog_name: prog_name.into(),
            prog_version: [0x0E, 0x00, 0x00, 0x04], // 14.00.00.04
        }
    }

    /// 编码为 LOGINACK token（含 0xAD token 标识）。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32);
        // 0xAD = LOGINACK token
        buf.push(0xAD);
        buf.push(self.interface);
        buf.extend_from_slice(&self.tds_version.to_be_bytes());
        // 服务器程序名（B-VARCHAR UTF-16LE）
        let name_utf16: Vec<u16> = self.prog_name.encode_utf16().collect();
        let byte_len = name_utf16.len() * 2;
        buf.extend_from_slice(&(byte_len as u16).to_le_bytes());
        for unit in name_utf16 {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        // 服务器程序版本（4 字节）
        buf.extend_from_slice(&self.prog_version);
        buf
    }
}

/// ERROR token：服务器回复认证失败或 SQL 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorToken {
    /// 错误号
    pub number: u32,
    /// 状态
    pub state: u8,
    /// 严重级别
    pub severity: u8,
    /// 错误消息（UTF-16LE）
    pub message: String,
    /// 服务器名（可选）
    pub server_name: String,
    /// 过程名（可选）
    pub proc_name: String,
    /// 行号
    pub line_number: u32,
}

impl ErrorToken {
    /// 创建新 ErrorToken。
    pub fn new(number: u32, message: impl Into<String>) -> Self {
        Self {
            number,
            state: 1,
            severity: 16,
            message: message.into(),
            server_name: String::new(),
            proc_name: String::new(),
            line_number: 0,
        }
    }

    /// 编码为 ERROR token（含 0xAA token 标识）。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        // 0xAA = ERROR token
        buf.push(0xAA);
        buf.extend_from_slice(&self.number.to_le_bytes());
        buf.push(self.state);
        buf.push(self.severity);
        // 错误消息（US-VARCHAR，UTF-16LE）
        let msg_utf16: Vec<u16> = self.message.encode_utf16().collect();
        let msg_byte_len = msg_utf16.len() * 2;
        buf.extend_from_slice(&(msg_byte_len as u16).to_le_bytes());
        for unit in msg_utf16 {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        // 服务器名（B-VARCHAR）
        let server_utf16: Vec<u16> = self.server_name.encode_utf16().collect();
        let server_byte_len = server_utf16.len() * 2;
        buf.extend_from_slice(&(server_byte_len as u16).to_le_bytes());
        for unit in server_utf16 {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        // 过程名（B-VARCHAR）
        let proc_utf16: Vec<u16> = self.proc_name.encode_utf16().collect();
        let proc_byte_len = proc_utf16.len() * 2;
        buf.extend_from_slice(&(proc_byte_len as u16).to_le_bytes());
        for unit in proc_utf16 {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        // 行号（4 字节 LE）
        buf.extend_from_slice(&self.line_number.to_le_bytes());
        buf
    }
}

/// 握手错误。
#[derive(Debug, Error)]
pub enum HandshakeError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 协议格式错误
    #[error("protocol error: {0}")]
    Protocol(String),
    /// 包解析错误
    #[error("packet error: {0}")]
    Packet(#[from] crate::packet::PacketError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pre_login_option_type_from_byte() {
        assert_eq!(
            PreLoginOptionType::from_byte(0x00),
            Some(PreLoginOptionType::Version)
        );
        assert_eq!(
            PreLoginOptionType::from_byte(0x01),
            Some(PreLoginOptionType::Option)
        );
        assert_eq!(
            PreLoginOptionType::from_byte(0x02),
            Some(PreLoginOptionType::Encryption)
        );
        assert_eq!(
            PreLoginOptionType::from_byte(0x03),
            Some(PreLoginOptionType::InstOpt)
        );
        assert_eq!(
            PreLoginOptionType::from_byte(0x04),
            Some(PreLoginOptionType::ThreadId)
        );
        assert_eq!(PreLoginOptionType::from_byte(0xFF), None);
    }

    #[test]
    fn test_encryption_value_from_byte() {
        assert_eq!(EncryptionValue::from_byte(0x00), Some(EncryptionValue::Off));
        assert_eq!(EncryptionValue::from_byte(0x01), Some(EncryptionValue::On));
        assert_eq!(
            EncryptionValue::from_byte(0x02),
            Some(EncryptionValue::Required)
        );
        assert_eq!(
            EncryptionValue::from_byte(0x03),
            Some(EncryptionValue::NotSupported)
        );
        assert_eq!(EncryptionValue::from_byte(0x99), None);
    }

    #[test]
    fn test_pre_login_encode_decode_roundtrip() {
        let pre_login = PreLogin::new()
            .with_option(PreLoginOption::version(15, 0, 2000))
            .with_option(PreLoginOption::encryption(EncryptionValue::Off))
            .with_option(PreLoginOption::inst_opt(0))
            .with_option(PreLoginOption::thread_id(42));
        let encoded = pre_login.encode();
        let decoded = PreLogin::decode(&encoded).unwrap();
        assert_eq!(decoded, pre_login);
        assert_eq!(decoded.options.len(), 4);
    }

    #[test]
    fn test_pre_login_find_option() {
        let pre_login = PreLogin::new()
            .with_option(PreLoginOption::encryption(EncryptionValue::Required))
            .with_option(PreLoginOption::thread_id(100));
        let enc = pre_login.find(PreLoginOptionType::Encryption).unwrap();
        assert_eq!(enc.data, vec![0x02]);
        let tid = pre_login.find(PreLoginOptionType::ThreadId).unwrap();
        assert_eq!(tid.data, 100u32.to_be_bytes().to_vec());
        assert!(pre_login.find(PreLoginOptionType::Version).is_none());
    }

    #[test]
    fn test_pre_login_decode_empty() {
        let decoded = PreLogin::decode(&[]).unwrap();
        assert!(decoded.options.is_empty());
    }

    #[test]
    fn test_pre_login_decode_missing_terminator() {
        // 没有 0xFF 终止符
        let buf = [0x00u8, 0x00, 0x00, 0x00, 0x06];
        let result = PreLogin::decode(&buf);
        assert!(matches!(result, Err(HandshakeError::Protocol(_))));
    }

    #[test]
    fn test_pre_login_decode_unknown_option_type() {
        // 选项类型 0x10 非法
        let buf = vec![0x10u8, 0x00, 0x06, 0x00, 0x01, 0xFF, 0xAB];
        let result = PreLogin::decode(&buf);
        assert!(matches!(result, Err(HandshakeError::Protocol(_))));
    }

    #[test]
    fn test_login7_encode_decode_roundtrip() {
        let login = Login7::new("client_host", "sa")
            .with_plain_password("P@ssw0rd")
            .with_app_name("test_app")
            .with_server_name("localhost")
            .with_database("master")
            .with_client_id([1, 2, 3, 4, 5, 6]);
        let encoded = login.encode();
        let decoded = Login7::decode(&encoded).unwrap();
        assert_eq!(decoded.tds_version, TDS_VERSION_71);
        assert_eq!(decoded.host_name, "client_host");
        assert_eq!(decoded.user_name, "sa");
        assert_eq!(decoded.password, login.password);
        assert_eq!(decoded.app_name, "test_app");
        assert_eq!(decoded.server_name, "localhost");
        assert_eq!(decoded.database, "master");
        assert_eq!(decoded.client_id, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn test_login7_plaintext_password_roundtrip() {
        let login = Login7::new("h", "sa").with_plain_password("MySecret123");
        assert_eq!(login.plaintext_password(), "MySecret123");
    }

    #[test]
    fn test_login7_empty_password() {
        let login = Login7::new("h", "sa");
        assert_eq!(login.plaintext_password(), "");
        let encoded = login.encode();
        let decoded = Login7::decode(&encoded).unwrap();
        assert_eq!(decoded.password, Vec::<u8>::new());
        assert_eq!(decoded.plaintext_password(), "");
    }

    #[test]
    fn test_login7_decode_too_short() {
        let buf = vec![0u8; 10];
        let result = Login7::decode(&buf);
        assert!(matches!(result, Err(HandshakeError::Protocol(_))));
    }

    #[test]
    fn test_login7_decode_length_mismatch() {
        // 声明长度 100 但实际只有 50 字节
        let mut buf = 100u32.to_be_bytes().to_vec();
        buf.extend_from_slice(&vec![0u8; 46]);
        let result = Login7::decode(&buf);
        assert!(matches!(result, Err(HandshakeError::Protocol(_))));
    }

    #[test]
    fn test_login7_default_constants() {
        assert_eq!(TDS_VERSION_71, 0x71000001);
        assert_eq!(DEFAULT_PACKET_SIZE, 4096);
        assert_eq!(DEFAULT_OPTION_FLAGS1, 0x00);
        assert_eq!(DEFAULT_OPTION_FLAGS2, 0x00);
    }

    #[test]
    fn test_login_ack_encode() {
        let ack = LoginAck::new("SzRSQL");
        let encoded = ack.encode();
        assert_eq!(encoded[0], 0xAD); // LOGINACK token
        assert_eq!(encoded[1], 1); // interface
    }

    #[test]
    fn test_error_token_encode() {
        let err = ErrorToken::new(18456, "Login failed for user 'sa'");
        let encoded = err.encode();
        assert_eq!(encoded[0], 0xAA); // ERROR token
        assert_eq!(&encoded[1..5], &18456u32.to_le_bytes());
        assert_eq!(encoded[5], 1); // state
        assert_eq!(encoded[6], 16); // severity
    }

    #[test]
    fn test_error_token_with_chinese_message() {
        let err = ErrorToken::new(18456, "登录失败：用户 'sa'");
        let encoded = err.encode();
        // 解析回来检查 message
        assert_eq!(encoded[0], 0xAA);
        let msg_byte_len = u16::from_le_bytes([encoded[7], encoded[8]]) as usize;
        let msg_bytes = &encoded[9..9 + msg_byte_len];
        let units: Vec<u16> = msg_bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let msg = String::from_utf16_lossy(&units);
        assert_eq!(msg, "登录失败：用户 'sa'");
    }
}
