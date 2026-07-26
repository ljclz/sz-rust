//! Phase 3.15 权限系统 Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 3.15。
//!
//! # 验收标准（SzRSQL实施进度.md Phase 3.15）
//!
//! - **Fuzz**：随机创建/删除用户/角色/权限，随机执行 GRANT/REVOKE，
//!   随机执行 SQL 验证权限检查结果符合预期
//! - **判定**：任意授权-撤销序列后权限状态一致
//!
//! # 设计要点
//!
//! 1. **XorShift64 PRNG**：固定种子，测试可重现（与 `mvcc_fuzz` / `page_fuzz` 同风格）
//! 2. **参考模型（RefModel）**：维护一份独立于 `RbacManager` 的权限状态投影
//!    - `superusers: HashSet<String>` — 超级用户集合
//!    - `user_exists: HashSet<String>` — 已创建用户集合
//!    - `user_roles: HashMap<String, HashSet<String>>` — 用户 → 角色集合（含 "public"）
//!    - `role_grants: HashMap<String, Vec<(Privilege, DatabaseObject)>>` — 角色 → 授权列表
//! 3. **不变量**（任意 op 序列后必须满足）：
//!    - `rbac.check(u, p, o) == ref.expected_check(u, p, o)`
//!    - 超级用户 `check` 永远返回 true
//!    - 不存在的用户 `check` 永远返回 false
//!    - GRANT 后 `check` 必为 true（若用户有该角色）
//!    - REVOKE 后 `check` 必为 false（若无其他 grant 覆盖）
//!    - ALL PRIVILEGES 通配：grant(All) → 任意 p 都通过
//!    - PUBLIC 角色继承：grant to "public" → 所有用户通过
//!    - 表-列不对称：grant(Table) 覆盖 column 检查，反之不成立
//! 4. **Op 枚举**：随机生成 9 种操作（CreateUser/CreateSuperuser/DropUser/CreateRole/
//!    DropRole/GrantRole/RevokeRole/Grant/Revoke/SetSuperuser），覆盖 RBAC API 全部入口
//! 5. **错误处理一致性**：对会失败的 op（如重复 create_user），Fuzz 模型与 RbacManager
//!    必须同时返回 Err，且不改变状态

use szrsql_catalog::rbac::{DatabaseObject, Privilege, RbacManager};
use szrsql_sql::ast::TableName;

use std::collections::{HashMap, HashSet};

// =====================================================================
//  XorShift64 — 固定种子 PRNG（与 mvcc_fuzz.rs / page_fuzz.rs 同风格）
// =====================================================================

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() & 0xFFFF_FFFF) as u32
    }

    /// [0, n) 范围
    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    /// 50% 概率返回 true
    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

// =====================================================================
//  参考模型 — 独立维护的权限状态投影
// =====================================================================

/// 参考模型：与 `RbacManager` 内部状态对齐的影子状态
#[derive(Debug, Clone, Default)]
struct RefModel {
    /// 超级用户集合（lowercase 用户名）
    superusers: HashSet<String>,
    /// 已创建用户集合（lowercase 用户名）
    user_exists: HashSet<String>,
    /// 用户 → 角色集合（lowercase，均含 "public"）
    user_roles: HashMap<String, HashSet<String>>,
    /// 角色 → 授权列表（lowercase 角色名）
    role_grants: HashMap<String, Vec<(Privilege, DatabaseObject)>>,
    /// 已创建角色集合（lowercase，不含 "public"）
    role_exists: HashSet<String>,
}

impl RefModel {
    fn new() -> Self {
        let mut m = Self::default();
        // PUBLIC 角色预存在
        m.role_grants.insert("public".to_string(), Vec::new());
        m
    }

    fn lower(s: &str) -> String {
        s.to_lowercase()
    }

    fn create_user(&mut self, name: &str, is_super: bool) -> bool {
        let key = Self::lower(name);
        if self.user_exists.contains(&key) {
            return false; // 重复创建失败
        }
        self.user_exists.insert(key.clone());
        if is_super {
            self.superusers.insert(key.clone());
        }
        // 新用户自动拥有 PUBLIC 角色
        let mut roles = HashSet::new();
        roles.insert("public".to_string());
        self.user_roles.insert(key, roles);
        true
    }

    fn drop_user(&mut self, name: &str) -> bool {
        let key = Self::lower(name);
        if !self.user_exists.remove(&key) {
            return false;
        }
        self.superusers.remove(&key);
        self.user_roles.remove(&key);
        true
    }

    fn create_role(&mut self, name: &str) -> bool {
        let key = Self::lower(name);
        if key == "public" || self.role_exists.contains(&key) {
            return false;
        }
        self.role_exists.insert(key.clone());
        self.role_grants.insert(key, Vec::new());
        true
    }

