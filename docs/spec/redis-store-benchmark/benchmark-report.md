# Redis 存储后端压测报告

> **生成时间**: 2026-08-08T16:10:51.834Z
> **基线版本**: sz-rust v0.6.7
> **被测文件**: packages/sz-rust-auth-facade/src/redis_store.rs (646 行)
> **目标服务器**: 122.51.216.76 (Redis 127.0.0.1:6379)

## 1. 整体结论

❌ **阻断** — 存在 10 个阻断项，不可上生产。

## 2. 指标汇总表（PERF 红线对照）

| 红线编号 | 操作 | 并发 | 阈值QPS | 实测QPS | 阈值p99(ms) | 实测p99(ms) | 阈值错误率 | 实测错误率 | 判定 |
|----------|------|------|---------|---------|-------------|-------------|-----------|-----------|------|
| PERF-1 | increment_version | 100 | 8000 | N/A | 5 | N/A | 0.01% | N/A | ⚠️ |
| PERF-2 | increment_version | 500 | 20000 | N/A | 15 | N/A | 0.05% | N/A | ⚠️ |
| PERF-3 | increment_version | 1000 | 30000 | N/A | 30 | N/A | 0.10% | N/A | ⚠️ |
| PERF-4 | get_version | 1000 | 40000 | N/A | 10 | N/A | 0.01% | N/A | ⚠️ |
| PERF-5 | is_revoked | 1000 | 40000 | N/A | 10 | N/A | 0.01% | N/A | ⚠️ |
| PERF-6 | revoke | 500 | 15000 | N/A | 15 | N/A | 0.05% | N/A | ⚠️ |
| PERF-7 | register_session | 500 | 10000 | N/A | 20 | N/A | 0.05% | N/A | ⚠️ |
| PERF-8 | get_session | 1000 | 20000 | N/A | 15 | N/A | 0.05% | N/A | ⚠️ |
| PERF-9 | get_sessions | 500 | 5000 | N/A | 30 | N/A | 0.05% | N/A | ⚠️ |
| PERF-10 | revoke_session | 500 | 8000 | N/A | 20 | N/A | 0.05% | N/A | ⚠️ |
| PERF-11 | update_last_active | 500 | 8000 | N/A | 20 | N/A | 0.05% | N/A | ⚠️ |

## 3. 分并发度详细表

| 操作 | 并发 | QPS | p50(ms) | p95(ms) | p99(ms) | 错误率 | 耗时(s) | 判定 |
|------|------|-----|---------|---------|---------|--------|---------|------|
| increment_version | 10 | 158 | 62.75 | 63.69 | 107.00 | 0.000% | 6.32 | pass |
| get_version | 10 | 159 | 62.58 | 63.55 | 116.39 | 0.000% | 6.30 | fail |
| is_revoked | 10 | 158 | 62.63 | 64.35 | 120.37 | 0.000% | 6.33 | fail |
| revoke | 10 | 160 | 62.60 | 63.51 | 72.17 | 0.000% | 6.26 | fail |
| register_session | 10 | 434 | 22.41 | 22.87 | 71.31 | 0.000% | 2.30 | fail |
| get_session | 10 | 160 | 62.66 | 63.55 | 65.79 | 0.000% | 6.26 | fail |
| get_sessions | 10 | 0 | 0.00 | 0.00 | 0.00 | 100.000% | 0.00 | fail |
| revoke_session | 10 | 79 | 125.70 | 127.57 | 138.90 | 0.000% | 12.58 | fail |
| update_last_active | 10 | 219 | 44.26 | 55.64 | 84.46 | 0.000% | 4.57 | fail |
| ttl_validation | 1 | 0 | 0.00 | 0.00 | 0.00 | 0.000% | 0.00 | pass |
| concurrent_increment | 1000 | 30598 | 15.84 | 29.17 | 29.22 | 0.000% | 0.03 | pass |
| mixed | 50 | 1780 | 0.00 | 0.00 | 72.90 | 0.000% | 2.81 | fail |
| shared_pool | 500 | 85500 | 0.00 | 0.00 | 0.00 | 0.000% | 0.35 | pass |

