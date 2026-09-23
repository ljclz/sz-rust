# SZ-Rust Facade ↔ ThinkPHP API 对照表

> v1.3.0 P3 — 统一八门面静态 API 对照

## Cache 门面

| SZ-Rust `Cache::` | PHP `think\facade\Cache::` | 说明 |
|-------------------|---------------------------|------|
| `set(key, value, ttl)` | `set($name, $value, $ttl = null)` | 写入缓存 |
| `get(key)` | `get($name, $default = null)` | 读取缓存 |
| `delete(key)` | `delete($name)` | 删除缓存 |
| `has(key)` | `has($name)` | 判断存在 |
| `inc(key, step)` | `inc($name, $step = 1)` | 自增 |
| `dec(key, step)` | `dec($name, $step = 1)` | 自减 |
| `clear()` | `clear()` | 清空 |

## Db 门面

| SZ-Rust `Db::` | PHP `think\facade\Db::` | 说明 |
|----------------|------------------------|------|
| `init(pool)` | (自动初始化) | 初始化连接池 |
| `pool_ref()` | `getConnection()` | 获取连接池 |
| `is_initialized()` | — | 是否已初始化 |

## Event 门面

| SZ-Rust `Event::` | PHP `think\facade\Event::` | 说明 |
|-------------------|--------------------------|------|
| `listen(event, listener, first)` | `listen($event, $listener, $first = false)` | 注册监听 |
| `listen_with_priority(event, listener, priority)` | (扩展) | 优先级注册 |
| `listen_events(events)` | `listenEvents($events)` | 批量注册 |
| `has_listener(event)` | `hasListener($event)` | 是否有监听 |
| `remove(event)` | `remove($event)` | 移除监听 |
| `bind(events)` | `bind($events)` | 事件别名 |
| `subscribe(subscriber)` | `subscribe($subscriber)` | 注册订阅者 |
| `observe(observer, prefix)` | `observe($observer, $prefix = '')` | 注册观察者 |
| `trigger(event, params, once)` | `trigger($event, $params, $once = false)` | 触发事件 |
| `dispatch(event, params, mode)` | (扩展 async) | 分发事件 |
| `dispatch_sync(event, params)` | `trigger($event, $params)` | 同步分发 |
| `until(event, params)` | `until($event, $params)` | 只获取一个返回值 |

## Queue 门面

| SZ-Rust `Queue::` / `QueueFacade::` | PHP `think\facade\Queue::` | 说明 |
|-------------------------------------|--------------------------|------|
| `init(pool)` | (自动初始化) | 初始化队列 |
| `push(kind, payload, dedupe_key, delay)` | `push($job, $data, $queue = null, $delay = 0)` | 投递任务 |
| `work(handlers, config, shutdown)` | `work($queue = null)` | 启动 Worker |
| `retry_dead(job_id)` | (扩展) | 死信重放 |
| `snapshot()` | (扩展) | 队列快照 |
| `init_schema()` | (扩展) | 幂等建表 |

## Log 门面

| SZ-Rust `Log::` | PHP `think\facade\Log::` | 说明 |
|-----------------|------------------------|------|
| `log(level, msg)` | `log($level, $message)` | 记录日志 |
| `debug(msg)` | `debug($message)` | DEBUG 级别 |
| `info(msg)` | `info($message)` | INFO 级别 |
| `warn(msg)` | `warning($message)` | WARN 级别 |
| `error(msg)` | `error($message)` | ERROR 级别 |

## Config 门面

| SZ-Rust `Config::` | PHP `think\facade\Config::` | 说明 |
|--------------------|---------------------------|------|
| `init(config)` | (自动加载) | 初始化配置 |
| `instance()` | `get()` | 获取配置实例 |

## Request 门面

| SZ-Rust `Request::` | PHP `think\Request::` | 说明 |
|---------------------|----------------------|------|
| `fetch_query_data(req)` | `param()` | 获取查询参数 |
| `fetch_query_data_by_key(req, key)` | `param($name)` | 按键获取查询参数 |
| `parse_query(query)` | (内部) | 解析 query string |
| `url_decode(s)` | `urldecode($s)` | URL 解码 |

## Response 门面

| SZ-Rust `Response::` | PHP `think\Response::` | 说明 |
|----------------------|-----------------------|------|
| `success(data, msg)` | `renderSuccess($data, $msg)` | 成功响应 |
| `success_empty()` | `renderSuccess()` | 空成功响应 |
| `error(msg)` | `renderError($msg)` | 错误响应 |
| `error_with_data(msg, data)` | `renderError($msg, $data)` | 带数据错误响应 |
| `error_with_code(code, msg, data)` | `renderError($code, $msg, $data)` | 自定义码错误响应 |
| `render_json(code, msg, data)` | `json($data)` | 渲染 JSON Response |
| `render_success(data, msg)` | `renderSuccess($data, $msg)` | 渲染成功 JSON |
| `render_error(msg)` | `renderError($msg)` | 渲染错误 JSON |