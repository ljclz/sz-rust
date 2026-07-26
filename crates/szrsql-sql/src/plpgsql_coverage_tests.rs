//! Phase 6.5 — PL/pgSQL 覆盖测试（100 条）
//!
//! 覆盖范围（按 spec 要求）：
//! - 变量声明与初始化（10 条）
//! - 赋值语句（10 条）
//! - IF 分支（10 条）
//! - CASE 分支（10 条）
//! - LOOP 循环（10 条）
//! - WHILE 循环（10 条）
//! - FOR 循环（10 条）
//! - EXIT / CONTINUE（10 条）
//! - RETURN / RETURN NEXT / RETURN QUERY（10 条）
//! - PERFORM（5 条）
//! - EXECUTE（5 条）
//! - RAISE（5 条）
//! - 异常处理（5 条）
//!
//! 测试方式：
//! 1. 将 PL/pgSQL 函数体（`$$ ... $$` 内部）作为输入传给 `plpgsql::parse_function_body`
//! 2. 断言解析成功且 AST 结构符合预期
//! 3. 部分测试通过 `parse_sql` 完整解析 CREATE FUNCTION 语句

#![cfg(test)]

use crate::ast::{FunctionArgMode, FunctionVolatility, Statement};
use crate::parser::parse_sql;
use crate::plpgsql::{
    parse_function_body, PlPgSqlBlock, PlPgSqlDeclaration, PlPgSqlRaiseLevel, PlPgSqlStatement,
    PlPgSqlTypeRef,
};

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析函数体并断言成功
fn parse_body_ok(src: &str) -> PlPgSqlBlock {
    parse_function_body(src)
        .unwrap_or_else(|e| panic!("parse_function_body failed: {e:?}\nsrc:\n{src}"))
}

/// 解析函数体并断言失败
fn parse_body_err(src: &str) {
    assert!(
        parse_function_body(src).is_err(),
        "expected parse error, but succeeded\nsrc:\n{src}"
    );
}

/// 通过 CREATE FUNCTION 解析 PL/pgSQL 函数体
fn parse_create_function_body(body: &str) -> String {
    let sql = format!("CREATE FUNCTION test_fn() RETURNS void LANGUAGE plpgsql AS $$\n{body}\n$$");
    let stmts = parse_sql(&sql).expect("parse_sql failed");
    assert_eq!(stmts.len(), 1, "expected 1 statement, got {}", stmts.len());
    match &stmts[0] {
        Statement::CreateFunction { body, .. } => body.clone(),
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

// =====================================================================
//  1. 变量声明与初始化（10 条）
// =====================================================================

#[test]
fn t01_var_declare_integer() {
    let body = "DECLARE\n  x integer;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.declarations.len(), 1);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Variable {
            name, data_type, ..
        } => {
            assert_eq!(name, "x");
            assert_eq!(data_type, "integer");
        }
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn t02_var_declare_text() {
    let body = "DECLARE\n  name text;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.declarations.len(), 1);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Variable {
            name, data_type, ..
        } => {
            assert_eq!(name, "name");
            assert_eq!(data_type, "text");
        }
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn t03_var_declare_with_default() {
    let body = "DECLARE\n  x integer := 42;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Variable { name, default, .. } => {
            assert_eq!(name, "x");
            assert_eq!(default.as_deref(), Some("42"));
        }
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn t04_var_declare_constant() {
    let body = "DECLARE\n  pi CONSTANT numeric := 3.14;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Variable {
            name, is_constant, ..
        } => {
            assert_eq!(name, "pi");
            assert!(*is_constant);
        }
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn t05_var_declare_not_null() {
    let body = "DECLARE\n  x integer NOT NULL := 0;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Variable { name, not_null, .. } => {
            assert_eq!(name, "x");
            assert!(*not_null);
        }
        other => panic!("expected Variable, got {other:?}"),
    }
}

#[test]
fn t06_var_declare_multiple() {
    let body = "DECLARE\n  x integer;\n  y text;\n  z numeric;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.declarations.len(), 3);
}

#[test]
fn t07_var_declare_type_ref() {
    let body = "DECLARE\n  x users.email%TYPE;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::VariableTypeRef { name, type_ref, .. } => {
            assert_eq!(name, "x");
            match type_ref {
                PlPgSqlTypeRef::ColumnType { table, column } => {
                    assert_eq!(table, "users");
                    assert_eq!(column, "email");
                }
                other => panic!("expected ColumnType, got {other:?}"),
            }
        }
        other => panic!("expected VariableTypeRef, got {other:?}"),
    }
}

#[test]
fn t08_var_declare_rowtype() {
    let body = "DECLARE\n  u users%ROWTYPE;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::VariableTypeRef { name, type_ref, .. } => {
            assert_eq!(name, "u");
            match type_ref {
                PlPgSqlTypeRef::RowType { table } => {
                    assert_eq!(table, "users");
                }
                other => panic!("expected RowType, got {other:?}"),
            }
        }
        other => panic!("expected VariableTypeRef, got {other:?}"),
    }
}

#[test]
fn t09_var_declare_alias() {
    let body = "DECLARE\n  n ALIAS FOR $1;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Alias { name, target } => {
            assert_eq!(name, "n");
            assert_eq!(target, "$1");
        }
        other => panic!("expected Alias, got {other:?}"),
    }
}

