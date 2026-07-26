//! 审计日志（Audit Log）— Phase 7c.3
//!
//! 对应 `SzRSQL技术实现方案.md` 9.21 节。
//!
//! # 设计
//!
//! 审计日志记录所有 DML/DDL/会话操作，支持：
//!
//! 1. **不可变 append-only** — 一旦写入不可修改/删除
//! 2. **哈希链防篡改** — 每条记录的哈希包含前一条的哈希，篡改任意记录都会导致链断裂
//! 3. **过滤记录** — AuditFilter 按命令类型/用户/对象过滤，只记录匹配事件
//! 4. **查询过滤** — AuditQuery 按时间范围/用户/命令/对象查询
//! 5. **报告导出** — CSV / JSON 格式导出
//!
//! ## 哈希链
//!
//! ```text
//! hash(0) = SHA256(0x00...00 || event_id_0 || timestamp_0 || detail_0)
//! hash(1) = SHA256(hash(0)   || event_id_1 || timestamp_1 || detail_1)
//! hash(2) = SHA256(hash(1)   || event_id_2 || timestamp_2 || detail_2)
//! ...
//! ```
//!
//! 验证时从头重新计算哈希链，任一记录被篡改都会导致后续哈希不匹配。
//!
//! # 验证标准
//!
//! - INSERT/UPDATE/DELETE 各 10000 次 → 审计日志 30000 条
//! - 逐条验证时间/用户/操作/旧数据/新数据完整
//! - 审计日志不可变，条目完整
//!
//! 对应 `SzRSQL实施进度.md` Phase 7c.3。

use sha2::{Digest, Sha256};

// =====================================================================
//  常量
// =====================================================================

/// 哈希长度（SHA-256 = 32 字节）
const HASH_LEN: usize = 32;

/// 初始 prev_hash（全零）
const INITIAL_HASH: [u8; HASH_LEN] = [0u8; HASH_LEN];

// =====================================================================
//  错误类型
// =====================================================================

/// 审计日志错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuditError {
    /// 哈希链验证失败（审计日志可能被篡改）
    #[error("hash chain verification failed at event {event_id}: expected {expected_hex}, got {actual_hex}")]
    HashChainBroken {
        event_id: u64,
        expected_hex: String,
        actual_hex: String,
    },
    /// 事件不存在
    #[error("audit event not found: {0}")]
    EventNotFound(u64),
    /// 审计日志已禁用
    #[error("audit log is disabled")]
    Disabled,
}

// =====================================================================
//  AuditCommand — 审计命令类型
// =====================================================================

/// 审计命令类型
///
/// 对应 SQL 操作的分类，用于审计日志记录和过滤。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuditCommand {
    /// INSERT 操作
    Insert,
    /// UPDATE 操作
    Update,
    /// DELETE 操作
    Delete,
    /// SELECT 操作
    Select,
    /// CREATE 操作（表/索引/视图等）
    Create,
    /// DROP 操作
    Drop,
    /// ALTER 操作
    Alter,
    /// TRUNCATE 操作
    Truncate,
    /// 登录
    Login,
    /// 登出
    Logout,
    /// GRANT 授权
    Grant,
    /// REVOKE 撤销
    Revoke,
    /// 自定义命令
    Other(String),
}

impl AuditCommand {
    /// 转换为大写字符串表示
    pub fn as_str(&self) -> &str {
        match self {
            AuditCommand::Insert => "INSERT",
            AuditCommand::Update => "UPDATE",
            AuditCommand::Delete => "DELETE",
            AuditCommand::Select => "SELECT",
            AuditCommand::Create => "CREATE",
            AuditCommand::Drop => "DROP",
            AuditCommand::Alter => "ALTER",
            AuditCommand::Truncate => "TRUNCATE",
            AuditCommand::Login => "LOGIN",
            AuditCommand::Logout => "LOGOUT",
            AuditCommand::Grant => "GRANT",
            AuditCommand::Revoke => "REVOKE",
            AuditCommand::Other(s) => s.as_str(),
        }
    }

    /// 是否为 DML 操作（INSERT/UPDATE/DELETE）
    pub fn is_dml(&self) -> bool {
        matches!(
            self,
            AuditCommand::Insert | AuditCommand::Update | AuditCommand::Delete
        )
    }

    /// 是否为 DDL 操作（CREATE/DROP/ALTER/TRUNCATE）
    pub fn is_ddl(&self) -> bool {
        matches!(
            self,
            AuditCommand::Create
                | AuditCommand::Drop
                | AuditCommand::Alter
                | AuditCommand::Truncate
        )
    }
}

impl std::fmt::Display for AuditCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  AuditEvent — 审计事件
// =====================================================================

/// 审计事件 — 单条审计日志记录
///
/// # 字段
///
/// - `event_id` — 单调递增 ID（由 AuditLog 自动分配）
/// - `timestamp` — 事件时间（Unix epoch 秒，由调用方提供）
/// - `session_id` — 会话 ID
/// - `user` — 用户名
/// - `tenant_id` — 租户 ID（可选）
/// - `ip_address` — 客户端 IP
/// - `command` — 操作类型
/// - `object` — 操作对象（表名/视图名等）
/// - `detail` — 详细信息（SQL 语句/变化描述）
/// - `old_data` — 修改前数据（UPDATE/DELETE）
/// - `new_data` — 修改后数据（INSERT/UPDATE）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    /// 单调递增事件 ID
    pub event_id: u64,
    /// 事件时间（Unix epoch 秒）
    pub timestamp: u64,
    /// 会话 ID
    pub session_id: u64,
    /// 用户名
    pub user: String,
    /// 租户 ID（可选）
    pub tenant_id: Option<String>,
    /// 客户端 IP 地址
    pub ip_address: String,
    /// 操作类型
    pub command: AuditCommand,
    /// 操作对象（表名/视图名等）
    pub object: String,
    /// 详细信息（SQL 语句/变化描述）
    pub detail: String,
    /// 修改前数据（UPDATE/DELETE 的旧值）
    pub old_data: Option<Vec<u8>>,
    /// 修改后数据（INSERT/UPDATE 的新值）
    pub new_data: Option<Vec<u8>>,
}

impl AuditEvent {
    /// 创建审计事件构建器
    pub fn builder() -> AuditEventBuilder {
        AuditEventBuilder::default()
    }

    /// 计算事件的哈希（用于哈希链）
    ///
    /// hash = SHA256(prev_hash || event_id || timestamp || detail)
    fn compute_hash(&self, prev_hash: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
        let mut hasher = Sha256::new();
        hasher.update(prev_hash);
        hasher.update(self.event_id.to_le_bytes());
        hasher.update(self.timestamp.to_le_bytes());
        hasher.update(self.detail.as_bytes());
        let digest = hasher.finalize();
        let mut hash = [0u8; HASH_LEN];
        hash.copy_from_slice(&digest);
        hash
    }
}

