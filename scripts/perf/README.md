# 性能静态检测脚本集

## 脚本说明

| 脚本 | 检测内容 | 退出码 |
|------|---------|--------|
| `detect_n_plus_one.py` | 循环体内 DB 查询（N+1 问题） | 0=无违规, 1=有违规 |
| `detect_serial_await.py` | 串行 await 可并行化候选 | 0=始终 |
| `detect_lock_across_await.py` | 锁跨 .await 持有 | 0=无违规, 1=有违规 |
| `detect_blocking_call.py` | async 上下文中阻塞调用 | 0=无违规, 1=有违规 |

## 使用方法

```bash
python scripts/perf/detect_n_plus_one.py packages/
python scripts/perf/detect_serial_await.py packages/
python scripts/perf/detect_lock_across_await.py packages/
python scripts/perf/detect_blocking_call.py packages/
```