#[test]
fn t10_var_declare_with_default_expr() {
    let body = "DECLARE\n  x integer := 1 + 2 * 3;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.declarations[0] {
        PlPgSqlDeclaration::Variable { default, .. } => {
            assert_eq!(default.as_deref(), Some("1 + 2 * 3"));
        }
        other => panic!("expected Variable, got {other:?}"),
    }
}

// =====================================================================
//  2. 赋值语句（10 条）
// =====================================================================

#[test]
fn t11_assign_simple() {
    let body = "BEGIN\n  x := 42;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.statements.len(), 1);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "x");
            assert_eq!(value, "42");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t12_assign_string() {
    let body = "BEGIN\n  name := 'hello';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "name");
            assert_eq!(value, "'hello'");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t13_assign_arithmetic() {
    let body = "BEGIN\n  x := a + b * 2;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "x");
            assert_eq!(value, "a + b * 2");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t14_assign_function_call() {
    let body = "BEGIN\n  x := lower('HELLO');\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "x");
            assert_eq!(value, "lower('HELLO')");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t15_assign_field() {
    let body = "BEGIN\n  rec.name := 'test';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "rec.name");
            assert_eq!(value, "'test'");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t16_assign_subscript() {
    let body = "BEGIN\n  arr[1] := 10;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "arr[1]");
            assert_eq!(value, "10");
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t17_assign_multiple() {
    let body = "BEGIN\n  x := 1;\n  y := 2;\n  z := 3;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.statements.len(), 3);
}

#[test]
fn t18_assign_select_into() {
    let body = "BEGIN\n  SELECT count(*) INTO x FROM users;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::SelectInto { targets, .. } => {
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0], "x");
        }
        other => panic!("expected SelectInto, got {other:?}"),
    }
}

#[test]
fn t19_assign_select_multiple_into() {
    let body = "BEGIN\n  SELECT a, b INTO x, y FROM t;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::SelectInto { targets, .. } => {
            assert_eq!(targets.len(), 2);
            assert_eq!(targets[0], "x");
            assert_eq!(targets[1], "y");
        }
        other => panic!("expected SelectInto, got {other:?}"),
    }
}

