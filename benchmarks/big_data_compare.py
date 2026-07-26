#!/usr/bin/env python3
"""
SzRSQL vs MySQL/PostgreSQL/Oracle/SQLite 大数据量对比测试

测试维度：
1. INSERT 吞吐（rows/sec）
2. SELECT 全表扫描延迟
3. SELECT + WHERE 过滤延迟
4. SELECT + ORDER BY 延迟
5. SELECT COUNT(*) 延迟
6. UPDATE 吞吐
7. DELETE 吞吐

数据规模：1K / 10K / 100K 行

用法: python big_data_compare.py
"""

import os
import sys
import time
import sqlite3
import statistics

# 数据库连接配置（来自 服务器信息.md）
MYSQL_CONFIG = {
    "host": "127.0.0.1",
    "port": 3306,
    "user": "root",
    "password": "test123",
    "database": "sz_orm_test",
}

PG_CONFIG = {
    "host": "127.0.0.1",
    "port": 5432,
    "user": "postgres",
    "password": "test123",
    "database": "sz_orm_test",
}

ORACLE_CONFIG = {
    "user": "sys",
    "password": "test123",
    "dsn": "127.0.0.1:1521/freepdb1",
    "mode": "SYSDBA",
}

SQLITE_PATH = ":memory:"

SIZES = [1000, 10000, 100000]


def fmt_duration(secs):
    if secs < 0.001:
        return f"{secs*1000000:.2f} μs"
    elif secs < 1.0:
        return f"{secs*1000:.2f} ms"
    else:
        return f"{secs:.2f} s"


def fmt_throughput(rows, secs):
    if secs == 0:
        return "N/A"
    rps = rows / secs
    if rps >= 1000000:
        return f"{rps/1000000:.2f} M rows/s"
    elif rps >= 1000:
        return f"{rps/1000:.2f} K rows/s"
    else:
        return f"{rps:.2f} rows/s"


def generate_rows(n):
    """生成 n 行测试数据"""
    return [(i, f"user_{i}", 20 + (i % 50), f"user_{i}@example.com") for i in range(n)]


# ============================================================
# SQLite 测试
# ============================================================

def bench_sqlite(n):
    """测试 SQLite 性能"""
    conn = sqlite3.connect(SQLITE_PATH)
    cursor = conn.cursor()

    # 创建表
    cursor.execute("DROP TABLE IF EXISTS users")
    cursor.execute(
        "CREATE TABLE users (id INTEGER, name TEXT, age INTEGER, email TEXT)"
    )
    conn.commit()

    rows = generate_rows(n)

    # INSERT
    start = time.perf_counter()
    cursor.executemany("INSERT INTO users VALUES (?, ?, ?, ?)", rows)
    conn.commit()
    insert_t = time.perf_counter() - start

    # SELECT *
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users")
    result = cursor.fetchall()
    select_all_t = time.perf_counter() - start
    assert len(result) == n, f"SELECT * returned {len(result)} rows, expected {n}"

    # WHERE
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users WHERE age > 50")
    result = cursor.fetchall()
    where_t = time.perf_counter() - start
    expected = sum(1 for i in range(n) if 20 + (i % 50) > 50)
    assert len(result) == expected

    # ORDER BY
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users ORDER BY age DESC, id ASC")
    result = cursor.fetchall()
    order_t = time.perf_counter() - start
    assert len(result) == n

    # COUNT(*)
    start = time.perf_counter()
    cursor.execute("SELECT COUNT(*) FROM users")
    result = cursor.fetchone()
    count_t = time.perf_counter() - start
    assert result[0] == n

    # UPDATE
    start = time.perf_counter()
    cursor.execute("UPDATE users SET age = age + 1 WHERE age > 50")
    conn.commit()
    update_t = time.perf_counter() - start

    # DELETE
    start = time.perf_counter()
    cursor.execute("DELETE FROM users WHERE age > 50")
    conn.commit()
    delete_t = time.perf_counter() - start

    conn.close()
    return {
        "insert": insert_t,
        "select_all": select_all_t,
        "where": where_t,
        "order_by": order_t,
        "count": count_t,
        "update": update_t,
        "delete": delete_t,
    }


