//! Row-Level Security (RLS) — Phase 3.13.
//!
//! # Design
//!
//! Implements PostgreSQL-compatible Row-Level Security policies:
//! - **`CREATE POLICY p ON t FOR SELECT USING (expr)`** — filter visible rows for SELECT
//! - **`CREATE POLICY p ON t FOR INSERT WITH CHECK (expr)`** — verify inserted rows
//! - **`CREATE POLICY p ON t FOR UPDATE USING (expr) WITH CHECK (expr)`** — both filter + verify
//! - **`CREATE POLICY p ON t FOR DELETE USING (expr)`** — filter deletable rows
//! - **`ALTER TABLE t ENABLE ROW LEVEL SECURITY`** — turn on RLS for table
//! - **`ALTER TABLE t FORCE ROW LEVEL SECURITY`** — even owner subject to RLS
//! - **Per-role policies**: `CREATE POLICY p ON t TO role1, role2 USING (...)`
//! - **PERMISSIVE (default) vs RESTRICTIVE**: permissive combine with OR, restrictive with AND
//!
//! # Policy combination semantics (PG-compatible)
//!
//! For a given (table, command, user_roles):
//! 1. Collect all matching policies
//! 2. Split into PERMISSIVE and RESTRICTIVE
//! 3. PERMISSIVE: OR together — at least one must pass (if none, deny all)
//! 4. RESTRICTIVE: AND together — all must pass (if none, vacuously true)
//! 5. Final: `permissive_pass && restrictive_pass`
//!
//! # Special variables in policy expressions
//!
//! - `current_user` / `user` → the current user name (Text)
//! - `session_user` → the session user name (Text)
//! - `current_role` → the current role name (Text)
//!
//! These are resolved via `RlsContext::lookup_column` — the expression
//! `tenant_id = current_user` parses as `BinaryOp { Identifier(["tenant_id"]), Eq, Identifier(["current_user"]) }`
//! and `current_user` is intercepted as a special variable.
//!
//! Corresponds to `SzRSQL实施进度.md` Phase 3.13.

use std::collections::{HashMap, HashSet};

use szrsql_sql::{
    ast::{Expr, TableName},
    expr::{EvalContext, EvalError, ExprEvaluator, RowContext},
};
use szrsql_types::value::Value;
use thiserror::Error;

// =====================================================================
//  Policy command type
// =====================================================================

/// Policy command — which SQL command the policy applies to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyCommand {
    /// SELECT
    Select,
    /// INSERT
    Insert,
    /// UPDATE
    Update,
    /// DELETE
    Delete,
    /// ALL commands (matches any)
    All,
}

impl PolicyCommand {
    /// Check if this command matches a specific command
    /// (All matches everything; otherwise must be equal)
    pub fn matches(&self, other: PolicyCommand) -> bool {
        matches!(self, PolicyCommand::All) || *self == other
    }
}

// =====================================================================
//  Policy
// =====================================================================

/// RLS Policy — a single row-level security rule
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// Policy name (per-table unique)
    pub name: String,
    /// Target table
    pub table: TableName,
    /// Command (SELECT/INSERT/UPDATE/DELETE/ALL)
    pub command: PolicyCommand,
    /// Applicable roles (empty = PUBLIC, all users)
    pub roles: Vec<String>,
    /// USING expression — filters visible/deletable rows (SELECT/UPDATE/DELETE)
    pub using: Option<Expr>,
    /// WITH CHECK expression — verifies inserted/updated rows (INSERT/UPDATE)
    pub with_check: Option<Expr>,
    /// PERMISSIVE (true, default) or RESTRICTIVE (false)
    pub permissive: bool,
}

impl Policy {
    /// Create a new permissive SELECT policy with USING
    pub fn new_select(name: impl Into<String>, table: TableName, using: Expr) -> Self {
        Self {
            name: name.into(),
            table,
            command: PolicyCommand::Select,
            roles: Vec::new(),
            using: Some(using),
            with_check: None,
            permissive: true,
        }
    }