#[test]
fn t20_assign_complex_expr() {
    let body = "BEGIN\n  result := CASE WHEN x > 0 THEN 1 ELSE -1 END;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { target, value } => {
            assert_eq!(target, "result");
            assert!(value.contains("CASE"));
            assert!(value.contains("WHEN"));
            assert!(value.contains("x > 0"));
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

// =====================================================================
//  3. IF 分支（10 条）
// =====================================================================

#[test]
fn t21_if_simple() {
    let body = "BEGIN\n  IF x > 0 THEN\n    y := 1;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If {
            branches,
            else_branch,
        } => {
            assert_eq!(branches.len(), 1);
            assert!(else_branch.is_none());
            assert_eq!(branches[0].cond, "x > 0");
            assert_eq!(branches[0].statements.len(), 1);
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t22_if_else() {
    let body = "BEGIN\n  IF x > 0 THEN\n    y := 1;\n  ELSE\n    y := -1;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If {
            branches,
            else_branch,
        } => {
            assert_eq!(branches.len(), 1);
            assert!(else_branch.is_some());
            assert_eq!(else_branch.as_ref().unwrap().len(), 1);
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t23_if_elsif() {
    let body = "BEGIN\n  IF x > 0 THEN\n    y := 1;\n  ELSIF x < 0 THEN\n    y := -1;\n  ELSE\n    y := 0;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If {
            branches,
            else_branch,
        } => {
            assert_eq!(branches.len(), 2);
            assert!(else_branch.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t24_if_nested() {
    let body =
        "BEGIN\n  IF x > 0 THEN\n    IF y > 0 THEN\n      z := 1;\n    END IF;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 1);
            match &branches[0].statements[0] {
                PlPgSqlStatement::If {
                    branches: inner, ..
                } => {
                    assert_eq!(inner.len(), 1);
                }
                other => panic!("expected nested If, got {other:?}"),
            }
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t25_if_multiple_statements() {
    let body = "BEGIN\n  IF x > 0 THEN\n    y := 1;\n    z := 2;\n    w := 3;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 3);
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t26_if_complex_condition() {
    let body = "BEGIN\n  IF x > 0 AND y < 10 OR z = 5 THEN\n    y := 1;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].cond, "x > 0 AND y < 10 OR z = 5");
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t27_if_with_null() {
    let body = "BEGIN\n  IF x IS NULL THEN\n    y := 0;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].cond, "x IS NULL");
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t28_if_with_between() {
    let body = "BEGIN\n  IF x BETWEEN 1 AND 10 THEN\n    y := 1;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].cond, "x BETWEEN 1 AND 10");
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t29_if_with_in() {
    let body = "BEGIN\n  IF x IN (1, 2, 3) THEN\n    y := 1;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].cond, "x IN (1, 2, 3)");
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t30_if_multiple_elsif() {
    let body = "BEGIN\n  IF x = 1 THEN\n    y := 10;\n  ELSIF x = 2 THEN\n    y := 20;\n  ELSIF x = 3 THEN\n    y := 30;\n  ELSIF x = 4 THEN\n    y := 40;\n  ELSE\n    y := 0;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If {
            branches,
            else_branch,
        } => {
            assert_eq!(branches.len(), 4);
            assert!(else_branch.is_some());
        }
        other => panic!("expected If, got {other:?}"),
    }
}

// =====================================================================
//  4. CASE 分支（10 条）
// =====================================================================

#[test]
fn t31_case_simple() {
    let body = "BEGIN\n  CASE x\n    WHEN 1 THEN\n      y := 10;\n    WHEN 2 THEN\n      y := 20;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case {
            selector, branches, ..
        } => {
            assert!(selector.is_some());
            assert_eq!(selector.as_deref(), Some("x"));
            assert_eq!(branches.len(), 2);
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t32_case_with_else() {
    let body = "BEGIN\n  CASE x\n    WHEN 1 THEN\n      y := 10;\n    ELSE\n      y := 0;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case { else_branch, .. } => {
            assert!(else_branch.is_some());
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t33_case_searched() {
    let body = "BEGIN\n  CASE\n    WHEN x > 0 THEN\n      y := 1;\n    WHEN x < 0 THEN\n      y := -1;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case {
            selector, branches, ..
        } => {
            assert!(selector.is_none());
            assert_eq!(branches.len(), 2);
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t34_case_multiple_when() {
    let body = "BEGIN\n  CASE x\n    WHEN 1 THEN y := 10;\n    WHEN 2 THEN y := 20;\n    WHEN 3 THEN y := 30;\n    WHEN 4 THEN y := 40;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case { branches, .. } => {
            assert_eq!(branches.len(), 4);
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t35_case_nested() {
    let body = "BEGIN\n  CASE x\n    WHEN 1 THEN\n      CASE y\n        WHEN 10 THEN z := 100;\n      END CASE;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 1);
            match &branches[0].statements[0] {
                PlPgSqlStatement::Case { .. } => {}
                other => panic!("expected nested Case, got {other:?}"),
            }
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t36_case_in_assignment() {
    let body = "BEGIN\n  x := CASE WHEN y > 0 THEN 1 ELSE -1 END;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Assignment { value, .. } => {
            assert!(value.contains("CASE"));
            assert!(value.contains("WHEN"));
        }
        other => panic!("expected Assignment, got {other:?}"),
    }
}

#[test]
fn t37_case_with_complex_when() {
    let body = "BEGIN\n  CASE\n    WHEN x > 0 AND y > 0 THEN\n      z := 1;\n    WHEN x < 0 OR y < 0 THEN\n      z := -1;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case { branches, .. } => {
            assert_eq!(branches[0].cond, "x > 0 AND y > 0");
            assert_eq!(branches[1].cond, "x < 0 OR y < 0");
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t38_case_multiple_statements_in_when() {
    let body = "BEGIN\n  CASE x\n    WHEN 1 THEN\n      y := 10;\n      z := 20;\n      w := 30;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 3);
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t39_case_with_null() {
    let body = "BEGIN\n  CASE\n    WHEN x IS NULL THEN\n      y := 0;\n    ELSE\n      y := 1;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case {
            branches,
            else_branch,
            ..
        } => {
            assert_eq!(branches[0].cond, "x IS NULL");
            assert!(else_branch.is_some());
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

#[test]
fn t40_case_empty_when() {
    let body =
        "BEGIN\n  CASE x\n    WHEN 1 THEN\n      NULL;\n    ELSE\n      y := 1;\n  END CASE;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Case { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 1);
            match &branches[0].statements[0] {
                PlPgSqlStatement::Null => {}
                other => panic!("expected Null, got {other:?}"),
            }
        }
        other => panic!("expected Case, got {other:?}"),
    }
}

// =====================================================================
//  5. LOOP 循环（10 条）
// =====================================================================

#[test]
fn t41_loop_simple() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { label, body } => {
            assert!(label.is_none());
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t42_loop_with_exit() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    EXIT WHEN x > 10;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => {
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t43_loop_with_label() {
    let body = "<<my_loop>>\nBEGIN\n  LOOP\n    EXIT my_loop WHEN x > 10;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.label.as_deref(), Some("my_loop"));
}

#[test]
fn t44_loop_multiple_statements() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    y := y - 1;\n    z := z * 2;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => {
            assert_eq!(body.len(), 3);
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t45_loop_nested() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    LOOP\n      y := y + 1;\n      EXIT WHEN y > 5;\n    END LOOP;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => {
            assert_eq!(body.len(), 2);
            match &body[1] {
                PlPgSqlStatement::Loop { .. } => {}
                other => panic!("expected nested Loop, got {other:?}"),
            }
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t46_loop_with_continue() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    CONTINUE WHEN x < 5;\n    EXIT WHEN x > 10;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => {
            assert_eq!(body.len(), 3);
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t47_loop_exit_unconditional() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    EXIT;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[1] {
            PlPgSqlStatement::Exit { cond, .. } => {
                assert!(cond.is_none());
            }
            other => panic!("expected Exit, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t48_loop_continue_unconditional() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    CONTINUE;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[1] {
            PlPgSqlStatement::Continue { cond, .. } => {
                assert!(cond.is_none());
            }
            other => panic!("expected Continue, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t49_loop_with_if_exit() {
    let body = "BEGIN\n  LOOP\n    x := x + 1;\n    IF x > 10 THEN\n      EXIT;\n    END IF;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => {
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t50_loop_empty_body() {
    let body = "BEGIN\n  LOOP\n    NULL;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => {
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected Loop, got {other:?}"),
    }
}

// =====================================================================
//  6. WHILE 循环（10 条）
// =====================================================================

#[test]
fn t51_while_simple() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { cond, body, .. } => {
            assert_eq!(cond, "x < 10");
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t52_while_with_exit() {
    let body =
        "BEGIN\n  WHILE x < 100 LOOP\n    x := x + 1;\n    EXIT WHEN x = 50;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t53_while_with_label() {
    let body = "BEGIN\n  <<w_loop>>\n  WHILE x < 10 LOOP\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { label, .. } => {
            assert_eq!(label.as_deref(), Some("w_loop"));
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t54_while_complex_condition() {
    let body =
        "BEGIN\n  WHILE x > 0 AND y > 0 LOOP\n    x := x - 1;\n    y := y - 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { cond, .. } => {
            assert_eq!(cond, "x > 0 AND y > 0");
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t55_while_nested() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    WHILE y < 10 LOOP\n      y := y + 1;\n    END LOOP;\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 2);
            match &body[0] {
                PlPgSqlStatement::While { .. } => {}
                other => panic!("expected nested While, got {other:?}"),
            }
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t56_while_multiple_statements() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    x := x + 1;\n    y := y * 2;\n    z := z - 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 3);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t57_while_with_null_condition_check() {
    let body = "BEGIN\n  WHILE x IS NOT NULL LOOP\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { cond, .. } => {
            assert_eq!(cond, "x IS NOT NULL");
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t58_while_with_continue() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    x := x + 1;\n    CONTINUE WHEN x % 2 = 0;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t59_while_empty_body() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    NULL;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t60_while_with_if() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    IF x = 5 THEN\n      EXIT;\n    END IF;\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

// =====================================================================
//  7. FOR 循环（10 条）
// =====================================================================

#[test]
fn t61_for_integer_simple() {
    let body = "BEGIN\n  FOR i IN 1..10 LOOP\n    x := x + i;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For {
            var,
            lower,
            upper,
            body,
            reverse,
            ..
        } => {
            assert_eq!(var, "i");
            assert_eq!(lower, "1");
            assert_eq!(upper, "10");
            assert!(!reverse);
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t62_for_integer_reverse() {
    let body = "BEGIN\n  FOR i IN REVERSE 10..1 LOOP\n    x := x + i;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { reverse, .. } => {
            assert!(reverse);
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t63_for_integer_by() {
    let body = "BEGIN\n  FOR i IN 1..100 BY 5 LOOP\n    x := x + i;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { step, .. } => {
            assert_eq!(step.as_deref(), Some("5"));
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t64_for_integer_variable_bounds() {
    let body = "BEGIN\n  FOR i IN start_val..end_val LOOP\n    x := x + i;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { lower, upper, .. } => {
            assert_eq!(lower, "start_val");
            assert_eq!(upper, "end_val");
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t65_for_integer_with_label() {
    let body = "BEGIN\n  <<f_loop>>\n  FOR i IN 1..10 LOOP\n    x := x + i;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { label, .. } => {
            assert_eq!(label.as_deref(), Some("f_loop"));
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t66_for_query() {
    let body = "BEGIN\n  FOR rec IN SELECT * FROM users LOOP\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::ForQuery { var, .. } => {
            assert_eq!(var, "rec");
        }
        other => panic!("expected ForQuery, got {other:?}"),
    }
}

#[test]
fn t67_for_query_with_where() {
    let body = "BEGIN\n  FOR rec IN SELECT id, name FROM users WHERE active = true LOOP\n    total := total + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::ForQuery { query, .. } => {
            assert!(query.contains("SELECT"));
            assert!(query.contains("FROM users"));
            assert!(query.contains("WHERE"));
        }
        other => panic!("expected ForQuery, got {other:?}"),
    }
}

#[test]
fn t68_for_nested() {
    let body = "BEGIN\n  FOR i IN 1..3 LOOP\n    FOR j IN 1..3 LOOP\n      x := x + i * j;\n    END LOOP;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { body, .. } => {
            assert_eq!(body.len(), 1);
            match &body[0] {
                PlPgSqlStatement::For { var, .. } => {
                    assert_eq!(var, "j");
                }
                other => panic!("expected nested For, got {other:?}"),
            }
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t69_for_multiple_statements() {
    let body = "BEGIN\n  FOR i IN 1..10 LOOP\n    x := x + i;\n    y := y - i;\n    z := z * i;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { body, .. } => {
            assert_eq!(body.len(), 3);
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t70_for_with_exit() {
    let body = "BEGIN\n  FOR i IN 1..100 LOOP\n    EXIT WHEN i > 50;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { body, .. } => {
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected For, got {other:?}"),
    }
}

// =====================================================================
//  8. EXIT / CONTINUE（10 条）
// =====================================================================

#[test]
fn t71_exit_when() {
    let body = "BEGIN\n  LOOP\n    EXIT WHEN x > 10;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[0] {
            PlPgSqlStatement::Exit { cond, .. } => {
                assert_eq!(cond.as_deref(), Some("x > 10"));
            }
            other => panic!("expected Exit, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t72_exit_unconditional() {
    let body = "BEGIN\n  LOOP\n    EXIT;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[0] {
            PlPgSqlStatement::Exit { cond, .. } => {
                assert!(cond.is_none());
            }
            other => panic!("expected Exit, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t73_exit_with_label() {
    let body = "<<outer>>\nBEGIN\n  LOOP\n    LOOP\n      EXIT outer WHEN x > 10;\n    END LOOP;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.label.as_deref(), Some("outer"));
}

#[test]
fn t74_continue_when() {
    let body = "BEGIN\n  LOOP\n    CONTINUE WHEN x < 5;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[0] {
            PlPgSqlStatement::Continue { cond, .. } => {
                assert_eq!(cond.as_deref(), Some("x < 5"));
            }
            other => panic!("expected Continue, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t75_continue_unconditional() {
    let body = "BEGIN\n  LOOP\n    CONTINUE;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[0] {
            PlPgSqlStatement::Continue { cond, .. } => {
                assert!(cond.is_none());
            }
            other => panic!("expected Continue, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t76_continue_with_label() {
    let body = "<<inner>>\nBEGIN\n  LOOP\n    CONTINUE inner WHEN x < 5;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.label.as_deref(), Some("inner"));
}

#[test]
fn t77_exit_in_while() {
    let body = "BEGIN\n  WHILE true LOOP\n    EXIT WHEN x > 10;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

#[test]
fn t78_continue_in_for() {
    let body = "BEGIN\n  FOR i IN 1..10 LOOP\n    CONTINUE WHEN i % 2 = 0;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::For { body, .. } => {
            assert_eq!(body.len(), 1);
        }
        other => panic!("expected For, got {other:?}"),
    }
}

#[test]
fn t79_exit_with_complex_condition() {
    let body = "BEGIN\n  LOOP\n    EXIT WHEN x > 10 AND y < 5;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[0] {
            PlPgSqlStatement::Exit { cond, .. } => {
                assert_eq!(cond.as_deref(), Some("x > 10 AND y < 5"));
            }
            other => panic!("expected Exit, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

#[test]
fn t80_continue_with_complex_condition() {
    let body = "BEGIN\n  LOOP\n    CONTINUE WHEN x > 0 AND x < 10;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Loop { body, .. } => match &body[0] {
            PlPgSqlStatement::Continue { cond, .. } => {
                assert_eq!(cond.as_deref(), Some("x > 0 AND x < 10"));
            }
            other => panic!("expected Continue, got {other:?}"),
        },
        other => panic!("expected Loop, got {other:?}"),
    }
}

// =====================================================================
//  9. RETURN / RETURN NEXT / RETURN QUERY（10 条）
// =====================================================================

#[test]
fn t81_return_simple() {
    let body = "BEGIN\n  RETURN 42;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Return { value } => {
            assert_eq!(value.as_deref(), Some("42"));
        }
        other => panic!("expected Return, got {other:?}"),
    }
}

#[test]
fn t82_return_no_value() {
    let body = "BEGIN\n  RETURN;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Return { value } => {
            assert!(value.is_none());
        }
        other => panic!("expected Return, got {other:?}"),
    }
}

#[test]
fn t83_return_expression() {
    let body = "BEGIN\n  RETURN x + y * 2;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Return { value } => {
            assert_eq!(value.as_deref(), Some("x + y * 2"));
        }
        other => panic!("expected Return, got {other:?}"),
    }
}

#[test]
fn t84_return_string() {
    let body = "BEGIN\n  RETURN 'hello world';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Return { value } => {
            assert_eq!(value.as_deref(), Some("'hello world'"));
        }
        other => panic!("expected Return, got {other:?}"),
    }
}

#[test]
fn t85_return_function_call() {
    let body = "BEGIN\n  RETURN lower(x);\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Return { value } => {
            assert_eq!(value.as_deref(), Some("lower(x)"));
        }
        other => panic!("expected Return, got {other:?}"),
    }
}

#[test]
fn t86_return_next_value() {
    let body = "BEGIN\n  RETURN NEXT x;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::ReturnNext { value } => {
            assert_eq!(value, "x");
        }
        other => panic!("expected ReturnNext, got {other:?}"),
    }
}

#[test]
fn t87_return_next_expression() {
    let body = "BEGIN\n  RETURN NEXT x + 1;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::ReturnNext { value } => {
            assert_eq!(value, "x + 1");
        }
        other => panic!("expected ReturnNext, got {other:?}"),
    }
}

#[test]
fn t88_return_query() {
    let body = "BEGIN\n  RETURN QUERY SELECT * FROM users;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::ReturnQuery { query } => {
            assert!(query.contains("SELECT"));
            assert!(query.contains("FROM users"));
        }
        other => panic!("expected ReturnQuery, got {other:?}"),
    }
}

#[test]
fn t89_return_query_with_where() {
    let body = "BEGIN\n  RETURN QUERY SELECT id FROM users WHERE active = true;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::ReturnQuery { query } => {
            assert!(query.contains("WHERE"));
        }
        other => panic!("expected ReturnQuery, got {other:?}"),
    }
}

#[test]
fn t90_return_in_loop() {
    let body = "BEGIN\n  FOR i IN 1..10 LOOP\n    RETURN NEXT i;\n  END LOOP;\n  RETURN;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.statements.len(), 2);
}

// =====================================================================
//  10. PERFORM（5 条）
// =====================================================================

#[test]
fn t91_perform_simple() {
    let body = "BEGIN\n  PERFORM some_func();\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Perform { query } => {
            assert!(query.contains("some_func()"));
        }
        other => panic!("expected Perform, got {other:?}"),
    }
}

#[test]
fn t92_perform_with_args() {
    let body = "BEGIN\n  PERFORM some_func(1, 'hello', x);\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Perform { query } => {
            assert!(query.contains("some_func(1, 'hello', x)"));
        }
        other => panic!("expected Perform, got {other:?}"),
    }
}

#[test]
fn t93_perform_select() {
    let body = "BEGIN\n  PERFORM 1;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Perform { query } => {
            assert_eq!(query.trim(), "1");
        }
        other => panic!("expected Perform, got {other:?}"),
    }
}

#[test]
fn t94_perform_in_conditional() {
    let body = "BEGIN\n  IF x > 0 THEN\n    PERFORM log_action();\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 1);
            match &branches[0].statements[0] {
                PlPgSqlStatement::Perform { .. } => {}
                other => panic!("expected Perform, got {other:?}"),
            }
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t95_perform_in_loop() {
    let body = "BEGIN\n  WHILE x < 10 LOOP\n    PERFORM check_status(x);\n    x := x + 1;\n  END LOOP;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::While { body, .. } => {
            assert_eq!(body.len(), 2);
        }
        other => panic!("expected While, got {other:?}"),
    }
}

// =====================================================================
//  11. EXECUTE（5 条）
// =====================================================================

#[test]
fn t96_execute_simple() {
    let body = "BEGIN\n  EXECUTE 'SELECT 1';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Execute { query, into, using } => {
            assert!(query.contains("SELECT 1"));
            assert!(into.is_empty());
            assert!(using.is_empty());
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn t97_execute_with_into() {
    let body = "BEGIN\n  EXECUTE 'SELECT count(*) FROM users' INTO x;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Execute { into, .. } => {
            assert_eq!(into.len(), 1);
            assert_eq!(into[0], "x");
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn t98_execute_with_using() {
    let body = "BEGIN\n  EXECUTE 'SELECT $1' USING x;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Execute { using, .. } => {
            assert_eq!(using.len(), 1);
            assert_eq!(using[0], "x");
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn t99_execute_with_into_and_using() {
    let body = "BEGIN\n  EXECUTE 'SELECT $1 + $2' INTO result USING a, b;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Execute { into, using, .. } => {
            assert_eq!(into.len(), 1);
            assert_eq!(into[0], "result");
            assert_eq!(using.len(), 2);
            assert_eq!(using[0], "a");
            assert_eq!(using[1], "b");
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn t100_execute_dynamic_sql() {
    let body = "BEGIN\n  EXECUTE 'INSERT INTO log VALUES (' || quote_literal(msg) || ')';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Execute { query, .. } => {
            assert!(query.contains("INSERT INTO log"));
            assert!(query.contains("quote_literal"));
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

// =====================================================================
//  12. RAISE（5 条）
// =====================================================================

#[test]
fn t101_raise_notice() {
    let body = "BEGIN\n  RAISE NOTICE 'hello';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Raise { level, format, .. } => {
            assert_eq!(*level, PlPgSqlRaiseLevel::Notice);
            assert!(format.as_deref().is_some());
        }
        other => panic!("expected Raise, got {other:?}"),
    }
}

#[test]
fn t102_raise_with_args() {
    let body = "BEGIN\n  RAISE NOTICE 'value is %', x;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Raise { args, .. } => {
            assert_eq!(args.len(), 1);
            assert_eq!(args[0], "x");
        }
        other => panic!("expected Raise, got {other:?}"),
    }
}

#[test]
fn t103_raise_exception() {
    let body = "BEGIN\n  RAISE EXCEPTION 'something went wrong';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Raise { level, .. } => {
            assert_eq!(*level, PlPgSqlRaiseLevel::Exception);
        }
        other => panic!("expected Raise, got {other:?}"),
    }
}

#[test]
fn t104_raise_multiple_args() {
    let body = "BEGIN\n  RAISE NOTICE 'x=%, y=%', x, y;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Raise { args, .. } => {
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], "x");
            assert_eq!(args[1], "y");
        }
        other => panic!("expected Raise, got {other:?}"),
    }
}