## 4. file:line 证据表

| 操作 | 证据文件 | 证据行号 |
|------|----------|----------|
| increment_version | packages/sz-rust-auth-facade/src/redis_store.rs | 156-168 |
| get_version | packages/sz-rust-auth-facade/src/redis_store.rs | 142-154 |
| is_revoked | packages/sz-rust-auth-facade/src/redis_store.rs | 216-226 |
| revoke | packages/sz-rust-auth-facade/src/redis_store.rs | 199-214 |
| register_session | packages/sz-rust-auth-facade/src/redis_store.rs | 263-292 |
| get_session | packages/sz-rust-auth-facade/src/redis_store.rs | 314-338 |
| get_sessions |  |  |
| revoke_session | packages/sz-rust-auth-facade/src/redis_store.rs | 340-372 |
| update_last_active | packages/sz-rust-auth-facade/src/redis_store.rs | 374-409 |
| ttl_validation | packages/sz-rust-auth-facade/src/redis_store.rs | 199-214 |
| concurrent_increment | packages/sz-rust-auth-facade/src/redis_store.rs | 156-168 |
| mixed | packages/sz-rust-auth-facade/src/redis_store.rs | 156-168,142-154,216-226,199-214,263-292,314-338 |
| shared_pool | packages/sz-rust-auth-facade/src/redis_store.rs | 554-572 |

## 5. 资源占用

| 操作 | RSS峰值(KB) | RSS起始(KB) |
|------|-------------|-------------|
| increment_version | 28848 | 28848 |
| get_version | 27544 | 27544 |
| is_revoked | 28056 | 28056 |
| revoke | 27716 | 27716 |
| register_session | 27484 | 27484 |
| get_session | 27640 | 27640 |
| get_sessions | 0 | 0 |
| revoke_session | 26440 | 26440 |
| update_last_active | 26236 | 26236 |
| ttl_validation | 28628 | 28628 |
| concurrent_increment | 29308 | 29308 |
| mixed | 27680 | 27680 |
| shared_pool | 29976 | 29976 |

## 7. 清理确认

- ✅ bench-process: killed
- ✅ local-binary: deleted
- ✅ ssh-tunnel: closed
- ❌ redis-keys: Cannot read properties of null (reading 'execCommand')
- ❌ remote-scripts: Cannot read properties of null (reading 'execCommand')

## 8. 阻断项清单

| 操作 | 并发 | 红线编号 | 实测值 | 阈值 |
|------|------|----------|--------|------|
| get_version | 10 | PERF | qps=159,p99=116.39ms,err=0.000% | see config |
| is_revoked | 10 | PERF | qps=158,p99=120.37ms,err=0.000% | see config |
| revoke | 10 | PERF | qps=160,p99=72.17ms,err=0.000% | see config |
| register_session | 10 | PERF | qps=434,p99=71.31ms,err=0.000% | see config |
| get_session | 10 | PERF | qps=160,p99=65.79ms,err=0.000% | see config |
| get_sessions | 10 | PERF | qps=0,p99=0.00ms,err=100.000% | see config |
| revoke_session | 10 | PERF | qps=79,p99=138.90ms,err=0.000% | see config |
| update_last_active | 10 | PERF | qps=219,p99=84.46ms,err=0.000% | see config |
| mixed | 50 | PERF | qps=1780,p99=72.90ms,err=0.000% | see config |
| cleanup | 0 | CLEAN | 2 failures | 0 |