    /// Create a new permissive INSERT policy with WITH CHECK
    pub fn new_insert(name: impl Into<String>, table: TableName, with_check: Expr) -> Self {
        Self {
            name: name.into(),
            table,
            command: PolicyCommand::Insert,
            roles: Vec::new(),
            using: None,
            with_check: Some(with_check),
            permissive: true,
        }
    }

    /// Set roles (replaces existing)
    pub fn with_roles(mut self, roles: Vec<String>) -> Self {
        self.roles = roles;
        self
    }

    /// Set permissive flag (false = RESTRICTIVE)
    pub fn with_permissive(mut self, permissive: bool) -> Self {
        self.permissive = permissive;
        self
    }

    /// Check if this policy applies to a given role set
    /// (empty roles = PUBLIC = all users; otherwise any role must match)
    pub fn applies_to_roles(&self, user_roles: &[String]) -> bool {
        if self.roles.is_empty() {
            return true; // PUBLIC
        }
        self.roles
            .iter()
            .any(|r| user_roles.iter().any(|ur| ur.eq_ignore_ascii_case(r)))
    }
}

// =====================================================================
//  Error type
// =====================================================================

/// RLS error type
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RlsError {
    /// Policy already exists
    #[error("policy already exists: {0}")]
    PolicyAlreadyExists(String),
    /// Policy not found
    #[error("policy not found: {0}")]
    PolicyNotFound(String),
    /// Table not found
    #[error("table not found: {0}")]
    TableNotFound(String),
    /// RLS already enabled
    #[error("RLS already enabled on table: {0}")]
    AlreadyEnabled(String),
    /// RLS not enabled
    #[error("RLS not enabled on table: {0}")]
    NotEnabled(String),
    /// Invalid policy — missing required expression
    #[error("invalid policy: {0}")]
    InvalidPolicy(String),
}

// =====================================================================
//  RlsManager
// =====================================================================

/// RLS Manager — stores policies + RLS-enabled/forced flags per table
#[derive(Debug, Default, Clone)]
pub struct RlsManager {
    /// table_key (lowercase qualified) → Vec<Policy>
    policies: HashMap<String, Vec<Policy>>,
    /// table_key → RLS enabled
    enabled: HashSet<String>,
    /// table_key → RLS forced (even owner subject to RLS)
    forced: HashSet<String>,
}

impl RlsManager {
    /// Create empty manager
    pub fn new() -> Self {
        Self::default()
    }

    /// table_key (lowercase qualified name)
    fn table_key(table: &TableName) -> String {
        table.qualified_name().to_lowercase()
    }

    /// Enable RLS on a table
    pub fn enable_rls(&mut self, table: &TableName) -> Result<(), RlsError> {
        let key = Self::table_key(table);
        if self.enabled.contains(&key) {
            return Err(RlsError::AlreadyEnabled(table.qualified_name()));
        }
        self.enabled.insert(key);
        Ok(())
    }

    /// Disable RLS on a table
    pub fn disable_rls(&mut self, table: &TableName) -> Result<(), RlsError> {
        let key = Self::table_key(table);
        if !self.enabled.remove(&key) {
            return Err(RlsError::NotEnabled(table.qualified_name()));
        }
        // Also remove forced flag
        self.forced.remove(&key);
        Ok(())
    }

    /// Force RLS on a table (even owner subject to RLS)
    pub fn force_rls(&mut self, table: &TableName) -> Result<(), RlsError> {
        let key = Self::table_key(table);
        if !self.enabled.contains(&key) {
            return Err(RlsError::NotEnabled(table.qualified_name()));
        }
        self.forced.insert(key);
        Ok(())
    }

    /// Unforce RLS on a table
    pub fn unforce_rls(&mut self, table: &TableName) -> Result<(), RlsError> {
        let key = Self::table_key(table);
        self.forced.remove(&key);
        Ok(())
    }