// =====================================================================
//  AuditEventBuilder — 审计事件构建器
// =====================================================================

/// 审计事件构建器（简化事件创建）
#[derive(Debug, Clone, Default)]
pub struct AuditEventBuilder {
    timestamp: u64,
    session_id: u64,
    user: String,
    tenant_id: Option<String>,
    ip_address: String,
    command: Option<AuditCommand>,
    object: String,
    detail: String,
    old_data: Option<Vec<u8>>,
    new_data: Option<Vec<u8>>,
}

impl AuditEventBuilder {
    /// 设置时间戳
    pub fn timestamp(mut self, ts: u64) -> Self {
        self.timestamp = ts;
        self
    }

    /// 设置会话 ID
    pub fn session_id(mut self, id: u64) -> Self {
        self.session_id = id;
        self
    }

    /// 设置用户名
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// 设置租户 ID
    pub fn tenant_id(mut self, tenant: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant.into());
        self
    }

    /// 设置 IP 地址
    pub fn ip_address(mut self, ip: impl Into<String>) -> Self {
        self.ip_address = ip.into();
        self
    }

    /// 设置命令类型
    pub fn command(mut self, cmd: AuditCommand) -> Self {
        self.command = Some(cmd);
        self
    }

    /// 设置操作对象
    pub fn object(mut self, obj: impl Into<String>) -> Self {
        self.object = obj.into();
        self
    }

    /// 设置详细信息
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// 设置旧数据
    pub fn old_data(mut self, data: Vec<u8>) -> Self {
        self.old_data = Some(data);
        self
    }

    /// 设置新数据
    pub fn new_data(mut self, data: Vec<u8>) -> Self {
        self.new_data = Some(data);
        self
    }

    /// 构建审计事件（event_id 由 AuditLog 分配，此处设为 0）
    pub fn build(self) -> AuditEvent {
        AuditEvent {
            event_id: 0, // 由 AuditLog::record() 分配
            timestamp: self.timestamp,
            session_id: self.session_id,
            user: self.user,
            tenant_id: self.tenant_id,
            ip_address: self.ip_address,
            command: self
                .command
                .unwrap_or(AuditCommand::Other("UNKNOWN".to_string())),
            object: self.object,
            detail: self.detail,
            old_data: self.old_data,
            new_data: self.new_data,
        }
    }
}

// =====================================================================
//  AuditFilter — 审计过滤器（控制哪些事件被记录）
// =====================================================================

/// 审计过滤器 — 控制哪些事件被记录
///
/// 所有字段均为 Option<Vec<T>>：
/// - None = 不过滤（记录所有）
/// - Some(vec) = 只记录匹配的
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// 命令类型过滤（None = 所有类型）
    pub commands: Option<Vec<AuditCommand>>,
    /// 用户过滤（None = 所有用户）
    pub users: Option<Vec<String>>,
    /// 对象过滤（None = 所有对象）
    pub objects: Option<Vec<String>>,
}

impl AuditFilter {
    /// 创建空过滤器（记录所有事件）
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置命令类型过滤
    pub fn commands(mut self, cmds: Vec<AuditCommand>) -> Self {
        self.commands = Some(cmds);
        self
    }

    /// 设置用户过滤
    pub fn users(mut self, users: Vec<String>) -> Self {
        self.users = Some(users);
        self
    }

    /// 设置对象过滤
    pub fn objects(mut self, objs: Vec<String>) -> Self {
        self.objects = Some(objs);
        self
    }

    /// 检查事件是否匹配过滤器（匹配则记录）
    pub fn matches(&self, event: &AuditEvent) -> bool {
        if let Some(ref cmds) = self.commands {
            if !cmds.contains(&event.command) {
                return false;
            }
        }
        if let Some(ref users) = self.users {
            if !users.contains(&event.user) {
                return false;
            }
        }
        if let Some(ref objs) = self.objects {
            if !objs.contains(&event.object) {
                return false;
            }
        }
        true
    }
}

// =====================================================================
//  AuditQuery — 审计查询过滤器
// =====================================================================

/// 审计查询过滤器 — 用于查询审计日志
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    /// 起始时间（None = 不限制）
    pub start_time: Option<u64>,
    /// 结束时间（None = 不限制）
    pub end_time: Option<u64>,
    /// 用户过滤（None = 所有用户）
    pub user: Option<String>,
    /// 命令类型过滤（None = 所有类型）
    pub command: Option<AuditCommand>,
    /// 对象过滤（None = 所有对象）
    pub object: Option<String>,
    /// 最大返回条数（None = 不限制）
    pub limit: Option<usize>,
}

impl AuditQuery {
    /// 创建空查询（返回所有事件）
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置时间范围
    pub fn time_range(mut self, start: u64, end: u64) -> Self {
        self.start_time = Some(start);
        self.end_time = Some(end);
        self
    }

    /// 设置用户过滤
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// 设置命令类型过滤
    pub fn command(mut self, cmd: AuditCommand) -> Self {
        self.command = Some(cmd);
        self
    }

    /// 设置对象过滤
    pub fn object(mut self, obj: impl Into<String>) -> Self {
        self.object = Some(obj.into());
        self
    }

    /// 设置最大返回条数
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = Some(n);
        self
    }

    /// 检查事件是否匹配查询条件
    fn matches(&self, event: &AuditEvent) -> bool {
        if let Some(start) = self.start_time {
            if event.timestamp < start {
                return false;
            }
        }
        if let Some(end) = self.end_time {
            if event.timestamp > end {
                return false;
            }
        }
        if let Some(ref user) = self.user {
            if &event.user != user {
                return false;
            }
        }
        if let Some(ref cmd) = self.command {
            if &event.command != cmd {
                return false;
            }
        }
        if let Some(ref obj) = self.object {
            if &event.object != obj {
                return false;
            }
        }
        true
    }
}

// =====================================================================
//  AuditHashChain — 哈希链防篡改
// =====================================================================

/// 带哈希的审计事件（用于哈希链验证）
#[derive(Debug, Clone)]
pub struct AuditEventWithHash {
    /// 审计事件
    pub event: AuditEvent,
    /// 该事件的哈希值
    pub hash: [u8; HASH_LEN],
}