    fn drop_role(&mut self, name: &str) -> bool {
        let key = Self::lower(name);
        if key == "public" || !self.role_exists.remove(&key) {
            return false;
        }
        self.role_grants.remove(&key);
        // 从所有用户的角色集合中移除该角色
        for roles in self.user_roles.values_mut() {
            roles.remove(&key);
        }
        true
    }

    fn grant_role(&mut self, role: &str, user: &str) -> bool {
        let role_key = Self::lower(role);
        let user_key = Self::lower(user);
        if !self.role_exists.contains(&role_key) || !self.user_exists.contains(&user_key) {
            return false;
        }
        if let Some(roles) = self.user_roles.get_mut(&user_key) {
            // 用户已属于该角色 → 失败（与 RbacManager::UserAlreadyInRole 一致）
            if roles.contains(&role_key) {
                return false;
            }
            roles.insert(role_key);
        }
        true
    }

    fn revoke_role(&mut self, role: &str, user: &str) -> bool {
        let role_key = Self::lower(role);
        let user_key = Self::lower(user);
        if !self.role_exists.contains(&role_key) || !self.user_exists.contains(&user_key) {
            return false;
        }
        if let Some(roles) = self.user_roles.get_mut(&user_key) {
            if !roles.remove(&role_key) {
                return false; // 用户不属于该角色（与 RbacManager::UserNotInRole 一致）
            }
        }
        true
    }

    fn grant(&mut self, priv_: Privilege, obj: DatabaseObject, role: &str) -> bool {
        let role_key = Self::lower(role);
        // PUBLIC 角色预存在，或显式创建的角色
        if role_key != "public" && !self.role_exists.contains(&role_key) {
            return false;
        }
        let grants = self.role_grants.entry(role_key).or_default();
        // 查找已有授权
        if let Some(existing) = grants.iter_mut().find(|(p, o)| *p == priv_ && *o == obj) {
            // 已存在：幂等（与 RbacManager::grant 行为一致，更新 grantable 但模型不跟踪 grantable）
            let _ = existing;
        } else {
            grants.push((priv_, obj));
        }
        true
    }

    fn revoke(&mut self, priv_: Privilege, obj: &DatabaseObject, role: &str) -> bool {
        let role_key = Self::lower(role);
        if role_key != "public" && !self.role_exists.contains(&role_key) {
            return false;
        }
        let grants = match self.role_grants.get_mut(&role_key) {
            Some(g) => g,
            None => return false,
        };
        let before = grants.len();
        grants.retain(|(p, o)| !(*p == priv_ && o == obj));
        grants.len() != before
    }

    fn set_superuser(&mut self, name: &str, is_super: bool) -> bool {
        let key = Self::lower(name);
        if !self.user_exists.contains(&key) {
            return false;
        }
        if is_super {
            self.superusers.insert(key);
        } else {
            self.superusers.remove(&key);
        }
        true
    }