    /// Check if RLS is enabled on a table
    pub fn is_enabled(&self, table: &TableName) -> bool {
        self.enabled.contains(&Self::table_key(table))
    }

    /// Check if RLS is forced on a table
    pub fn is_forced(&self, table: &TableName) -> bool {
        self.forced.contains(&Self::table_key(table))
    }

    /// Create a policy
    pub fn create_policy(&mut self, policy: Policy) -> Result<(), RlsError> {
        let key = Self::table_key(&policy.table);
        let policies = self.policies.entry(key).or_default();
        // Check duplicate name (per-table unique)
        if policies
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&policy.name))
        {
            return Err(RlsError::PolicyAlreadyExists(policy.name));
        }
        // Validate: SELECT/DELETE must have using; INSERT must have with_check; UPDATE can have both
        match policy.command {
            PolicyCommand::Select | PolicyCommand::Delete => {
                if policy.using.is_none() {
                    return Err(RlsError::InvalidPolicy(format!(
                        "{} policy requires USING expression",
                        format!("{:?}", policy.command).to_lowercase()
                    )));
                }
            }
            PolicyCommand::Insert => {
                if policy.with_check.is_none() {
                    return Err(RlsError::InvalidPolicy(
                        "insert policy requires WITH CHECK expression".into(),
                    ));
                }
            }
            PolicyCommand::Update => {
                if policy.using.is_none() && policy.with_check.is_none() {
                    return Err(RlsError::InvalidPolicy(
                        "update policy requires USING or WITH CHECK expression".into(),
                    ));
                }
            }
            PolicyCommand::All => {
                if policy.using.is_none() && policy.with_check.is_none() {
                    return Err(RlsError::InvalidPolicy(
                        "all policy requires USING or WITH CHECK expression".into(),
                    ));
                }
            }
        }
        policies.push(policy);
        Ok(())
    }

    /// Drop a policy by name
    pub fn drop_policy(&mut self, table: &TableName, name: &str) -> Result<(), RlsError> {
        let key = Self::table_key(table);
        let policies = self.policies.get_mut(&key).ok_or_else(|| {
            RlsError::PolicyNotFound(format!("{} on table {}", name, table.qualified_name()))
        })?;
        let before = policies.len();
        policies.retain(|p| !p.name.eq_ignore_ascii_case(name));
        if policies.len() == before {
            return Err(RlsError::PolicyNotFound(format!(
                "{} on table {}",
                name,
                table.qualified_name()
            )));
        }
        Ok(())
    }

    /// List all policies for a table
    pub fn policies_for_table(&self, table: &TableName) -> Vec<&Policy> {
        self.policies
            .get(&Self::table_key(table))
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// List policies matching (table, command, user_roles)
    pub fn matching_policies(
        &self,
        table: &TableName,
        command: PolicyCommand,
        user_roles: &[String],
    ) -> Vec<&Policy> {
        self.policies_for_table(table)
            .into_iter()
            .filter(|p| p.command.matches(command) && p.applies_to_roles(user_roles))
            .collect()
    }

    /// Total policy count
    pub fn policy_count(&self) -> usize {
        self.policies.values().map(|v| v.len()).sum()
    }

    /// Policy count for a specific table
    pub fn policy_count_for_table(&self, table: &TableName) -> usize {
        self.policies_for_table(table).len()
    }
}

// =====================================================================
//  RlsContext — EvalContext with current_user/session_user/current_role
// =====================================================================

/// RLS evaluation context — wraps a RowContext and adds session variables
/// (current_user / session_user / current_role)
pub struct RlsContext<'a> {
    /// Row data context
    pub row: &'a RowContext,
    /// Current user name
    pub current_user: String,
    /// Session user name (defaults to current_user)
    pub session_user: String,
    /// Current role name (defaults to current_user)
    pub current_role: String,
}