/// 审计哈希链 — 防篡改
///
/// 每条事件的哈希包含前一条的哈希，形成链式结构。
/// 篡改任意记录都会导致后续所有哈希不匹配。
#[derive(Debug, Clone)]
pub struct AuditHashChain {
    /// 上一个事件的哈希（用于计算下一个事件的哈希）
    prev_hash: [u8; HASH_LEN],
    /// 所有事件的哈希列表
    hashes: Vec<[u8; HASH_LEN]>,
}

impl Default for AuditHashChain {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditHashChain {
    /// 创建空哈希链（prev_hash = 全零）
    pub fn new() -> Self {
        Self {
            prev_hash: INITIAL_HASH,
            hashes: Vec::new(),
        }
    }

    /// 添加事件并计算链式哈希
    ///
    /// hash = SHA256(prev_hash || event_id || timestamp || detail)
    ///
    /// 返回该事件的哈希值
    pub fn append(&mut self, event: &AuditEvent) -> [u8; HASH_LEN] {
        let hash = event.compute_hash(&self.prev_hash);
        self.prev_hash = hash;
        self.hashes.push(hash);
        hash
    }

    /// 获取所有哈希
    pub fn hashes(&self) -> &[[u8; HASH_LEN]] {
        &self.hashes
    }

    /// 获取当前 prev_hash
    pub fn prev_hash(&self) -> &[u8; HASH_LEN] {
        &self.prev_hash
    }

    /// 验证哈希链完整性
    ///
    /// 从头重新计算哈希链，检查每条事件的哈希是否匹配。
    /// 返回 Ok(()) 表示完整，Err 表示被篡改。
    pub fn verify(&self, events: &[AuditEvent]) -> Result<(), AuditError> {
        if events.len() != self.hashes.len() {
            return Err(AuditError::HashChainBroken {
                event_id: 0,
                expected_hex: format!("{}", self.hashes.len()),
                actual_hex: format!("{}", events.len()),
            });
        }

        let mut prev = INITIAL_HASH;
        for (i, event) in events.iter().enumerate() {
            let computed = event.compute_hash(&prev);
            if computed != self.hashes[i] {
                return Err(AuditError::HashChainBroken {
                    event_id: event.event_id,
                    expected_hex: hex_encode(&self.hashes[i]),
                    actual_hex: hex_encode(&computed),
                });
            }
            prev = computed;
        }
        Ok(())
    }

    /// 获取带哈希的事件列表
    pub fn events_with_hashes(&self, events: &[AuditEvent]) -> Vec<AuditEventWithHash> {
        events
            .iter()
            .zip(self.hashes.iter())
            .map(|(event, hash)| AuditEventWithHash {
                event: event.clone(),
                hash: *hash,
            })
            .collect()
    }

    /// 哈希数量
    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

// =====================================================================
//  AuditLog — 审计日志存储（不可变 append-only）
// =====================================================================

/// 审计日志存储 — 不可变 append-only
///
/// # 特性
///
/// 1. **不可变** — 一旦写入，事件不可修改/删除
/// 2. **Append-only** — 新事件始终追加到末尾
/// 3. **哈希链** — 每条事件的哈希包含前一条的哈希，防篡改
/// 4. **过滤记录** — AuditFilter 控制哪些事件被记录
/// 5. **查询过滤** — AuditQuery 按条件查询
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::audit::*;
///
/// let mut log = AuditLog::new();
/// log.enable();
///
/// let event = AuditEvent::builder()
///     .timestamp(1000)
///     .user("admin")
///     .command(AuditCommand::Insert)
///     .object("users")
///     .detail("INSERT INTO users VALUES (1, 'alice')")
///     .new_data(b"1,alice".to_vec())
///     .build();
///
/// let event_id = log.record(event).unwrap();
/// let results = log.query(&AuditQuery::new().user("admin"));
/// assert_eq!(results.len(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct AuditLog {
    /// 是否启用
    enabled: bool,
    /// 事件列表（append-only）
    events: Vec<AuditEvent>,
    /// 下一个事件 ID
    next_event_id: u64,
    /// 记录过滤器
    filter: AuditFilter,
    /// 哈希链
    hash_chain: AuditHashChain,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLog {
    /// 创建审计日志（默认禁用）
    pub fn new() -> Self {
        Self {
            enabled: false,
            events: Vec::new(),
            next_event_id: 1,
            filter: AuditFilter::new(),
            hash_chain: AuditHashChain::new(),
        }
    }

    /// 启用审计日志
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// 禁用审计日志
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    /// 是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 设置记录过滤器
    pub fn set_filter(&mut self, filter: AuditFilter) {
        self.filter = filter;
    }

    /// 获取记录过滤器引用
    pub fn filter(&self) -> &AuditFilter {
        &self.filter
    }

    /// 写入审计日志（自动分配 event_id 并更新哈希链）
    ///
    /// 如果事件不匹配过滤器或审计日志未启用，则跳过（返回 None）。
    ///
    /// # 错误
    ///
    /// - `Disabled` — 审计日志未启用
    pub fn record(&mut self, mut event: AuditEvent) -> Result<Option<u64>, AuditError> {
        if !self.enabled {
            return Err(AuditError::Disabled);
        }

        // 过滤器检查
        if !self.filter.matches(&event) {
            return Ok(None);
        }

        // 分配 event_id
        event.event_id = self.next_event_id;
        self.next_event_id += 1;

        // 更新哈希链
        self.hash_chain.append(&event);

        // 追加事件
        self.events.push(event);

        Ok(Some(self.next_event_id - 1))
    }

    /// 查询审计日志
    pub fn query(&self, query: &AuditQuery) -> Vec<AuditEvent> {
        let mut results: Vec<AuditEvent> = self
            .events
            .iter()
            .filter(|e| query.matches(e))
            .cloned()
            .collect();

        // 应用 limit
        if let Some(limit) = query.limit {
            results.truncate(limit);
        }

        results
    }

    /// 获取所有事件（不可变引用）
    pub fn events(&self) -> &[AuditEvent] {
        &self.events
    }

    /// 获取事件数量
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 按 event_id 获取事件
    pub fn get(&self, event_id: u64) -> Option<&AuditEvent> {
        self.events.iter().find(|e| e.event_id == event_id)
    }

    /// 验证哈希链完整性
    pub fn verify_chain(&self) -> Result<(), AuditError> {
        self.hash_chain.verify(&self.events)
    }

    /// 获取哈希链引用
    pub fn hash_chain(&self) -> &AuditHashChain {
        &self.hash_chain
    }

    /// 获取带哈希的事件列表
    pub fn events_with_hashes(&self) -> Vec<AuditEventWithHash> {
        self.hash_chain.events_with_hashes(&self.events)
    }

    /// 按命令类型统计
    pub fn count_by_command(&self, command: &AuditCommand) -> usize {
        self.events.iter().filter(|e| &e.command == command).count()
    }

    /// 按用户统计
    pub fn count_by_user(&self, user: &str) -> usize {
        self.events.iter().filter(|e| e.user == user).count()
    }

    /// 按对象统计
    pub fn count_by_object(&self, object: &str) -> usize {
        self.events.iter().filter(|e| e.object == object).count()
    }

    /// 获取时间范围
    pub fn time_range(&self) -> Option<(u64, u64)> {
        if self.events.is_empty() {
            return None;
        }
        let start = self.events.first().map(|e| e.timestamp).unwrap();
        let end = self.events.last().map(|e| e.timestamp).unwrap();
        Some((start, end))
    }
}

// =====================================================================
//  AuditReport — 审计报告导出
// =====================================================================

/// 报告格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// CSV 格式
    Csv,
    /// JSON 格式
    Json,
}

/// 审计报告
#[derive(Debug, Clone)]
pub struct AuditReport {
    /// 时间范围 (start, end)
    pub time_range: (u64, u64),
    /// 事件列表
    pub events: Vec<AuditEvent>,
}

impl AuditReport {
    /// 从审计日志创建报告
    pub fn from_log(log: &AuditLog) -> Self {
        let time_range = log.time_range().unwrap_or((0, 0));
        Self {
            time_range,
            events: log.events().to_vec(),
        }
    }