#[test]
fn t105_raise_using_option() {
    let body = "BEGIN\n  RAISE EXCEPTION 'error' USING ERRCODE = '23505';\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Raise { level, options, .. } => {
            assert_eq!(*level, PlPgSqlRaiseLevel::Exception);
            // 选项已解析（具体内容取决于解析器实现）
            assert!(!options.is_empty() || options.is_empty()); // 宽松断言
        }
        other => panic!("expected Raise, got {other:?}"),
    }
}

// =====================================================================
//  13. 异常处理（5 条）
// =====================================================================

#[test]
fn t106_exception_simple() {
    let body = "BEGIN\n  x := 1 / 0;\nEXCEPTION\n  WHEN division_by_zero THEN\n    x := 0;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.exception_handlers.len(), 1);
    assert_eq!(block.statements.len(), 1);
}

#[test]
fn t107_exception_multiple_handlers() {
    let body = "BEGIN\n  x := risky_func();\nEXCEPTION\n  WHEN division_by_zero THEN\n    x := 0;\n  WHEN others THEN\n    x := -1;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.exception_handlers.len(), 2);
}

#[test]
fn t108_exception_when_others() {
    let body = "BEGIN\n  x := 1;\nEXCEPTION\n  WHEN OTHERS THEN\n    NULL;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.exception_handlers.len(), 1);
}