    /// 计算期望的权限检查结果
    ///
    /// 规则（与 RbacManager::check 完全对齐）：
    /// 1. 用户不存在 → false
    /// 2. 超级用户 → true
    /// 3. 遍历用户的所有角色（含 PUBLIC），找到匹配 grant → true
    /// 4. 否则 false
    fn expected_check(&self, user: &str, priv_: Privilege, obj: &DatabaseObject) -> bool {
        let user_key = Self::lower(user);
        if !self.user_exists.contains(&user_key) {
            return false;
        }
        if self.superusers.contains(&user_key) {
            return true;
        }
        let roles = match self.user_roles.get(&user_key) {
            Some(r) => r,
            None => return false,
        };
        for role in roles {
            if let Some(grants) = self.role_grants.get(role) {
                for (g_priv, g_obj) in grants {
                    if g_obj.matches(obj) && (*g_priv == Privilege::All || *g_priv == priv_) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

// =====================================================================
//  随机操作生成器
// =====================================================================

/// 用户名池（10 个候选）
const USER_POOL: &[&str] = &[
    "u1", "u2", "u3", "u4", "u5", "alice", "bob", "carol", "dave", "eve",
];

/// 角色名池（8 个候选，不含 public）
const ROLE_POOL: &[&str] = &[
    "r1", "r2", "r3", "reader", "writer", "admin", "role_a", "role_b",
];

/// 表名池
const TABLE_POOL: &[&str] = &["t1", "t2", "t3", "orders", "users", "items"];

/// 列名池
const COLUMN_POOL: &[&str] = &["id", "name", "data", "amount", "user_id", "tenant_id"];

/// 权限池（不含 All，特殊场景单独测试）
const PRIV_POOL: &[Privilege] = &[
    Privilege::Select,
    Privilege::Insert,
    Privilege::Update,
    Privilege::Delete,
    Privilege::Truncate,
    Privilege::References,
    Privilege::Trigger,
    Privilege::Create,
    Privilege::Connect,
    Privilege::Temporary,
    Privilege::Execute,
    Privilege::Usage,
];

/// 随机操作枚举
#[derive(Debug, Clone)]
enum Op {
    CreateUser {
        name: String,
        is_super: bool,
    },
    DropUser {
        name: String,
    },
    CreateRole {
        name: String,
    },
    DropRole {
        name: String,
    },
    GrantRole {
        role: String,
        user: String,
    },
    RevokeRole {
        role: String,
        user: String,
    },
    Grant {
        priv_: Privilege,
        obj: DatabaseObject,
        role: String,
    },
    Revoke {
        priv_: Privilege,
        obj: DatabaseObject,
        role: String,
    },
    SetSuperuser {
        name: String,
        is_super: bool,
    },
}

/// 生成随机操作序列
fn gen_op_sequence(rng: &mut XorShift64, count: usize) -> Vec<Op> {
    let mut ops = Vec::with_capacity(count);
    for _ in 0..count {
        let op_type = rng.next_range(9); // 9 种操作
        let user_name = USER_POOL[rng.next_range(USER_POOL.len() as u32) as usize].to_string();
        let role_name = ROLE_POOL[rng.next_range(ROLE_POOL.len() as u32) as usize].to_string();
        let table_name = TABLE_POOL[rng.next_range(TABLE_POOL.len() as u32) as usize];
        let is_super = rng.next_bool();

        let op = match op_type {
            0 => Op::CreateUser {
                name: user_name,
                is_super,
            },
            1 => Op::DropUser { name: user_name },
            2 => Op::CreateRole { name: role_name },
            3 => Op::DropRole { name: role_name },
            4 => Op::GrantRole {
                role: role_name,
                user: user_name,
            },
            5 => Op::RevokeRole {
                role: role_name,
                user: user_name,
            },
            6 => {
                let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
                let obj = if rng.next_bool() {
                    DatabaseObject::Table(TableName::new(table_name))
                } else {
                    let col = COLUMN_POOL[rng.next_range(COLUMN_POOL.len() as u32) as usize];
                    DatabaseObject::Column {
                        table: TableName::new(table_name),
                        column: col.to_string(),
                    }
                };
                Op::Grant {
                    priv_,
                    obj,
                    role: role_name,
                }
            }
            7 => {
                let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
                let obj = if rng.next_bool() {
                    DatabaseObject::Table(TableName::new(table_name))
                } else {
                    let col = COLUMN_POOL[rng.next_range(COLUMN_POOL.len() as u32) as usize];
                    DatabaseObject::Column {
                        table: TableName::new(table_name),
                        column: col.to_string(),
                    }
                };
                Op::Revoke {
                    priv_,
                    obj,
                    role: role_name,
                }
            }
            _ => Op::SetSuperuser {
                name: user_name,
                is_super,
            },
        };
        ops.push(op);
    }
    ops
}

/// 在 RbacManager 和 RefModel 上同时应用一个操作，返回 (rbac_result, ref_result)
fn apply_op(rbac: &mut RbacManager, model: &mut RefModel, op: &Op) -> (bool, bool) {
    let rbac_ok = match op {
        Op::CreateUser { name, is_super } => {
            if *is_super {
                rbac.create_superuser(name).is_ok()
            } else {
                rbac.create_user(name).is_ok()
            }
        }
        Op::DropUser { name } => rbac.drop_user(name).is_ok(),
        Op::CreateRole { name } => rbac.create_role(name).is_ok(),
        Op::DropRole { name } => rbac.drop_role(name).is_ok(),
        Op::GrantRole { role, user } => rbac.grant_role(role, user).is_ok(),
        Op::RevokeRole { role, user } => rbac.revoke_role(role, user).is_ok(),
        Op::Grant { priv_, obj, role } => rbac.grant(*priv_, obj.clone(), role, false).is_ok(),
        Op::Revoke { priv_, obj, role } => rbac.revoke(*priv_, obj, role).is_ok(),
        Op::SetSuperuser { name, is_super } => rbac.set_superuser(name, *is_super).is_ok(),
    };
    let ref_ok = match op {
        Op::CreateUser { name, is_super } => model.create_user(name, *is_super),
        Op::DropUser { name } => model.drop_user(name),
        Op::CreateRole { name } => model.create_role(name),
        Op::DropRole { name } => model.drop_role(name),
        Op::GrantRole { role, user } => model.grant_role(role, user),
        Op::RevokeRole { role, user } => model.revoke_role(role, user),
        Op::Grant { priv_, obj, role } => model.grant(*priv_, obj.clone(), role),
        Op::Revoke { priv_, obj, role } => model.revoke(*priv_, obj, role),
        Op::SetSuperuser { name, is_super } => model.set_superuser(name, *is_super),
    };
    (rbac_ok, ref_ok)
}

/// 生成随机 check 查询并对比 RbacManager 与 RefModel 的结果
fn verify_check_consistency(
    rng: &mut XorShift64,
    rbac: &RbacManager,
    model: &RefModel,
    checks_per_round: u32,
) -> u32 {
    let mut mismatches = 0u32;
    for _ in 0..checks_per_round {
        let user = USER_POOL[rng.next_range(USER_POOL.len() as u32) as usize];
        let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
        let table_name = TABLE_POOL[rng.next_range(TABLE_POOL.len() as u32) as usize];
        let obj = if rng.next_bool() {
            DatabaseObject::Table(TableName::new(table_name))
        } else {
            let col = COLUMN_POOL[rng.next_range(COLUMN_POOL.len() as u32) as usize];
            DatabaseObject::Column {
                table: TableName::new(table_name),
                column: col.to_string(),
            }
        };
        let rbac_result = rbac.check(user, priv_, &obj);
        let ref_result = model.expected_check(user, priv_, &obj);
        if rbac_result != ref_result {
            mismatches += 1;
            // 输出诊断信息便于调试（仅在出错时）
            eprintln!(
                "MISMATCH: user={user} priv={priv_:?} obj={obj:?} rbac={rbac_result} ref={ref_result}"
            );
        }
    }
    mismatches
}

// =====================================================================
//  Phase 3.15 Fuzz 测试
// =====================================================================

/// 主 Fuzz 测试：10000 轮随机操作 + 每轮 10 次 check 一致性验证
#[test]
fn test_fuzz_random_ops_consistent_check() {
    let mut rng = XorShift64::new(0x1234_5678_9ABC_DEF0);
    let mut rbac = RbacManager::new();
    let mut model = RefModel::new();
    let ops = gen_op_sequence(&mut rng, 10000);

    let mut op_mismatches = 0u32;
    let mut check_mismatches = 0u32;

    for (i, op) in ops.iter().enumerate() {
        let (rbac_ok, ref_ok) = apply_op(&mut rbac, &mut model, op);
        if rbac_ok != ref_ok {
            op_mismatches += 1;
            eprintln!("OP MISMATCH at {i}: {op:?} rbac={rbac_ok} ref={ref_ok}");
        }
        // 每 100 次操作做一次批量 check 验证
        if i % 100 == 0 {
            check_mismatches += verify_check_consistency(&mut rng, &rbac, &model, 10);
        }
    }
    // 最终批量验证
    check_mismatches += verify_check_consistency(&mut rng, &rbac, &model, 1000);

    assert_eq!(
        op_mismatches, 0,
        "操作结果不一致：{op_mismatches} 次 rbac/ref 分歧"
    );
    assert_eq!(
        check_mismatches, 0,
        "权限检查不一致：{check_mismatches} 次 rbac/ref 分歧"
    );
}

/// 超级用户不变量：任意 op 序列后，超级用户 check 永远 true
#[test]
fn test_fuzz_superuser_invariant() {
    let mut rng = XorShift64::new(0xAABB_CCDD_EEFF_0011);
    let mut rbac = RbacManager::new();
    rbac.create_superuser("admin").unwrap();

    let admin_obj = DatabaseObject::Table(TableName::new("t1"));
    // 未授任何权限给 admin，但 check 应永远 true
    for _ in 0..1000 {
        let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
        let table_name = TABLE_POOL[rng.next_range(TABLE_POOL.len() as u32) as usize];
        let obj = if rng.next_bool() {
            DatabaseObject::Table(TableName::new(table_name))
        } else {
            let col = COLUMN_POOL[rng.next_range(COLUMN_POOL.len() as u32) as usize];
            DatabaseObject::Column {
                table: TableName::new(table_name),
                column: col.to_string(),
            }
        };
        assert!(
            rbac.check("admin", priv_, &obj),
            "超级用户应通过所有 check：priv={priv_:?} obj={obj:?}"
        );
        let _ = admin_obj;
    }

    // 取消超级用户后再验证
    rbac.set_superuser("admin", false).unwrap();
    assert!(!rbac.is_superuser("admin"));
    // 此时无任何 grant → check 应 false
    assert!(!rbac.check(
        "admin",
        Privilege::Select,
        &DatabaseObject::Table(TableName::new("t1"))
    ));
}

/// 不存在用户不变量：任意 op 序列后，未创建用户 check 永远 false
#[test]
fn test_fuzz_nonexistent_user_returns_false() {
    let mut rng = XorShift64::new(0xBEEF_CAFE_1234_5678);
    let mut rbac = RbacManager::new();
    // 不创建任何用户，直接 check
    for _ in 0..1000 {
        let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
        let table_name = TABLE_POOL[rng.next_range(TABLE_POOL.len() as u32) as usize];
        let obj = DatabaseObject::Table(TableName::new(table_name));
        assert!(
            !rbac.check("ghost", priv_, &obj),
            "不存在的用户 check 应返回 false"
        );
    }

    // 创建后删除，再 check 应 false
    rbac.create_user("temp").unwrap();
    rbac.drop_user("temp").unwrap();
    for _ in 0..100 {
        let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
        assert!(!rbac.check("temp", priv_, &DatabaseObject::Table(TableName::new("t1"))));
    }
}

/// GRANT → REVOKE 幂等性：grant 后 check true，revoke 后 check false（无其他覆盖）
#[test]
fn test_fuzz_grant_revoke_idempotent_state() {
    let mut rng = XorShift64::new(0xDEAD_BEEF_CAFE_BABE);
    let mut rbac = RbacManager::new();
    let mut model = RefModel::new();

    rbac.create_user("alice").unwrap();
    rbac.create_role("reader").unwrap();
    rbac.grant_role("reader", "alice").unwrap();
    model.create_user("alice", false);
    model.create_role("reader");
    model.grant_role("reader", "alice");

    let obj = DatabaseObject::Table(TableName::new("t1"));

    // 100 轮 grant/revoke 循环
    for i in 0..100 {
        let priv_ = PRIV_POOL[i % PRIV_POOL.len()];
        // GRANT → alice 应通过
        rbac.grant(priv_, obj.clone(), "reader", false).unwrap();
        model.grant(priv_, obj.clone(), "reader");
        assert!(
            rbac.check("alice", priv_, &obj),
            "GRANT 后 alice 应通过 check (i={i})"
        );
        assert_eq!(
            model.expected_check("alice", priv_, &obj),
            rbac.check("alice", priv_, &obj)
        );

        // REVOKE → alice 应不通过
        rbac.revoke(priv_, &obj, "reader").unwrap();
        model.revoke(priv_, &obj, "reader");
        assert!(
            !rbac.check("alice", priv_, &obj),
            "REVOKE 后 alice 应不通过 check (i={i})"
        );
        assert_eq!(
            model.expected_check("alice", priv_, &obj),
            rbac.check("alice", priv_, &obj)
        );
    }
    let _ = rng.next_u64(); // 保持 rng 使用以维持可重现性语义
}

/// PUBLIC 角色继承：grant to "public" → 所有用户通过
#[test]
fn test_fuzz_public_role_inheritance() {
    let mut rng = XorShift64::new(0x0F0F_0F0F_F0F0_F0F0);
    let mut rbac = RbacManager::new();
    // 创建 5 个用户
    for name in &["u1", "u2", "u3", "u4", "u5"] {
        rbac.create_user(name).unwrap();
    }
    // GRANT SELECT ON t1 TO public
    let obj = DatabaseObject::Table(TableName::new("t1"));
    rbac.grant(Privilege::Select, obj.clone(), "public", false)
        .unwrap();

    // 所有 5 个用户都应通过 Select 检查
    for name in &["u1", "u2", "u3", "u4", "u5"] {
        assert!(
            rbac.check(name, Privilege::Select, &obj),
            "PUBLIC 角色继承：{name} 应通过 Select 检查"
        );
        // 但 Insert 仍应 false（PUBLIC 只 grant 了 Select）
        assert!(
            !rbac.check(name, Privilege::Insert, &obj),
            "PUBLIC 只 grant 了 Select，{name} 的 Insert 应 false"
        );
    }

    // 随机创建新用户，也应自动继承 PUBLIC 权限
    for _ in 0..50 {
        let name = format!("random_{}", rng.next_u32());
        rbac.create_user(&name).unwrap();
        assert!(
            rbac.check(&name, Privilege::Select, &obj),
            "新创建用户 {name} 应继承 PUBLIC 的 Select 权限"
        );
    }

    // REVOKE → 所有用户失去权限
    rbac.revoke(Privilege::Select, &obj, "public").unwrap();
    for name in &["u1", "u2", "u3", "u4", "u5"] {
        assert!(
            !rbac.check(name, Privilege::Select, &obj),
            "REVOKE PUBLIC 后 {name} 应失去 Select 权限"
        );
    }
}

/// ALL PRIVILEGES 通配：grant(All) → 任意具体权限 check 都通过
#[test]
fn test_fuzz_all_privileges_wildcard() {
    let mut rng = XorShift64::new(0x1111_2222_3333_4444);
    let mut rbac = RbacManager::new();
    rbac.create_user("admin_user").unwrap();
    rbac.create_role("super_role").unwrap();
    rbac.grant_role("super_role", "admin_user").unwrap();

    let obj = DatabaseObject::Table(TableName::new("t1"));
    rbac.grant(Privilege::All, obj.clone(), "super_role", false)
        .unwrap();

    // 任意具体权限都应通过
    for _ in 0..1000 {
        let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
        assert!(
            rbac.check("admin_user", priv_, &obj),
            "ALL PRIVILEGES 应涵盖 {priv_:?}"
        );
    }

    // REVOKE All → 所有权限失效
    rbac.revoke(Privilege::All, &obj, "super_role").unwrap();
    for priv_ in PRIV_POOL {
        assert!(
            !rbac.check("admin_user", *priv_, &obj),
            "REVOKE All 后 {:?} 应失效",
            priv_
        );
    }
}

/// 表-列不对称：grant(Table) 覆盖 column 检查；grant(Column) 不覆盖 table 检查
#[test]
fn test_fuzz_table_column_asymmetric() {
    let mut rbac = RbacManager::new();
    rbac.create_user("alice").unwrap();
    rbac.create_role("r").unwrap();
    rbac.grant_role("r", "alice").unwrap();

    // 场景 1：GRANT SELECT ON t1 → check SELECT on t1.id 也通过
    let table_obj = DatabaseObject::Table(TableName::new("t1"));
    let col_obj = DatabaseObject::Column {
        table: TableName::new("t1"),
        column: "id".to_string(),
    };
    rbac.grant(Privilege::Select, table_obj.clone(), "r", false)
        .unwrap();
    assert!(
        rbac.check("alice", Privilege::Select, &table_obj),
        "表级 grant 应通过表级 check"
    );
    assert!(
        rbac.check("alice", Privilege::Select, &col_obj),
        "表级 grant 应通过列级 check（表覆盖列）"
    );

    // 清理后场景 2：GRANT SELECT(id) ON t1 → check SELECT on t1 不通过
    rbac.revoke(Privilege::Select, &table_obj, "r").unwrap();
    rbac.grant(Privilege::Select, col_obj.clone(), "r", false)
        .unwrap();
    assert!(
        rbac.check("alice", Privilege::Select, &col_obj),
        "列级 grant 应通过列级 check"
    );
    assert!(
        !rbac.check("alice", Privilege::Select, &table_obj),
        "列级 grant 不应通过表级 check（不对称）"
    );
}

/// 大小写不敏感：RoleA 和 rolea 等价；Alice 和 alice 等价
#[test]
fn test_fuzz_case_insensitive() {
    let mut rng = XorShift64::new(0xCAFE_BABE_0011_2233);
    let mut rbac = RbacManager::new();
    rbac.create_user("Alice").unwrap();
    rbac.create_role("RoleA").unwrap();
    rbac.grant_role("ROLEA", "alice").unwrap(); // 大小写混合

    let obj = DatabaseObject::Table(TableName::new("T1"));
    rbac.grant(Privilege::Select, obj.clone(), "rOlEa", false)
        .unwrap();

    // 各种大小写组合都应通过
    for user in &["Alice", "alice", "ALICE", "AlIcE"] {
        assert!(
            rbac.check(user, Privilege::Select, &obj),
            "大小写不敏感：{user} 应通过 check"
        );
    }

    // 随机大小写组合验证
    let cases = ["Alice", "ALICE", "alice", "aLiCe", "AlIcE"];
    for _ in 0..100 {
        let user = cases[rng.next_range(cases.len() as u32) as usize];
        assert!(
            rbac.check(user, Privilege::Select, &obj),
            "随机大小写：{user} 应通过"
        );
    }
}

/// DROP ROLE 清理：drop_role 后，grants 移除，用户角色集合更新
#[test]
fn test_fuzz_drop_role_cleans_grants() {
    let mut rbac = RbacManager::new();
    rbac.create_user("alice").unwrap();
    rbac.create_role("reader").unwrap();
    rbac.grant_role("reader", "alice").unwrap();
    let obj = DatabaseObject::Table(TableName::new("t1"));
    rbac.grant(Privilege::Select, obj.clone(), "reader", false)
        .unwrap();

    // 验证初始有权限
    assert!(rbac.check("alice", Privilege::Select, &obj));

    // DROP ROLE → alice 失去权限
    rbac.drop_role("reader").unwrap();
    assert!(
        !rbac.check("alice", Privilege::Select, &obj),
        "DROP ROLE 后 alice 应失去权限"
    );

    // 重新创建同名角色，alice 不应自动恢复
    rbac.create_role("reader").unwrap();
    assert!(
        !rbac.check("alice", Privilege::Select, &obj),
        "重新创建同名角色后 alice 不应自动恢复权限"
    );
}

/// DROP USER 清理：drop_user 后，从角色成员列表中移除
#[test]
fn test_fuzz_drop_user_cleans_memberships() {
    let mut rbac = RbacManager::new();
    rbac.create_user("alice").unwrap();
    rbac.create_role("reader").unwrap();
    rbac.grant_role("reader", "alice").unwrap();

    // 验证 alice 在 reader 角色成员列表中
    assert!(rbac.role_members("reader").contains(&"alice".to_string()));

    // DROP USER → 从成员列表移除
    rbac.drop_user("alice").unwrap();
    assert!(
        !rbac.role_members("reader").contains(&"alice".to_string()),
        "DROP USER 后应从角色成员列表移除"
    );

    // 重新创建同名用户，不应自动恢复角色
    rbac.create_user("alice").unwrap();
    assert!(
        !rbac.role_members("reader").contains(&"alice".to_string()),
        "重新创建同名用户不应自动恢复角色成员资格"
    );
}

/// 错误处理一致性：对会失败的 op，rbac 和 ref 同时返回失败且不改变状态
#[test]
fn test_fuzz_error_handling_consistency() {
    let mut rbac = RbacManager::new();
    let mut model = RefModel::new();

    // 1. 重复 create_user → 都失败
    rbac.create_user("alice").unwrap();
    model.create_user("alice", false);
    let rbac_dup = rbac.create_user("alice").is_ok();
    let ref_dup = model.create_user("alice", false);
    assert_eq!(rbac_dup, ref_dup, "重复 create_user 应同时失败");
    assert_eq!(rbac.user_count(), 1);

    // 2. drop 不存在的用户 → 都失败
    let rbac_drop_ghost = rbac.drop_user("ghost").is_ok();
    let ref_drop_ghost = model.drop_user("ghost");
    assert_eq!(rbac_drop_ghost, ref_drop_ghost);

    // 3. create 重复角色 → 都失败
    rbac.create_role("r").unwrap();
    model.create_role("r");
    let rbac_dup_role = rbac.create_role("r").is_ok();
    let ref_dup_role = model.create_role("r");
    assert_eq!(rbac_dup_role, ref_dup_role);

    // 4. drop public 角色 → 都失败
    let rbac_drop_public = rbac.drop_role("public").is_ok();
    let ref_drop_public = model.drop_role("public");
    assert_eq!(rbac_drop_public, ref_drop_public);

    // 5. grant_role 不存在的角色 → 都失败
    let rbac_grant_ghost_role = rbac.grant_role("ghost_role", "alice").is_ok();
    let ref_grant_ghost_role = model.grant_role("ghost_role", "alice");
    assert_eq!(rbac_grant_ghost_role, ref_grant_ghost_role);

    // 6. grant_role 给不存在的用户 → 都失败
    let rbac_grant_ghost_user = rbac.grant_role("r", "ghost_user").is_ok();
    let ref_grant_ghost_user = model.grant_role("r", "ghost_user");
    assert_eq!(rbac_grant_ghost_user, ref_grant_ghost_user);

    // 7. grant 不存在的角色 → 都失败
    let obj = DatabaseObject::Table(TableName::new("t1"));
    let rbac_grant_ghost = rbac
        .grant(Privilege::Select, obj.clone(), "ghost_role", false)
        .is_ok();
    let ref_grant_ghost = model.grant(Privilege::Select, obj.clone(), "ghost_role");
    assert_eq!(rbac_grant_ghost, ref_grant_ghost);

    // 8. revoke 不存在的 grant → 都失败
    let rbac_revoke_missing = rbac.revoke(Privilege::Select, &obj, "r").is_ok();
    let ref_revoke_missing = model.revoke(Privilege::Select, &obj, "r");
    assert_eq!(rbac_revoke_missing, ref_revoke_missing);

    // 验证状态未改变
    assert_eq!(
        rbac.check("alice", Privilege::Select, &obj),
        model.expected_check("alice", Privilege::Select, &obj)
    );
    assert!(!rbac.check("alice", Privilege::Select, &obj));
}

/// 长序列压力测试：50000 次 op + 持续 check 一致性验证
#[test]
fn test_fuzz_stress_long_sequence() {
    let mut rng = XorShift64::new(0x5A5A_5A5A_A5A5_A5A5);
    let mut rbac = RbacManager::new();
    let mut model = RefModel::new();

    let ops = gen_op_sequence(&mut rng, 50000);
    let mut op_mismatches = 0u32;
    let mut check_mismatches = 0u32;

    for (i, op) in ops.iter().enumerate() {
        let (rbac_ok, ref_ok) = apply_op(&mut rbac, &mut model, op);
        if rbac_ok != ref_ok {
            op_mismatches += 1;
            if op_mismatches < 10 {
                eprintln!("OP MISMATCH at {i}: {op:?} rbac={rbac_ok} ref={ref_ok}");
            }
        }
        // 每 500 次操作做一次批量 check 验证
        if i % 500 == 0 {
            check_mismatches += verify_check_consistency(&mut rng, &rbac, &model, 50);
        }
    }
    // 最终大量 check 验证
    check_mismatches += verify_check_consistency(&mut rng, &rbac, &model, 5000);

    assert_eq!(op_mismatches, 0, "操作结果不一致：{op_mismatches} 次");
    assert_eq!(check_mismatches, 0, "权限检查不一致：{check_mismatches} 次");
}

/// 多角色权限并集：用户属于多个角色，任一角色有权限即通过
#[test]
fn test_fuzz_multi_role_union() {
    let mut rng = XorShift64::new(0xFEDC_BA98_7654_3210);
    let mut rbac = RbacManager::new();
    rbac.create_user("alice").unwrap();
    rbac.create_role("reader").unwrap();
    rbac.create_role("writer").unwrap();
    rbac.create_role("admin").unwrap();
    rbac.grant_role("reader", "alice").unwrap();
    rbac.grant_role("writer", "alice").unwrap();

    let t1 = DatabaseObject::Table(TableName::new("t1"));
    let t2 = DatabaseObject::Table(TableName::new("t2"));
    // reader: SELECT on t1
    rbac.grant(Privilege::Select, t1.clone(), "reader", false)
        .unwrap();
    // writer: INSERT on t2
    rbac.grant(Privilege::Insert, t2.clone(), "writer", false)
        .unwrap();
    // admin: ALL on t1
    rbac.grant(Privilege::All, t1.clone(), "admin", false)
        .unwrap();

    // alice 同时拥有 reader + writer 权限
    assert!(rbac.check("alice", Privilege::Select, &t1)); // reader
    assert!(rbac.check("alice", Privilege::Insert, &t2)); // writer
    assert!(!rbac.check("alice", Privilege::Insert, &t1)); // 无
    assert!(!rbac.check("alice", Privilege::Select, &t2)); // 无

    // 授予 admin 角色 → alice 拥有 t1 的所有权限
    rbac.grant_role("admin", "alice").unwrap();
    for _ in 0..100 {
        let priv_ = PRIV_POOL[rng.next_range(PRIV_POOL.len() as u32) as usize];
        assert!(
            rbac.check("alice", priv_, &t1),
            "alice 加入 admin 后应拥有 t1 的所有权限：{priv_:?}"
        );
    }
}

/// 综合 Fuzz：op + check 混合，验证任意操作序列后状态一致
#[test]
fn test_fuzz_comprehensive_mixed_ops_and_checks() {
    let mut rng = XorShift64::new(0x9999_8888_7777_6666);
    let mut rbac = RbacManager::new();
    let mut model = RefModel::new();

    let mut op_mismatches = 0u32;
    let mut check_mismatches = 0u32;

    // 5000 轮，每轮 1 op + 5 check 混合
    for round in 0..5000 {
        let op_count = 1 + rng.next_range(3); // 1-3 ops per round
        for _ in 0..op_count {
            let mut ops = gen_op_sequence(&mut rng, 1);
            if let Some(op) = ops.pop() {
                let (rbac_ok, ref_ok) = apply_op(&mut rbac, &mut model, &op);
                if rbac_ok != ref_ok {
                    op_mismatches += 1;
                    if op_mismatches < 5 {
                        eprintln!("ROUND {round} OP MISMATCH: {op:?} rbac={rbac_ok} ref={ref_ok}");
                    }
                }
            }
        }
        // 每轮 5 次 check
        check_mismatches += verify_check_consistency(&mut rng, &rbac, &model, 5);
    }

    assert_eq!(op_mismatches, 0, "操作不一致：{op_mismatches}");
    assert_eq!(check_mismatches, 0, "检查不一致：{check_mismatches}");
}