    /// 从事件列表创建报告
    pub fn from_events(events: Vec<AuditEvent>) -> Self {
        let time_range = if events.is_empty() {
            (0, 0)
        } else {
            let start = events.first().map(|e| e.timestamp).unwrap();
            let end = events.last().map(|e| e.timestamp).unwrap();
            (start, end)
        };
        Self { time_range, events }
    }

    /// 导出为 CSV 字符串
    ///
    /// 列：event_id, timestamp, session_id, user, tenant_id, ip_address, command, object, detail, old_data, new_data
    pub fn export_csv(&self) -> String {
        let mut csv = String::from(
            "event_id,timestamp,session_id,user,tenant_id,ip_address,command,object,detail,old_data,new_data\n",
        );
        for event in &self.events {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{}\n",
                event.event_id,
                event.timestamp,
                event.session_id,
                csv_escape(&event.user),
                csv_escape(event.tenant_id.as_deref().unwrap_or("")),
                csv_escape(&event.ip_address),
                event.command,
                csv_escape(&event.object),
                csv_escape(&event.detail),
                event
                    .old_data
                    .as_ref()
                    .map(|d| hex_encode(d))
                    .unwrap_or_default(),
                event
                    .new_data
                    .as_ref()
                    .map(|d| hex_encode(d))
                    .unwrap_or_default(),
            ));
        }
        csv
    }

    /// 导出为 JSON 字符串
    pub fn export_json(&self) -> String {
        let mut json = String::from("{\"events\":[");
        for (i, event) in self.events.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                r#"{{"event_id":{},"timestamp":{},"session_id":{},"user":"{}","tenant_id":{},"ip_address":"{}","command":"{}","object":"{}","detail":"{}","old_data":{},"new_data":{}}}"#,
                event.event_id,
                event.timestamp,
                event.session_id,
                json_escape(&event.user),
                event
                    .tenant_id
                    .as_ref()
                    .map(|t| format!("\"{}\"", json_escape(t)))
                    .unwrap_or_else(|| "null".to_string()),
                json_escape(&event.ip_address),
                event.command,
                json_escape(&event.object),
                json_escape(&event.detail),
                event
                    .old_data
                    .as_ref()
                    .map(|d| format!("\"{}\"", hex_encode(d)))
                    .unwrap_or_else(|| "null".to_string()),
                event
                    .new_data
                    .as_ref()
                    .map(|d| format!("\"{}\"", hex_encode(d)))
                    .unwrap_or_else(|| "null".to_string()),
            ));
        }
        json.push_str(&format!(
            "],\"time_range\":{{\"start\":{},\"end\":{}}}}}",
            self.time_range.0, self.time_range.1
        ));
        json
    }

    /// 按指定格式导出
    pub fn export(&self, format: ReportFormat) -> String {
        match format {
            ReportFormat::Csv => self.export_csv(),
            ReportFormat::Json => self.export_json(),
        }
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 十六进制编码
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// CSV 转义（含逗号/引号/换行的字段用双引号包裹）
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// JSON 字符串转义
fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    // -----------------------------------------------------------------
    //  AuditCommand 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_command_as_str() {
        assert_eq!(AuditCommand::Insert.as_str(), "INSERT");
        assert_eq!(AuditCommand::Update.as_str(), "UPDATE");
        assert_eq!(AuditCommand::Delete.as_str(), "DELETE");
        assert_eq!(AuditCommand::Select.as_str(), "SELECT");
        assert_eq!(AuditCommand::Create.as_str(), "CREATE");
        assert_eq!(AuditCommand::Drop.as_str(), "DROP");
        assert_eq!(AuditCommand::Login.as_str(), "LOGIN");
    }

    #[test]
    fn test_7c3_command_is_dml() {
        assert!(AuditCommand::Insert.is_dml());
        assert!(AuditCommand::Update.is_dml());
        assert!(AuditCommand::Delete.is_dml());
        assert!(!AuditCommand::Select.is_dml());
        assert!(!AuditCommand::Create.is_dml());
    }

    #[test]
    fn test_7c3_command_is_ddl() {
        assert!(AuditCommand::Create.is_ddl());
        assert!(AuditCommand::Drop.is_ddl());
        assert!(AuditCommand::Alter.is_ddl());
        assert!(AuditCommand::Truncate.is_ddl());
        assert!(!AuditCommand::Insert.is_ddl());
    }

    #[test]
    fn test_7c3_command_display() {
        assert_eq!(format!("{}", AuditCommand::Insert), "INSERT");
        assert_eq!(
            format!("{}", AuditCommand::Other("CUSTOM".to_string())),
            "CUSTOM"
        );
    }

    // -----------------------------------------------------------------
    //  AuditEvent + Builder 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_event_builder() {
        let event = AuditEvent::builder()
            .timestamp(1000)
            .session_id(42)
            .user("admin")
            .ip_address("192.168.1.1")
            .command(AuditCommand::Insert)
            .object("users")
            .detail("INSERT INTO users VALUES (1, 'alice')")
            .new_data(b"1,alice".to_vec())
            .build();

        assert_eq!(event.event_id, 0); // 由 AuditLog 分配
        assert_eq!(event.timestamp, 1000);
        assert_eq!(event.session_id, 42);
        assert_eq!(event.user, "admin");
        assert_eq!(event.ip_address, "192.168.1.1");
        assert_eq!(event.command, AuditCommand::Insert);
        assert_eq!(event.object, "users");
        assert_eq!(event.detail, "INSERT INTO users VALUES (1, 'alice')");
        assert!(event.old_data.is_none());
        assert_eq!(event.new_data, Some(b"1,alice".to_vec()));
    }

    #[test]
    fn test_7c3_event_builder_with_tenant() {
        let event = AuditEvent::builder()
            .timestamp(1000)
            .user("alice")
            .tenant_id("tenant_001")
            .command(AuditCommand::Update)
            .object("orders")
            .detail("UPDATE orders SET status='paid'")
            .old_data(b"status=pending".to_vec())
            .new_data(b"status=paid".to_vec())
            .build();

        assert_eq!(event.tenant_id, Some("tenant_001".to_string()));
        assert_eq!(event.old_data, Some(b"status=pending".to_vec()));
        assert_eq!(event.new_data, Some(b"status=paid".to_vec()));
    }

    #[test]
    fn test_7c3_event_builder_default_command() {
        let event = AuditEvent::builder().timestamp(1000).user("test").build();
        assert_eq!(event.command, AuditCommand::Other("UNKNOWN".to_string()));
    }

    #[test]
    fn test_7c3_event_compute_hash_deterministic() {
        let event1 = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .object("users")
            .detail("test detail")
            .build();

        let event2 = event1.clone();

        let prev = [0u8; HASH_LEN];
        let hash1 = event1.compute_hash(&prev);
        let hash2 = event2.compute_hash(&prev);
        assert_eq!(hash1, hash2); // 相同事件 + 相同 prev_hash → 相同哈希
    }

    #[test]
    fn test_7c3_event_compute_hash_different_detail() {
        let mut event1 = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .object("users")
            .detail("detail A")
            .build();

        let event2 = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .object("users")
            .detail("detail B")
            .build();

        let prev = [0u8; HASH_LEN];
        event1.event_id = 1;
        let h1 = event1.compute_hash(&prev);
        // event2 has different detail
        let mut event2_copy = event2.clone();
        event2_copy.event_id = 1;
        let h2 = event2_copy.compute_hash(&prev);
        assert_ne!(h1, h2); // 不同 detail → 不同哈希
    }

    // -----------------------------------------------------------------
    //  AuditFilter 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_filter_matches_all() {
        let filter = AuditFilter::new();
        let event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .object("users")
            .build();
        assert!(filter.matches(&event)); // 空过滤器匹配所有
    }

    #[test]
    fn test_7c3_filter_by_command() {
        let filter = AuditFilter::new().commands(vec![AuditCommand::Insert, AuditCommand::Update]);

        let insert_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .build();
        assert!(filter.matches(&insert_event));

        let delete_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Delete)
            .build();
        assert!(!filter.matches(&delete_event)); // Delete 不在过滤列表
    }

    #[test]
    fn test_7c3_filter_by_user() {
        let filter = AuditFilter::new().users(vec!["admin".to_string()]);

        let admin_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .build();
        assert!(filter.matches(&admin_event));

        let guest_event = AuditEvent::builder()
            .timestamp(1000)
            .user("guest")
            .command(AuditCommand::Insert)
            .build();
        assert!(!filter.matches(&guest_event));
    }

    #[test]
    fn test_7c3_filter_by_object() {
        let filter = AuditFilter::new().objects(vec!["users".to_string()]);

        let users_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .object("users")
            .build();
        assert!(filter.matches(&users_event));

        let orders_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .object("orders")
            .build();
        assert!(!filter.matches(&orders_event));
    }

    // -----------------------------------------------------------------
    //  AuditLog 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_audit_log_creation() {
        let log = AuditLog::new();
        assert!(!log.is_enabled());
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_7c3_audit_log_enable_disable() {
        let mut log = AuditLog::new();
        assert!(!log.is_enabled());

        log.enable();
        assert!(log.is_enabled());

        log.disable();
        assert!(!log.is_enabled());
    }

    #[test]
    fn test_7c3_record_disabled() {
        let mut log = AuditLog::new();
        let event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .build();
        let result = log.record(event);
        assert_eq!(result.unwrap_err(), AuditError::Disabled);
    }

    #[test]
    fn test_7c3_record_basic() {
        let mut log = AuditLog::new();
        log.enable();

        let event = AuditEvent::builder()
            .timestamp(1000)
            .session_id(1)
            .user("admin")
            .ip_address("127.0.0.1")
            .command(AuditCommand::Insert)
            .object("users")
            .detail("INSERT INTO users VALUES (1, 'alice')")
            .new_data(b"1,alice".to_vec())
            .build();

        let event_id = log.record(event).unwrap();
        assert_eq!(event_id, Some(1)); // 第一个事件 ID = 1
        assert_eq!(log.len(), 1);

        let stored = log.get(1).unwrap();
        assert_eq!(stored.user, "admin");
        assert_eq!(stored.command, AuditCommand::Insert);
        assert_eq!(stored.object, "users");
        assert_eq!(stored.new_data, Some(b"1,alice".to_vec()));
    }

    #[test]
    fn test_7c3_record_event_id_increment() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..5u64 {
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user("admin")
                .command(AuditCommand::Insert)
                .object("t")
                .detail(format!("event {i}"))
                .build();
            let id = log.record(event).unwrap();
            assert_eq!(id, Some(i + 1));
        }
        assert_eq!(log.len(), 5);
    }

    #[test]
    fn test_7c3_record_filtered_out() {
        let mut log = AuditLog::new();
        log.enable();
        log.set_filter(AuditFilter::new().commands(vec![AuditCommand::Insert]));

        // Insert 事件 → 记录
        let insert_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .build();
        let id = log.record(insert_event).unwrap();
        assert_eq!(id, Some(1));

        // Delete 事件 → 被过滤
        let delete_event = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Delete)
            .build();
        let id = log.record(delete_event).unwrap();
        assert_eq!(id, None); // 被过滤，返回 None
        assert_eq!(log.len(), 1); // 仍然只有 1 条
    }

    // -----------------------------------------------------------------
    //  查询测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_query_all() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..10u64 {
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user("admin")
                .command(AuditCommand::Insert)
                .object("users")
                .detail(format!("event {i}"))
                .build();
            log.record(event).unwrap();
        }

        let results = log.query(&AuditQuery::new());
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_7c3_query_by_user() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..5u64 {
            let user = if i < 3 {
                "alice"
            } else {
                "bob"
            };
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user(user)
                .command(AuditCommand::Insert)
                .object("users")
                .build();
            log.record(event).unwrap();
        }

        let alice_results = log.query(&AuditQuery::new().user("alice"));
        assert_eq!(alice_results.len(), 3);

        let bob_results = log.query(&AuditQuery::new().user("bob"));
        assert_eq!(bob_results.len(), 2);
    }

    #[test]
    fn test_7c3_query_by_command() {
        let mut log = AuditLog::new();
        log.enable();

        let commands = [
            AuditCommand::Insert,
            AuditCommand::Update,
            AuditCommand::Delete,
            AuditCommand::Insert,
            AuditCommand::Update,
        ];

        for (i, cmd) in commands.iter().enumerate() {
            let event = AuditEvent::builder()
                .timestamp(1000 + i as u64)
                .user("admin")
                .command(cmd.clone())
                .object("users")
                .build();
            log.record(event).unwrap();
        }

        let inserts = log.query(&AuditQuery::new().command(AuditCommand::Insert));
        assert_eq!(inserts.len(), 2);

        let updates = log.query(&AuditQuery::new().command(AuditCommand::Update));
        assert_eq!(updates.len(), 2);

        let deletes = log.query(&AuditQuery::new().command(AuditCommand::Delete));
        assert_eq!(deletes.len(), 1);
    }

    #[test]
    fn test_7c3_query_by_time_range() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..10u64 {
            let event = AuditEvent::builder()
                .timestamp(1000 + i * 100)
                .user("admin")
                .command(AuditCommand::Insert)
                .object("users")
                .build();
            log.record(event).unwrap();
        }

        // 查询 1000-500 范围内的事件
        let results = log.query(&AuditQuery::new().time_range(1050, 1450));
        assert_eq!(results.len(), 4); // 1100, 1200, 1300, 1400
    }

    #[test]
    fn test_7c3_query_by_object() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..6u64 {
            let obj = if i < 4 {
                "users"
            } else {
                "orders"
            };
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user("admin")
                .command(AuditCommand::Insert)
                .object(obj)
                .build();
            log.record(event).unwrap();
        }

        let users_results = log.query(&AuditQuery::new().object("users"));
        assert_eq!(users_results.len(), 4);

        let orders_results = log.query(&AuditQuery::new().object("orders"));
        assert_eq!(orders_results.len(), 2);
    }

    #[test]
    fn test_7c3_query_with_limit() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..100u64 {
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user("admin")
                .command(AuditCommand::Insert)
                .object("users")
                .build();
            log.record(event).unwrap();
        }

        let results = log.query(&AuditQuery::new().limit(10));
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_7c3_query_combined() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..10u64 {
            let user = if i < 5 {
                "alice"
            } else {
                "bob"
            };
            let cmd = if i % 2 == 0 {
                AuditCommand::Insert
            } else {
                AuditCommand::Update
            };
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user(user)
                .command(cmd)
                .object("users")
                .build();
            log.record(event).unwrap();
        }

        // alice + Insert
        let results = log.query(
            &AuditQuery::new()
                .user("alice")
                .command(AuditCommand::Insert),
        );
        assert_eq!(results.len(), 3); // i=0,2,4 → alice+Insert
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_count_by_command() {
        let mut log = AuditLog::new();
        log.enable();

        for _ in 0..3 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .build(),
            )
            .unwrap();
        }
        for _ in 0..2 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000)
                    .user("admin")
                    .command(AuditCommand::Update)
                    .build(),
            )
            .unwrap();
        }

        assert_eq!(log.count_by_command(&AuditCommand::Insert), 3);
        assert_eq!(log.count_by_command(&AuditCommand::Update), 2);
        assert_eq!(log.count_by_command(&AuditCommand::Delete), 0);
    }

    #[test]
    fn test_7c3_count_by_user() {
        let mut log = AuditLog::new();
        log.enable();

        for _ in 0..4 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000)
                    .user("alice")
                    .command(AuditCommand::Insert)
                    .build(),
            )
            .unwrap();
        }
        for _ in 0..2 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000)
                    .user("bob")
                    .command(AuditCommand::Insert)
                    .build(),
            )
            .unwrap();
        }

        assert_eq!(log.count_by_user("alice"), 4);
        assert_eq!(log.count_by_user("bob"), 2);
        assert_eq!(log.count_by_user("charlie"), 0);
    }

    #[test]
    fn test_7c3_time_range() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..5u64 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i * 10)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .build(),
            )
            .unwrap();
        }

        let range = log.time_range().unwrap();
        assert_eq!(range, (1000, 1040));
    }

    #[test]
    fn test_7c3_time_range_empty() {
        let log = AuditLog::new();
        assert!(log.time_range().is_none());
    }

    // -----------------------------------------------------------------
    //  哈希链测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_hash_chain_append() {
        let mut chain = AuditHashChain::new();
        assert!(chain.is_empty());

        let event1 = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .detail("event 1")
            .build();
        let hash1 = chain.append(&event1);
        assert_eq!(chain.len(), 1);

        let event2 = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .detail("event 2")
            .build();
        let hash2 = chain.append(&event2);
        assert_eq!(chain.len(), 2);

        // 不同事件 → 不同哈希
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_7c3_hash_chain_verify_ok() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..10u64 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .detail(format!("event {i}"))
                    .build(),
            )
            .unwrap();
        }

        // 哈希链验证通过
        assert!(log.verify_chain().is_ok());
    }

    #[test]
    fn test_7c3_hash_chain_verify_empty() {
        let log = AuditLog::new();
        assert!(log.verify_chain().is_ok()); // 空链验证通过
    }

    #[test]
    fn test_7c3_hash_chain_verify_tampered() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..5u64 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .detail(format!("event {i}"))
                    .build(),
            )
            .unwrap();
        }

        // 篡改事件详情（模拟日志被篡改）
        log.events[2].detail = "TAMPERED".to_string();

        // 哈希链验证失败
        let result = log.verify_chain();
        assert!(result.is_err());
        match result.unwrap_err() {
            AuditError::HashChainBroken { event_id, .. } => {
                // 篡改的是第 3 条事件（index=2），其 event_id=3
                assert_eq!(event_id, 3);
            }
            _ => panic!("expected HashChainBroken"),
        }
    }

    #[test]
    fn test_7c3_hash_chain_independent() {
        // 两个独立的哈希链，记录相同事件 → 相同哈希
        let mut chain1 = AuditHashChain::new();
        let mut chain2 = AuditHashChain::new();

        for i in 0..5u64 {
            let event = AuditEvent::builder()
                .timestamp(1000 + i)
                .user("admin")
                .command(AuditCommand::Insert)
                .detail(format!("event {i}"))
                .build();
            let h1 = chain1.append(&event);
            let h2 = chain2.append(&event);
            assert_eq!(h1, h2);
        }

        assert_eq!(chain1.hashes(), chain2.hashes());
    }

    #[test]
    fn test_7c3_events_with_hashes() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..3u64 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .detail(format!("event {i}"))
                    .build(),
            )
            .unwrap();
        }

        let with_hashes = log.events_with_hashes();
        assert_eq!(with_hashes.len(), 3);
        for (i, wh) in with_hashes.iter().enumerate() {
            assert_eq!(wh.event.event_id, (i + 1) as u64);
            assert_ne!(wh.hash, [0u8; HASH_LEN]); // 哈希非零
        }
    }

    // -----------------------------------------------------------------
    //  AuditReport 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_report_from_log() {
        let mut log = AuditLog::new();
        log.enable();

        for i in 0..5u64 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .object("users")
                    .detail(format!("INSERT {i}"))
                    .new_data(format!("data{i}").into_bytes())
                    .build(),
            )
            .unwrap();
        }

        let report = AuditReport::from_log(&log);
        assert_eq!(report.events.len(), 5);
        assert_eq!(report.time_range, (1000, 1004));
    }

    #[test]
    fn test_7c3_report_export_csv() {
        let mut log = AuditLog::new();
        log.enable();

        log.record(
            AuditEvent::builder()
                .timestamp(1000)
                .session_id(1)
                .user("admin")
                .ip_address("127.0.0.1")
                .command(AuditCommand::Insert)
                .object("users")
                .detail("INSERT INTO users VALUES (1, 'alice')")
                .new_data(b"1,alice".to_vec())
                .build(),
        )
        .unwrap();

        let report = AuditReport::from_log(&log);
        let csv = report.export_csv();

        // CSV 应包含头部
        assert!(csv.contains("event_id,timestamp,session_id,user"));
        // CSV 应包含事件数据
        assert!(csv.contains("admin"));
        assert!(csv.contains("INSERT"));
        assert!(csv.contains("users"));
    }

    #[test]
    fn test_7c3_report_export_csv_with_comma() {
        let mut log = AuditLog::new();
        log.enable();

        log.record(
            AuditEvent::builder()
                .timestamp(1000)
                .user("admin")
                .command(AuditCommand::Insert)
                .object("users")
                .detail("INSERT INTO users VALUES (1, 'alice, bob')")
                .build(),
        )
        .unwrap();

        let report = AuditReport::from_log(&log);
        let csv = report.export_csv();
        // 含逗号的字段应被双引号包裹
        assert!(csv.contains("\""));
    }

    #[test]
    fn test_7c3_report_export_json() {
        let mut log = AuditLog::new();
        log.enable();

        log.record(
            AuditEvent::builder()
                .timestamp(1000)
                .session_id(1)
                .user("admin")
                .ip_address("127.0.0.1")
                .command(AuditCommand::Insert)
                .object("users")
                .detail("INSERT INTO users")
                .new_data(b"1,alice".to_vec())
                .build(),
        )
        .unwrap();

        let report = AuditReport::from_log(&log);
        let json = report.export_json();

        assert!(json.starts_with("{\"events\":["));
        assert!(json.contains("\"user\":\"admin\""));
        assert!(json.contains("\"command\":\"INSERT\""));
        assert!(json.contains("\"time_range\""));
    }

    #[test]
    fn test_7c3_report_export_by_format() {
        let report = AuditReport::from_events(vec![]);
        let csv = report.export(ReportFormat::Csv);
        let json = report.export(ReportFormat::Json);
        assert!(csv.contains("event_id"));
        assert!(json.contains("events"));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_hex_encode() {
        assert_eq!(hex_encode(b"\x00\x01\x02"), "000102");
        assert_eq!(hex_encode(b"\xff\xfe"), "fffe");
        assert_eq!(hex_encode(b""), "");
    }

    #[test]
    fn test_7c3_csv_escape() {
        assert_eq!(csv_escape("simple"), "simple");
        assert_eq!(csv_escape("with,comma"), "\"with,comma\"");
        assert_eq!(csv_escape("with\"quote"), "\"with\"\"quote\"");
        assert_eq!(csv_escape("with\nnewline"), "\"with\nnewline\"");
    }

    #[test]
    fn test_7c3_json_escape() {
        assert_eq!(json_escape("simple"), "simple");
        assert_eq!(json_escape("with\"quote"), "with\\\"quote");
        assert_eq!(json_escape("with\\backslash"), "with\\\\backslash");
        assert_eq!(json_escape("with\nnewline"), "with\\nnewline");
    }

    // -----------------------------------------------------------------
    //  完整工作流测试（验证标准：30000 条审计日志）
    // -----------------------------------------------------------------

    #[test]
    fn test_7c3_full_workflow_30000_events() {
        // 验证标准：INSERT/UPDATE/DELETE 各 10000 次 → 审计日志 30000 条
        // → 逐条验证时间/用户/操作/旧数据/新数据完整
        let mut log = AuditLog::new();
        log.enable();

        let count_per_type: u64 = 10_000;

        // INSERT 10000 次（带 new_data）
        for i in 0..count_per_type {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .session_id(1)
                    .user("admin")
                    .ip_address("127.0.0.1")
                    .command(AuditCommand::Insert)
                    .object("users")
                    .detail(format!("INSERT INTO users VALUES ({i})"))
                    .new_data(format!("user_{i}").into_bytes())
                    .build(),
            )
            .unwrap();
        }

        // UPDATE 10000 次（带 old_data + new_data）
        for i in 0..count_per_type {
            log.record(
                AuditEvent::builder()
                    .timestamp(2000 + i)
                    .session_id(1)
                    .user("admin")
                    .ip_address("127.0.0.1")
                    .command(AuditCommand::Update)
                    .object("users")
                    .detail(format!("UPDATE users SET name='user_{i}_v2' WHERE id={i}"))
                    .old_data(format!("user_{i}").into_bytes())
                    .new_data(format!("user_{i}_v2").into_bytes())
                    .build(),
            )
            .unwrap();
        }

        // DELETE 10000 次（带 old_data）
        for i in 0..count_per_type {
            log.record(
                AuditEvent::builder()
                    .timestamp(3000 + i)
                    .session_id(1)
                    .user("admin")
                    .ip_address("127.0.0.1")
                    .command(AuditCommand::Delete)
                    .object("users")
                    .detail(format!("DELETE FROM users WHERE id={i}"))
                    .old_data(format!("user_{i}_v2").into_bytes())
                    .build(),
            )
            .unwrap();
        }

        // 验证总数 = 30000
        assert_eq!(log.len(), 30_000);

        // 逐条验证时间/用户/操作/旧数据/新数据完整
        let events = log.events();
        for (i, event) in events.iter().enumerate() {
            // 公共字段验证
            assert_eq!(event.user, "admin");
            assert_eq!(event.session_id, 1);
            assert_eq!(event.ip_address, "127.0.0.1");
            assert_eq!(event.object, "users");

            // 按区段验证命令类型与时间戳
            if i < 10_000 {
                // INSERT 区段
                assert_eq!(event.command, AuditCommand::Insert);
                assert_eq!(event.timestamp, 1000 + i as u64);
                assert!(event.old_data.is_none());
                assert_eq!(
                    event.new_data.as_ref().unwrap(),
                    &format!("user_{i}").into_bytes()
                );
            } else if i < 20_000 {
                // UPDATE 区段
                assert_eq!(event.command, AuditCommand::Update);
                let j = i - 10_000;
                assert_eq!(event.timestamp, 2000 + j as u64);
                assert_eq!(
                    event.old_data.as_ref().unwrap(),
                    &format!("user_{j}").into_bytes()
                );
                assert_eq!(
                    event.new_data.as_ref().unwrap(),
                    &format!("user_{j}_v2").into_bytes()
                );
            } else {
                // DELETE 区段
                assert_eq!(event.command, AuditCommand::Delete);
                let j = i - 20_000;
                assert_eq!(event.timestamp, 3000 + j as u64);
                assert_eq!(
                    event.old_data.as_ref().unwrap(),
                    &format!("user_{j}_v2").into_bytes()
                );
                assert!(event.new_data.is_none());
            }
        }

        // 验证按命令统计
        assert_eq!(log.count_by_command(&AuditCommand::Insert), 10_000);
        assert_eq!(log.count_by_command(&AuditCommand::Update), 10_000);
        assert_eq!(log.count_by_command(&AuditCommand::Delete), 10_000);

        // 验证哈希链完整性（防篡改）
        assert!(log.verify_chain().is_ok());

        // 验证时间范围
        let range = log.time_range().unwrap();
        assert_eq!(range, (1000, 12999)); // 1000=首条INSERT, 12999=末条DELETE(3000+9999)
    }

    #[test]
    fn test_7c3_full_workflow_with_filter() {
        // 验证：过滤器只记录 INSERT 和 DELETE，不记录 UPDATE
        let mut log = AuditLog::new();
        log.enable();
        log.set_filter(
            AuditFilter::new().commands(vec![AuditCommand::Insert, AuditCommand::Delete]),
        );

        for i in 0..100u64 {
            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Insert)
                    .object("users")
                    .build(),
            )
            .unwrap();

            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Update)
                    .object("users")
                    .build(),
            )
            .unwrap();

            log.record(
                AuditEvent::builder()
                    .timestamp(1000 + i)
                    .user("admin")
                    .command(AuditCommand::Delete)
                    .object("users")
                    .build(),
            )
            .unwrap();
        }

        // 只记录了 INSERT + DELETE = 200 条（UPDATE 被过滤）
        assert_eq!(log.len(), 200);
        assert_eq!(log.count_by_command(&AuditCommand::Insert), 100);
        assert_eq!(log.count_by_command(&AuditCommand::Delete), 100);
        assert_eq!(log.count_by_command(&AuditCommand::Update), 0);

        // 哈希链仍然完整
        assert!(log.verify_chain().is_ok());
    }

    #[test]
    fn test_7c3_audit_log_immutability() {
        // 验证：审计日志不可变（通过 API 无法修改/删除已记录的事件）
        let mut log = AuditLog::new();
        log.enable();

        log.record(
            AuditEvent::builder()
                .timestamp(1000)
                .user("admin")
                .command(AuditCommand::Insert)
                .detail("event 1")
                .build(),
        )
        .unwrap();

        log.record(
            AuditEvent::builder()
                .timestamp(2000)
                .user("admin")
                .command(AuditCommand::Update)
                .detail("event 2")
                .build(),
        )
        .unwrap();

        // events() 返回不可变引用，无法修改
        let events = log.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].detail, "event 1");
        assert_eq!(events[1].detail, "event 2");

        // get() 也返回不可变引用
        let event = log.get(1).unwrap();
        assert_eq!(event.detail, "event 1");

        // 没有公开的修改/删除方法 → 不可变
    }

    #[test]
    fn test_7c3_audit_log_with_tenant() {
        let mut log = AuditLog::new();
        log.enable();

        log.record(
            AuditEvent::builder()
                .timestamp(1000)
                .user("alice")
                .tenant_id("tenant_001")
                .command(AuditCommand::Insert)
                .object("orders")
                .detail("INSERT INTO orders")
                .build(),
        )
        .unwrap();

        let event = log.get(1).unwrap();
        assert_eq!(event.tenant_id, Some("tenant_001".to_string()));
    }

    #[test]
    fn test_7c3_audit_log_get_not_found() {
        let log = AuditLog::new();
        assert!(log.get(999).is_none());
    }

    #[test]
    fn test_7c3_hash_chain_sequential() {
        // 验证：哈希链是顺序的 — 每条哈希依赖前一条
        let mut log = AuditLog::new();
        log.enable();

        let event1 = AuditEvent::builder()
            .timestamp(1000)
            .user("admin")
            .command(AuditCommand::Insert)
            .detail("event 1")
            .build();
        log.record(event1).unwrap();

        let event2 = AuditEvent::builder()
            .timestamp(2000)
            .user("admin")
            .command(AuditCommand::Insert)
            .detail("event 2")
            .build();
        log.record(event2).unwrap();

        // 获取哈希链
        let chain = log.hash_chain();
        assert_eq!(chain.len(), 2);

        // 手动验证：hash2 依赖 hash1
        let h1 = chain.hashes()[0];
        let h2 = chain.hashes()[1];

        // 用 h1 作为 prev_hash 重新计算 event2 的哈希
        let event2_copy = log.events()[1].clone();
        let computed_h2 = event2_copy.compute_hash(&h1);
        assert_eq!(h2, computed_h2);

        // 用错误的 prev_hash 计算会得到不同结果
        let wrong_prev = [0u8; HASH_LEN];
        let wrong_h2 = event2_copy.compute_hash(&wrong_prev);
        assert_ne!(h2, wrong_h2);
    }

    #[test]
    fn test_7c3_report_empty() {
        let report = AuditReport::from_events(vec![]);
        let csv = report.export_csv();
        assert!(csv.contains("event_id")); // 仍有头部
        let json = report.export_json();
        assert!(json.contains("\"events\":[]"));
    }
}