# ============================================================
# MySQL 测试
# ============================================================

def bench_mysql(n):
    """测试 MySQL 性能"""
    import pymysql
    conn = pymysql.connect(**MYSQL_CONFIG)
    cursor = conn.cursor()

    cursor.execute("DROP TABLE IF EXISTS users")
    cursor.execute(
        "CREATE TABLE users (id BIGINT, name VARCHAR(255), age BIGINT, email VARCHAR(255)) ENGINE=InnoDB"
    )
    conn.commit()

    rows = generate_rows(n)

    # INSERT
    start = time.perf_counter()
    cursor.executemany("INSERT INTO users VALUES (%s, %s, %s, %s)", rows)
    conn.commit()
    insert_t = time.perf_counter() - start

    # SELECT *
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users")
    result = cursor.fetchall()
    select_all_t = time.perf_counter() - start
    assert len(result) == n

    # WHERE
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users WHERE age > 50")
    result = cursor.fetchall()
    where_t = time.perf_counter() - start

    # ORDER BY
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users ORDER BY age DESC, id ASC")
    result = cursor.fetchall()
    order_t = time.perf_counter() - start

    # COUNT(*)
    start = time.perf_counter()
    cursor.execute("SELECT COUNT(*) FROM users")
    result = cursor.fetchone()
    count_t = time.perf_counter() - start
    assert result[0] == n

    # UPDATE
    start = time.perf_counter()
    cursor.execute("UPDATE users SET age = age + 1 WHERE age > 50")
    conn.commit()
    update_t = time.perf_counter() - start

    # DELETE
    start = time.perf_counter()
    cursor.execute("DELETE FROM users WHERE age > 50")
    conn.commit()
    delete_t = time.perf_counter() - start

    cursor.execute("DROP TABLE IF EXISTS users")
    conn.commit()
    conn.close()
    return {
        "insert": insert_t,
        "select_all": select_all_t,
        "where": where_t,
        "order_by": order_t,
        "count": count_t,
        "update": update_t,
        "delete": delete_t,
    }


# ============================================================
# PostgreSQL 测试
# ============================================================

def bench_pg(n):
    """测试 PostgreSQL 性能"""
    import psycopg2
    conn = psycopg2.connect(**PG_CONFIG)
    conn.autocommit = False
    cursor = conn.cursor()

    cursor.execute("DROP TABLE IF EXISTS users")
    cursor.execute(
        "CREATE TABLE users (id BIGINT, name TEXT, age BIGINT, email TEXT)"
    )
    conn.commit()

    rows = generate_rows(n)

    # INSERT
    start = time.perf_counter()
    cursor.executemany("INSERT INTO users VALUES (%s, %s, %s, %s)", rows)
    conn.commit()
    insert_t = time.perf_counter() - start

    # SELECT *
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users")
    result = cursor.fetchall()
    select_all_t = time.perf_counter() - start
    assert len(result) == n

    # WHERE
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users WHERE age > 50")
    result = cursor.fetchall()
    where_t = time.perf_counter() - start

    # ORDER BY
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users ORDER BY age DESC, id ASC")
    result = cursor.fetchall()
    order_t = time.perf_counter() - start

    # COUNT(*)
    start = time.perf_counter()
    cursor.execute("SELECT COUNT(*) FROM users")
    result = cursor.fetchone()
    count_t = time.perf_counter() - start
    assert result[0] == n

    # UPDATE
    start = time.perf_counter()
    cursor.execute("UPDATE users SET age = age + 1 WHERE age > 50")
    conn.commit()
    update_t = time.perf_counter() - start

    # DELETE
    start = time.perf_counter()
    cursor.execute("DELETE FROM users WHERE age > 50")
    conn.commit()
    delete_t = time.perf_counter() - start

    cursor.execute("DROP TABLE IF EXISTS users")
    conn.commit()
    conn.close()
    return {
        "insert": insert_t,
        "select_all": select_all_t,
        "where": where_t,
        "order_by": order_t,
        "count": count_t,
        "update": update_t,
        "delete": delete_t,
    }