impl<'a> RlsContext<'a> {
    /// Create with current_user (session_user/current_role default to same)
    pub fn new(row: &'a RowContext, current_user: impl Into<String>) -> Self {
        let user = current_user.into();
        Self {
            row,
            session_user: user.clone(),
            current_role: user.clone(),
            current_user: user,
        }
    }

    /// Set session user (chainable)
    pub fn with_session_user(mut self, user: impl Into<String>) -> Self {
        self.session_user = user.into();
        self
    }

    /// Set current role (chainable)
    pub fn with_current_role(mut self, role: impl Into<String>) -> Self {
        self.current_role = role.into();
        self
    }
}

impl<'a> EvalContext for RlsContext<'a> {
    fn lookup_column(&self, name: &str) -> Result<Value, EvalError> {
        match name.to_lowercase().as_str() {
            "current_user" | "user" => Ok(Value::Text(self.current_user.clone())),
            "session_user" => Ok(Value::Text(self.session_user.clone())),
            "current_role" => Ok(Value::Text(self.current_role.clone())),
            _ => self.row.lookup_column(name),
        }
    }

    fn lookup_qualified(&self, table: &str, column: &str) -> Result<Value, EvalError> {
        match column.to_lowercase().as_str() {
            "current_user" | "user" => Ok(Value::Text(self.current_user.clone())),
            "session_user" => Ok(Value::Text(self.session_user.clone())),
            "current_role" => Ok(Value::Text(self.current_role.clone())),
            _ => self.row.lookup_qualified(table, column),
        }
    }
}

// =====================================================================
//  PolicyEvaluator — filter rows based on policies
// =====================================================================

/// Policy evaluator — applies RLS policies to rows
#[derive(Debug, Default, Clone)]
pub struct PolicyEvaluator;

impl PolicyEvaluator {
    /// Create new evaluator
    pub fn new() -> Self {
        Self
    }

    /// Build a RowContext from a row + schema (column name → value)
    fn build_row_context(row: &[Value], column_names: &[String]) -> RowContext {
        let mut ctx = RowContext::new();
        for (i, col_name) in column_names.iter().enumerate() {
            if let Some(val) = row.get(i) {
                ctx.columns.insert(col_name.to_lowercase(), val.clone());
            }
        }
        ctx
    }

    /// Evaluate a USING/WITH_CHECK expression against (row, user)
    /// Returns true if the expression passes (true), false otherwise
    /// (NULL, non-bool, errors → false, deny by default)
    fn eval_expr(expr: &Expr, row: &[Value], column_names: &[String], user: &str) -> bool {
        let row_ctx = Self::build_row_context(row, column_names);
        let rls_ctx = RlsContext::new(&row_ctx, user);
        match ExprEvaluator::eval(expr, &rls_ctx) {
            Ok(Value::Bool(b)) => b,
            Ok(Value::Null) => false,
            Ok(_) => false,  // Non-bool → false
            Err(_) => false, // Error → false (deny by default)
        }
    }

    /// Check if a single row passes the policy combination
    /// (permissive: OR; restrictive: AND; no permissive → deny)
    fn passes_policies(
        &self,
        row: &[Value],
        column_names: &[String],
        user: &str,
        policies: &[&Policy],
        use_with_check: bool,
    ) -> bool {
        let permissive: Vec<&&Policy> = policies.iter().filter(|p| p.permissive).collect();
        let restrictive: Vec<&&Policy> = policies.iter().filter(|p| !p.permissive).collect();

        // Permissive: at least one must pass (if none, deny)
        let permissive_pass = !permissive.is_empty()
            && permissive.iter().any(|p| {
                let expr = if use_with_check {
                    p.with_check.as_ref()
                } else {
                    p.using.as_ref()
                };
                match expr {
                    Some(e) => Self::eval_expr(e, row, column_names, user),
                    None => true, // No expr = always pass
                }
            });

        // Restrictive: all must pass
        let restrictive_pass = restrictive.iter().all(|p| {
            let expr = if use_with_check {
                p.with_check.as_ref()
            } else {
                p.using.as_ref()
            };
            match expr {
                Some(e) => Self::eval_expr(e, row, column_names, user),
                None => true,
            }
        });

        permissive_pass && restrictive_pass
    }