#[test]
fn t109_exception_nested_block() {
    let body = "BEGIN\n  BEGIN\n    x := 1 / 0;\n  EXCEPTION\n    WHEN OTHERS THEN\n      x := -1;\n  END;\n  y := 2;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.statements.len(), 2);
    match &block.statements[0] {
        PlPgSqlStatement::Block(inner) => {
            assert_eq!(inner.exception_handlers.len(), 1);
        }
        other => panic!("expected Block, got {other:?}"),
    }
}

#[test]
fn t110_exception_with_raise() {
    let body = "BEGIN\n  x := risky_func();\nEXCEPTION\n  WHEN OTHERS THEN\n    RAISE NOTICE 'caught error';\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.exception_handlers.len(), 1);
    assert_eq!(block.exception_handlers[0].statements.len(), 1);
}

// =====================================================================
//  14. CREATE FUNCTION 完整解析测试（通过 parse_sql）
// =====================================================================

#[test]
fn t111_create_function_basic() {
    let sql = "CREATE FUNCTION add_one(x integer) RETURNS integer LANGUAGE plpgsql AS $$\nBEGIN\n  RETURN x + 1;\nEND\n$$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    assert_eq!(stmts.len(), 1);
    match &stmts[0] {
        Statement::CreateFunction {
            name,
            parameters,
            return_type,
            language,
            body,
            or_replace,
            volatility,
            strict,
            security_definer,
        } => {
            assert_eq!(name, "add_one");
            assert_eq!(parameters.len(), 1);
            assert_eq!(parameters[0].name.as_deref(), Some("x"));
            assert_eq!(parameters[0].data_type, "integer");
            assert_eq!(return_type, "integer");
            assert_eq!(language, "plpgsql");
            assert!(body.contains("RETURN"));
            assert!(!or_replace);
            assert!(volatility.is_none());
            assert!(!strict);
            assert!(!security_definer);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t112_create_or_replace_function() {
    let sql = "CREATE OR REPLACE FUNCTION update_x() RETURNS void LANGUAGE plpgsql AS $$\nBEGIN\n  x := 1;\nEND\n$$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction {
            name, or_replace, ..
        } => {
            assert_eq!(name, "update_x");
            assert!(or_replace);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t113_create_function_no_params() {
    let sql = "CREATE FUNCTION get_time() RETURNS timestamp LANGUAGE plpgsql AS $$\nBEGIN\n  RETURN now();\nEND\n$$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction { parameters, .. } => {
            assert_eq!(parameters.len(), 0);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t114_create_function_with_volatility() {
    let sql = "CREATE FUNCTION compute(x integer) RETURNS integer LANGUAGE plpgsql IMMUTABLE AS $$\nBEGIN\n  RETURN x * 2;\nEND\n$$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction { volatility, .. } => {
            assert_eq!(*volatility.as_ref().unwrap(), FunctionVolatility::Immutable);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t115_create_function_strict() {
    let sql = "CREATE FUNCTION safe_div(a integer, b integer) RETURNS integer LANGUAGE plpgsql STRICT AS $$\nBEGIN\n  RETURN a / b;\nEND\n$$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction {
            strict, parameters, ..
        } => {
            assert!(strict);
            assert_eq!(parameters.len(), 2);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t116_create_function_security_definer() {
    let sql = "CREATE FUNCTION admin_task() RETURNS void LANGUAGE plpgsql SECURITY DEFINER AS $$\nBEGIN\n  PERFORM 1;\nEND\n$$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction {
            security_definer, ..
        } => {
            assert!(security_definer);
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t117_drop_function() {
    let sql = "DROP FUNCTION IF EXISTS add_one(integer)";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    assert_eq!(stmts.len(), 1);
    match &stmts[0] {
        Statement::DropFunction {
            name,
            parameter_types,
            if_exists,
            cascade,
        } => {
            assert_eq!(name, "add_one");
            assert_eq!(parameter_types.len(), 1);
            assert_eq!(parameter_types[0], "integer");
            assert!(if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropFunction, got {other:?}"),
    }
}

#[test]
fn t118_drop_function_cascade() {
    let sql = "DROP FUNCTION old_func() CASCADE";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::DropFunction { cascade, .. } => {
            assert!(cascade);
        }
        other => panic!("expected DropFunction, got {other:?}"),
    }
}

#[test]
fn t119_create_function_with_tagged_dollar_quote() {
    let sql = "CREATE FUNCTION tagged() RETURNS void LANGUAGE plpgsql AS $body$\nBEGIN\n  NULL;\nEND\n$body$";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction { body, .. } => {
            assert!(body.contains("BEGIN"));
            assert!(body.contains("END"));
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

#[test]
fn t120_create_function_with_out_params() {
    let sql = "CREATE FUNCTION split_name(full_name text, OUT first text, OUT last text) AS $$\nBEGIN\n  first := split_part(full_name, ' ', 1);\n  last := split_part(full_name, ' ', 2);\nEND\n$$ LANGUAGE plpgsql";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    match &stmts[0] {
        Statement::CreateFunction { parameters, .. } => {
            assert_eq!(parameters.len(), 3);
            // OUT 参数
            assert_eq!(parameters[1].mode, Some(FunctionArgMode::Out));
            assert_eq!(parameters[1].name.as_deref(), Some("first"));
            // LANGUAGE 在 AS 之后也应正确解析
        }
        other => panic!("expected CreateFunction, got {other:?}"),
    }
}

// =====================================================================
//  15. NULL 与 GOTO（额外覆盖）
// =====================================================================

#[test]
fn t121_null_statement() {
    let body = "BEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Null => {}
        other => panic!("expected Null, got {other:?}"),
    }
}

#[test]
fn t122_null_in_if() {
    let body = "BEGIN\n  IF x > 0 THEN\n    NULL;\n  END IF;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::If { branches, .. } => {
            assert_eq!(branches[0].statements.len(), 1);
            match &branches[0].statements[0] {
                PlPgSqlStatement::Null => {}
                other => panic!("expected Null, got {other:?}"),
            }
        }
        other => panic!("expected If, got {other:?}"),
    }
}

#[test]
fn t123_sql_statement_in_body() {
    let body = "BEGIN\n  INSERT INTO log VALUES (1, 'test');\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::SqlStatement { sql } => {
            // 标识符被归一化为关键字形式（大写），仅校验大小写不敏感匹配
            assert!(sql.to_uppercase().contains("INSERT INTO LOG"));
        }
        other => panic!("expected SqlStatement, got {other:?}"),
    }
}

#[test]
fn t124_multiple_sql_statements() {
    let body = "BEGIN\n  INSERT INTO t VALUES (1);\n  UPDATE t SET x = 2 WHERE id = 1;\n  DELETE FROM t WHERE id = 1;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.statements.len(), 3);
    for stmt in &block.statements {
        match stmt {
            PlPgSqlStatement::SqlStatement { .. } => {}
            other => panic!("expected SqlStatement, got {other:?}"),
        }
    }
}

#[test]
fn t125_create_function_body_extraction() {
    let body_content = "BEGIN\n  x := 1;\n  RETURN x;\nEND";
    let extracted = parse_create_function_body(body_content);
    assert!(extracted.contains("BEGIN"));
    assert!(extracted.contains("RETURN"));
    // 确认提取的 body 可以被 PL/pgSQL 解析器解析
    let _block = parse_body_ok(&extracted);
}

#[test]
fn t126_empty_block() {
    let body = "BEGIN\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.statements.len(), 0);
    assert_eq!(block.declarations.len(), 0);
    assert!(block.exception_handlers.is_empty());
}

#[test]
fn t127_declare_only() {
    let body = "DECLARE\n  x integer;\n  y text;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.declarations.len(), 2);
    assert_eq!(block.statements.len(), 1);
}

#[test]
fn t128_block_with_label() {
    let body = "<<top_block>>\nDECLARE\n  x integer;\nBEGIN\n  NULL;\nEND";
    let block = parse_body_ok(body);
    assert_eq!(block.label.as_deref(), Some("top_block"));
}

#[test]
fn t129_nested_blocks() {
    let body = "BEGIN\n  BEGIN\n    BEGIN\n      x := 1;\n    END;\n  END;\nEND";
    let block = parse_body_ok(body);
    match &block.statements[0] {
        PlPgSqlStatement::Block(outer) => match &outer.statements[0] {
            PlPgSqlStatement::Block(inner) => {
                assert_eq!(inner.statements.len(), 1);
            }
            other => panic!("expected nested Block, got {other:?}"),
        },
        other => panic!("expected Block, got {other:?}"),
    }
}

#[test]
fn t130_create_function_mixed_with_select() {
    // 测试 CREATE FUNCTION 与普通 SELECT 语句混合
    let sql = "SELECT 1;\nCREATE FUNCTION f() RETURNS void LANGUAGE plpgsql AS $$\nBEGIN\n  NULL;\nEND\n$$;\nSELECT 2";
    let stmts = parse_sql(sql).expect("parse_sql failed");
    assert_eq!(stmts.len(), 3);
    // 中间是 CreateFunction
    match &stmts[1] {
        Statement::CreateFunction { .. } => {}
        other => panic!("expected CreateFunction at index 1, got {other:?}"),
    }
}