# ============================================================
# Oracle 测试
# ============================================================

def bench_oracle(n):
    """测试 Oracle 性能"""
    import oracledb
    conn = oracledb.connect(**ORACLE_CONFIG)
    cursor = conn.cursor()

    cursor.execute("DROP TABLE users")
    cursor.execute(
        "CREATE TABLE users (id NUMBER, name VARCHAR2(255), age NUMBER, email VARCHAR2(255))"
    )
    conn.commit()

    rows = generate_rows(n)

    # INSERT (Oracle executemany)
    start = time.perf_counter()
    cursor.executemany("INSERT INTO users VALUES (:1, :2, :3, :4)", rows)
    conn.commit()
    insert_t = time.perf_counter() - start

    # SELECT *
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users")
    result = cursor.fetchall()
    select_all_t = time.perf_counter() - start
    assert len(result) == n

    # WHERE
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users WHERE age > 50")
    result = cursor.fetchall()
    where_t = time.perf_counter() - start

    # ORDER BY
    start = time.perf_counter()
    cursor.execute("SELECT * FROM users ORDER BY age DESC, id ASC")
    result = cursor.fetchall()
    order_t = time.perf_counter() - start

    # COUNT(*)
    start = time.perf_counter()
    cursor.execute("SELECT COUNT(*) FROM users")
    result = cursor.fetchone()
    count_t = time.perf_counter() - start
    assert result[0] == n

    # UPDATE
    start = time.perf_counter()
    cursor.execute("UPDATE users SET age = age + 1 WHERE age > 50")
    conn.commit()
    update_t = time.perf_counter() - start

    # DELETE
    start = time.perf_counter()
    cursor.execute("DELETE FROM users WHERE age > 50")
    conn.commit()
    delete_t = time.perf_counter() - start

    cursor.execute("DROP TABLE users")
    conn.commit()
    conn.close()
    return {
        "insert": insert_t,
        "select_all": select_all_t,
        "where": where_t,
        "order_by": order_t,
        "count": count_t,
        "update": update_t,
        "delete": delete_t,
    }


# SzRSQL 结果（从 Rust 基准测试手动填入）
SZRSQL_RESULTS = {
    1000: {
        "insert": 28.20e-6,
        "select_all": 472.60e-6,
        "where": 373.40e-6,
        "order_by": 601.60e-6,
        "count": 544.80e-6,
        "update": None,  # 未测试
        "delete": None,  # 未测试
    },
    10000: {
        "insert": 63.20e-6,
        "select_all": 4.24e-3,
        "where": 3.50e-3,
        "order_by": 7.18e-3,
        "count": 6.00e-3,
        "update": None,
        "delete": None,
    },
    100000: {
        "insert": 445.40e-6,
        "select_all": 43.20e-3,
        "where": 43.78e-3,
        "order_by": 85.60e-3,
        "count": 74.59e-3,
        "update": None,
        "delete": None,
    },
}