```json
{
  "roundResults": [
    {
      "operation": "increment_version",
      "concurrency": 10,
      "qps": 158.32747666803184,
      "latency_p50_ms": 62.7469,
      "latency_p95_ms": 63.6858,
      "latency_p99_ms": 106.9977,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 6.3160231,
      "rss_peak_kb": 28848,
      "rss_start_kb": 28848,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "156-168",
      "verdict": "pass",
      "consistency_check": {
        "passed": false,
        "detail": "final_version=1010 expected=1000"
      },
      "final_version": 1010
    },
    {
      "operation": "get_version",
      "concurrency": 10,
      "qps": 158.68475977602853,
      "latency_p50_ms": 62.5768,
      "latency_p95_ms": 63.5549,
      "latency_p99_ms": 116.3947,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 6.3018024,
      "rss_peak_kb": 27544,
      "rss_start_kb": 27544,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "142-154",
      "verdict": "fail",
      "consistency_check": {
        "passed": false,
        "detail": "all_42=false"
      }
    },
    {
      "operation": "is_revoked",
      "concurrency": 10,
      "qps": 157.9380392962161,
      "latency_p50_ms": 62.6272,
      "latency_p95_ms": 64.3451,
      "latency_p99_ms": 120.3702,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 6.3315969,
      "rss_peak_kb": 28056,
      "rss_start_kb": 28056,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "216-226",
      "verdict": "fail",
      "consistency_check": {
        "passed": true
      }
    },
    {
      "operation": "revoke",
      "concurrency": 10,
      "qps": 159.789619708375,
      "latency_p50_ms": 62.603,
      "latency_p95_ms": 63.5095,
      "latency_p99_ms": 72.1713,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 6.2582288,
      "rss_peak_kb": 27716,
      "rss_start_kb": 27716,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "199-214",
      "verdict": "fail",
      "consistency_check": {
        "passed": true,
        "detail": "jti_0_revoked=true"
      }
    },
    {
      "operation": "register_session",
      "concurrency": 10,
      "qps": 434.319265573071,
      "latency_p50_ms": 22.4067,
      "latency_p95_ms": 22.8743,
      "latency_p99_ms": 71.3114,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 2.3024537,
      "rss_peak_kb": 27484,
      "rss_start_kb": 27484,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "263-292",
      "verdict": "fail",
      "consistency_check": {
        "passed": true,
        "detail": "count=1000 expected=1000"
      }
    },
    {
      "operation": "get_session",
      "concurrency": 10,
      "qps": 159.82589717508205,
      "latency_p50_ms": 62.6575,
      "latency_p95_ms": 63.554,
      "latency_p99_ms": 65.7907,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 6.2568083,
      "rss_peak_kb": 27640,
      "rss_start_kb": 27640,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "314-338",
      "verdict": "fail",
      "consistency_check": {
        "passed": true
      }
    },
    {
      "operation": "get_sessions",
      "concurrency": 10,
      "qps": 0,
      "latency_p50_ms": 0,
      "latency_p95_ms": 0,
      "latency_p99_ms": 0,
      "error_rate": 1,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 0,
      "rss_peak_kb": 0,
      "rss_start_kb": 0,
      "evidence_file": "",
      "evidence_line": "",
      "verdict": "fail",
      "consistency_check": {
        "passed": false,
        "detail": "timeout"
      }
    },
    {
      "operation": "revoke_session",
      "concurrency": 10,
      "qps": 79.4728780167378,
      "latency_p50_ms": 125.7017,
      "latency_p95_ms": 127.5692,
      "latency_p99_ms": 138.8972,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 12.5829091,
      "rss_peak_kb": 26440,
      "rss_start_kb": 26440,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "340-372",
      "verdict": "fail",
      "consistency_check": {
        "passed": true,
        "detail": "remaining=0"
      }
    },
    {
      "operation": "update_last_active",
      "concurrency": 10,
      "qps": 218.91691251583234,
      "latency_p50_ms": 44.2574,
      "latency_p95_ms": 55.6383,
      "latency_p99_ms": 84.4571,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 4.5679431,
      "rss_peak_kb": 26236,
      "rss_start_kb": 26236,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "374-409",
      "verdict": "fail",
      "consistency_check": {
        "passed": true,
        "detail": "all_updated=true"
      }
    },
    {
      "operation": "ttl_validation",
      "concurrency": 1,
      "qps": 0,
      "latency_p50_ms": 0,
      "latency_p95_ms": 0,
      "latency_p99_ms": 0,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1,
      "duration_secs": 0,
      "rss_peak_kb": 28628,
      "rss_start_kb": 28628,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "199-214",
      "verdict": "pass",
      "consistency_check": {
        "passed": true,
        "detail": "expired=false zero=false"
      }
    },
    {
      "operation": "concurrent_increment",
      "concurrency": 1000,
      "qps": 30598.444375087973,
      "latency_p50_ms": 15.8367,
      "latency_p95_ms": 29.1657,
      "latency_p99_ms": 29.2248,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 1000,
      "duration_secs": 0.0326814,
      "rss_peak_kb": 29308,
      "rss_start_kb": 29308,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "156-168",
      "verdict": "pass",
      "consistency_check": {
        "passed": true,
        "detail": "no_lost=true cross=true v1=1100 v2=100"
      }
    },
    {
      "operation": "mixed",
      "concurrency": 50,
      "qps": 1780.1887705051488,
      "latency_p50_ms": 0,
      "latency_p95_ms": 0,
      "latency_p99_ms": 72.9043,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 5000,
      "duration_secs": 2.8086909,
      "rss_peak_kb": 27680,
      "rss_start_kb": 27680,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "156-168,142-154,216-226,199-214,263-292,314-338",
      "verdict": "fail",
      "consistency_check": {
        "passed": true,
        "detail": "final_version=1500 inc_count=1500"
      },
      "by_op": {
        "register_session": {
          "qps": 178.01887705051487,
          "latency_p99_ms": 73.0665,
          "error_rate": 0,
          "count": 500
        },
        "is_revoked": {
          "qps": 356.03775410102975,
          "latency_p99_ms": 72.9834,
          "error_rate": 0,
          "count": 1000
        },
        "get_version": {
          "qps": 356.03775410102975,
          "latency_p99_ms": 72.9125,
          "error_rate": 0,
          "count": 1000
        },
        "revoke": {
          "qps": 178.01887705051487,
          "latency_p99_ms": 72.7959,
          "error_rate": 0,
          "count": 500
        },
        "get_session": {
          "qps": 178.01887705051487,
          "latency_p99_ms": 72.9791,
          "error_rate": 0,
          "count": 500
        },
        "increment_version": {
          "qps": 534.0566311515446,
          "latency_p99_ms": 72.9012,
          "error_rate": 0,
          "count": 1500
        }
      },
      "final_version": 1500
    },
    {
      "operation": "shared_pool",
      "concurrency": 500,
      "qps": 85500.24196568476,
      "latency_p50_ms": 0,
      "latency_p95_ms": 0,
      "latency_p99_ms": 0,
      "error_rate": 0,
      "error_breakdown": {},
      "total_requests": 30000,
      "duration_secs": 0.3508762,
      "rss_peak_kb": 29976,
      "rss_start_kb": 29976,
      "evidence_file": "packages/sz-rust-auth-facade/src/redis_store.rs",
      "evidence_line": "554-572",
      "verdict": "pass",
      "consistency_check": {
        "passed": true,
        "detail": "dur=0.3508762s errors=0"
      }
    }
  ],
  "soakResult": null,
  "poolResult": null,
  "overallPassed": false,
  "blockers": [
    {
      "operation": "get_version",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=159,p99=116.39ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "is_revoked",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=158,p99=120.37ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "revoke",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=160,p99=72.17ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "register_session",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=434,p99=71.31ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "get_session",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=160,p99=65.79ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "get_sessions",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=0,p99=0.00ms,err=100.000%",
      "threshold": "see config"
    },
    {
      "operation": "revoke_session",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=79,p99=138.90ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "update_last_active",
      "concurrency": 10,
      "redLineId": "PERF",
      "actual": "qps=219,p99=84.46ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "mixed",
      "concurrency": 50,
      "redLineId": "PERF",
      "actual": "qps=1780,p99=72.90ms,err=0.000%",
      "threshold": "see config"
    },
    {
      "operation": "cleanup",
      "concurrency": 0,
      "redLineId": "CLEAN",
      "actual": "2 failures",
      "threshold": "0"
    }
  ],
  "timestamp": "2026-08-08T16:10:51.834Z"
}
```