    /// Filter rows for SELECT command
    ///
    /// - If RLS not enabled → return all rows (no filtering)
    /// - If RLS enabled but no policies → return empty (deny-all default)
    /// - If `bypass_rls=true` → return all rows (owner bypass, unless FORCE RLS)
    #[allow(clippy::too_many_arguments)]
    pub fn filter_select(
        &self,
        rows: &[Vec<Value>],
        column_names: &[String],
        table: &TableName,
        user: &str,
        user_roles: &[String],
        manager: &RlsManager,
        bypass_rls: bool,
    ) -> Vec<Vec<Value>> {
        // Bypass RLS (table owner, unless FORCE RLS)
        if bypass_rls && !manager.is_forced(table) {
            return rows.to_vec();
        }
        // RLS not enabled → no filtering
        if !manager.is_enabled(table) {
            return rows.to_vec();
        }
        // Get matching SELECT policies
        let policies = manager.matching_policies(table, PolicyCommand::Select, user_roles);
        if policies.is_empty() {
            return Vec::new(); // Deny-all default
        }
        // Filter rows
        rows.iter()
            .filter(|row| self.passes_policies(row, column_names, user, &policies, false))
            .cloned()
            .collect()
    }

    /// Check if an INSERT row passes WITH CHECK policies
    ///
    /// Returns true if the row can be inserted.
    #[allow(clippy::too_many_arguments)]
    pub fn check_insert(
        &self,
        row: &[Value],
        column_names: &[String],
        table: &TableName,
        user: &str,
        user_roles: &[String],
        manager: &RlsManager,
        bypass_rls: bool,
    ) -> bool {
        if bypass_rls && !manager.is_forced(table) {
            return true;
        }
        if !manager.is_enabled(table) {
            return true; // RLS not enabled → no check
        }
        let policies = manager.matching_policies(table, PolicyCommand::Insert, user_roles);
        if policies.is_empty() {
            return false; // Deny-all default
        }
        self.passes_policies(row, column_names, user, &policies, true)
    }

    /// Check if a DELETE row passes USING policies
    ///
    /// Returns true if the row can be deleted.
    #[allow(clippy::too_many_arguments)]
    pub fn check_delete(
        &self,
        row: &[Value],
        column_names: &[String],
        table: &TableName,
        user: &str,
        user_roles: &[String],
        manager: &RlsManager,
        bypass_rls: bool,
    ) -> bool {
        if bypass_rls && !manager.is_forced(table) {
            return true;
        }
        if !manager.is_enabled(table) {
            return true;
        }
        let policies = manager.matching_policies(table, PolicyCommand::Delete, user_roles);
        if policies.is_empty() {
            return false;
        }
        self.passes_policies(row, column_names, user, &policies, false)
    }

    /// Check if an UPDATE passes both USING (old row) and WITH CHECK (new row)
    ///
    /// Returns true if the update is allowed.
    #[allow(clippy::too_many_arguments)]
    pub fn check_update(
        &self,
        old_row: &[Value],
        new_row: &[Value],
        column_names: &[String],
        table: &TableName,
        user: &str,
        user_roles: &[String],
        manager: &RlsManager,
        bypass_rls: bool,
    ) -> bool {
        if bypass_rls && !manager.is_forced(table) {
            return true;
        }
        if !manager.is_enabled(table) {
            return true;
        }
        let policies = manager.matching_policies(table, PolicyCommand::Update, user_roles);
        if policies.is_empty() {
            return false;
        }
        // USING on old row
        let using_pass = self.passes_policies(old_row, column_names, user, &policies, false);
        // WITH CHECK on new row
        let with_check_pass = self.passes_policies(new_row, column_names, user, &policies, true);
        using_pass && with_check_pass
    }
}