def main():
    print("=" * 100)
    print("SzRSQL vs MySQL/PostgreSQL/Oracle/SQLite 大数据量对比测试")
    print("=" * 100)
    print()

    # 测试 SQLite（本机内存）
    print("测试 SQLite (in-memory)...")
    sqlite_results = {}
    for size in SIZES:
        print(f"  SQLite {size} 行...", end="", flush=True)
        sqlite_results[size] = bench_sqlite(size)
        print(f" 完成 (INSERT: {fmt_duration(sqlite_results[size]['insert'])})")

    # 测试 MySQL
    print("\n测试 MySQL 9.6 (本机)...")
    mysql_results = {}
    for size in SIZES:
        print(f"  MySQL {size} 行...", end="", flush=True)
        try:
            mysql_results[size] = bench_mysql(size)
            print(f" 完成 (INSERT: {fmt_duration(mysql_results[size]['insert'])})")
        except Exception as e:
            print(f" 失败: {e}")
            mysql_results[size] = None

    # 测试 PostgreSQL
    print("\n测试 PostgreSQL 18 (本机)...")
    pg_results = {}
    for size in SIZES:
        print(f"  PostgreSQL {size} 行...", end="", flush=True)
        try:
            pg_results[size] = bench_pg(size)
            print(f" 完成 (INSERT: {fmt_duration(pg_results[size]['insert'])})")
        except Exception as e:
            print(f" 失败: {e}")
            pg_results[size] = None

    # 测试 Oracle
    print("\n测试 Oracle 23ai Free (本机)...")
    oracle_results = {}
    for size in SIZES:
        print(f"  Oracle {size} 行...", end="", flush=True)
        try:
            oracle_results[size] = bench_oracle(size)
            print(f" 完成 (INSERT: {fmt_duration(oracle_results[size]['insert'])})")
        except Exception as e:
            print(f" 失败: {e}")
            oracle_results[size] = None

    # 打印汇总表
    print()
    print("=" * 100)
    print("对比结果汇总")
    print("=" * 100)

    for op in ["insert", "select_all", "where", "order_by", "count", "update", "delete"]:
        op_name = {
            "insert": "INSERT (批量)",
            "select_all": "SELECT *",
            "where": "SELECT WHERE",
            "order_by": "SELECT ORDER BY",
            "count": "SELECT COUNT(*)",
            "update": "UPDATE",
            "delete": "DELETE",
        }[op]

        print(f"\n--- {op_name} ---")
        print(f"{'数据规模':<12} {'SzRSQL':<15} {'SQLite':<15} {'MySQL':<15} {'PostgreSQL':<15} {'Oracle':<15}")
        print("-" * 87)

        for size in SIZES:
            row = f"{size:<12}"
            # SzRSQL
            sz = SZRSQL_RESULTS.get(size, {}).get(op)
            row += f"{fmt_duration(sz) if sz else 'N/A':<15}"
            # SQLite
            sl = sqlite_results.get(size, {}).get(op)
            row += f"{fmt_duration(sl) if sl else 'N/A':<15}"
            # MySQL
            my = mysql_results.get(size, {}).get(op) if mysql_results.get(size) else None
            row += f"{fmt_duration(my) if my else 'N/A':<15}"
            # PostgreSQL
            pg = pg_results.get(size, {}).get(op) if pg_results.get(size) else None
            row += f"{fmt_duration(pg) if pg else 'N/A':<15}"
            # Oracle
            orc = oracle_results.get(size, {}).get(op) if oracle_results.get(size) else None
            row += f"{fmt_duration(orc) if orc else 'N/A':<15}"
            print(row)

            # 吞吐量
            tput = f"{'吞吐:':<12}"
            tput += f"{fmt_throughput(size, sz) if sz else 'N/A':<15}"
            tput += f"{fmt_throughput(size, sl) if sl else 'N/A':<15}"
            tput += f"{fmt_throughput(size, my) if my else 'N/A':<15}"
            tput += f"{fmt_throughput(size, pg) if pg else 'N/A':<15}"
            tput += f"{fmt_throughput(size, orc) if orc else 'N/A':<15}"
            print(tput)

    print()
    print("=" * 100)
    print("结论分析")
    print("=" * 100)
    print("""
1. SzRSQL 优势场景：
   - 内存数据库，无磁盘 I/O，INSERT 极快
   - 无网络开销，适合嵌入式场景

2. SzRSQL 劣势场景：
   - 无持久化（进程退出数据丢失）
   - 无索引优化（全表扫描）
   - 单线程执行（无并行查询）

3. 与成熟数据库差距：
   - SQLite: 内存模式性能接近，但 SQLite 支持持久化
   - MySQL/PostgreSQL: 有网络开销，但支持索引/持久化/并发
   - Oracle: 企业级功能完整，但开销最大
""")


if __name__ == "__main__":
    main()
