//! pgwire LISTEN/NOTIFY/UNLISTEN 跨会话通知中心 — Phase 4.6。
//!
//! # 设计
//!
//! PostgreSQL 的 LISTEN/NOTIFY 是异步通知机制：
//! - `LISTEN <channel>`：注册当前会话监听指定频道
//! - `UNLISTEN <channel>`：取消监听（`UNLISTEN *` 取消所有）
//! - `NOTIFY <channel> [, '<payload>']`：向指定频道发送通知，所有监听该频道的会话
//!   （含发送者自己）都会收到 `NotificationResponse` 消息
//!
//! 实现要点：
//! - 通知中心 `NotifyHub` 跨会话共享（`Arc<Mutex<NotifyHubInner>>`）
//! - 每个会话以 `pid`（BackendKeyData 中的 pid）为唯一标识
//! - NOTIFY 时将通知推入每个监听者的待发送队列
//! - server 层在每次 Query/Execute 处理后调用 `drain_pending` 取出待发送通知
//!   并编码为 `BackendMessage::NotificationResponse` 发送给客户端
//! - 会话断开时调用 `unregister` 清理订阅与待发送队列，避免内存泄漏
//!
//! # 并发安全
//!
//! `NotifyHub` 内部用 `parking_lot::Mutex` 保护（通知操作是非阻塞的，临界区极短）。
//! 所有方法都是同步的，可从 async 上下文中直接调用。

use std::collections::HashMap;
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;

// =====================================================================
//  Notification
// =====================================================================

/// 一条 NOTIFY 通知。
///
/// 对应 pgwire `NotificationResponse`（'A' 类型）消息的 payload。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// 发送方会话的 pid（即 NOTIFY 执行者的 backend pid）。
    pub notifier_pid: i32,
    /// 频道名。
    pub channel: String,
    /// 负载字符串（PG 中默认为空字符串）。
    pub payload: String,
}

// =====================================================================
//  NotifyHub
// =====================================================================

/// 跨会话通知中心内部状态。
#[derive(Debug, Default)]
struct NotifyHubInner {
    /// 每个会话监听的频道列表（pid → 频道集合）。
    ///
    /// 使用 `Vec<String>` 而非 `HashSet<String>` 以保留 LISTEN 顺序，
    /// 重复 LISTEN 同一频道会被去重。
    subscriptions: HashMap<i32, Vec<String>>,
    /// 每个会话待发送的通知队列（pid → 通知列表）。
    pending: HashMap<i32, Vec<Notification>>,
}

/// 跨会话通知中心。
///
/// 由 `PgwireServer` 持有一份 `Arc<NotifyHub>`，每个 `ExecutorService` 通过
/// `Arc` 共享同一实例，实现 NOTIFY 跨会话广播。
#[derive(Debug, Clone, Default)]
pub struct NotifyHub {
    inner: Arc<Mutex<NotifyHubInner>>,
}

impl NotifyHub {
    /// 创建空通知中心。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个新会话（pid）到通知中心。
    ///
    /// 必须在 LISTEN 之前调用。若 pid 已存在则覆盖（清空原有订阅）。
    pub fn register(&self, pid: i32) {
        let mut inner = self.inner.lock();
        inner.subscriptions.insert(pid, Vec::new());
        inner.pending.insert(pid, Vec::new());
    }

    /// 注销会话（pid），清理订阅与待发送队列。
    ///
    /// 必须在连接断开时调用以避免内存泄漏。
    pub fn unregister(&self, pid: i32) {
        let mut inner = self.inner.lock();
        inner.subscriptions.remove(&pid);
        inner.pending.remove(&pid);
    }

    /// `LISTEN <channel>` — 注册当前会话监听指定频道。
    ///
    /// PG 语义：重复 LISTEN 同一频道是幂等的（不会重复接收通知）。
    pub fn listen(&self, pid: i32, channel: &str) {
        let mut inner = self.inner.lock();
        let subs = inner.subscriptions.entry(pid).or_default();
        if !subs.iter().any(|c| c == channel) {
            subs.push(channel.to_string());
        }
    }

    /// `UNLISTEN <channel>` — 取消当前会话监听指定频道。
    ///
    /// PG 语义：未监听的频道执行 UNLISTEN 是幂等的（不报错）。
    pub fn unlisten(&self, pid: i32, channel: &str) {
        let mut inner = self.inner.lock();
        if let Some(subs) = inner.subscriptions.get_mut(&pid) {
            subs.retain(|c| c != channel);
        }
    }

    /// `UNLISTEN *` — 取消当前会话监听所有频道。
    pub fn unlisten_all(&self, pid: i32) {
        let mut inner = self.inner.lock();
        if let Some(subs) = inner.subscriptions.get_mut(&pid) {
            subs.clear();
        }
    }

    /// 返回当前会话监听的所有频道列表。
    pub fn listening_channels(&self, pid: i32) -> Vec<String> {
        let inner = self.inner.lock();
        inner.subscriptions.get(&pid).cloned().unwrap_or_default()
    }

    /// `NOTIFY <channel> [, '<payload>']` — 向所有监听该频道的会话推送通知。
    ///
    /// PG 语义：
    /// - 包括发送者自己（若其监听了该频道）
    /// - 推送顺序：按 pid 升序（保证确定性，便于测试）
    /// - 通知的 `notifier_pid` 字段为发送者的 pid
    ///
    /// 返回接收通知的会话数（用于 CommandComplete 标签生成）。
    pub fn notify(&self, channel: &str, payload: &str, notifier_pid: i32) -> usize {
        let mut inner = self.inner.lock();
        let mut delivered = 0usize;
        // 收集所有监听该频道的 pid（按升序确保确定性）
        let mut listeners: Vec<i32> = inner
            .subscriptions
            .iter()
            .filter(|(_, subs)| subs.iter().any(|c| c == channel))
            .map(|(pid, _)| *pid)
            .collect();
        listeners.sort_unstable();
        for pid in listeners {
            inner.pending.entry(pid).or_default().push(Notification {
                notifier_pid,
                channel: channel.to_string(),
                payload: payload.to_string(),
            });
            delivered += 1;
        }
        delivered
    }

    /// 取出当前会话所有待发送的通知（清空队列）。
    ///
    /// server 层在每次 Query/Execute 响应后调用此方法，将通知编码为
    /// `BackendMessage::NotificationResponse` 发送给客户端。
    pub fn drain_pending(&self, pid: i32) -> Vec<Notification> {
        let mut inner = self.inner.lock();
        inner
            .pending
            .get_mut(&pid)
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_listen_unlisten_basic() {
        let hub = NotifyHub::new();
        hub.register(1);
        assert!(hub.listening_channels(1).is_empty());

        hub.listen(1, "foo");
        hub.listen(1, "bar");
        assert_eq!(hub.listening_channels(1), vec!["foo", "bar"]);

        // 重复 LISTEN 应幂等
        hub.listen(1, "foo");
        assert_eq!(hub.listening_channels(1), vec!["foo", "bar"]);

        hub.unlisten(1, "foo");
        assert_eq!(hub.listening_channels(1), vec!["bar"]);

        // UNLISTEN 未监听的频道应幂等
        hub.unlisten(1, "nonexistent");
        assert_eq!(hub.listening_channels(1), vec!["bar"]);
    }

    #[test]
    fn test_unlisten_all() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.listen(1, "a");
        hub.listen(1, "b");
        hub.listen(1, "c");

        hub.unlisten_all(1);
        assert!(hub.listening_channels(1).is_empty());
    }

    #[test]
    fn test_notify_delivers_to_all_listeners_including_self() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.register(2);
        hub.register(3);
        hub.listen(1, "events");
        hub.listen(2, "events");
        // pid=3 未监听

        // pid=1 NOTIFY events
        let delivered = hub.notify("events", "hello", 1);
        assert_eq!(delivered, 2);

        // pid=1 和 pid=2 各收到一条
        let p1 = hub.drain_pending(1);
        let p2 = hub.drain_pending(2);
        let p3 = hub.drain_pending(3);

        assert_eq!(p1.len(), 1);
        assert_eq!(p1[0].notifier_pid, 1);
        assert_eq!(p1[0].channel, "events");
        assert_eq!(p1[0].payload, "hello");

        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].notifier_pid, 1);

        assert!(p3.is_empty());
    }

    #[test]
    fn test_notify_no_listeners_returns_zero() {
        let hub = NotifyHub::new();
        hub.register(1);
        // 无监听者
        let delivered = hub.notify("orphan", "payload", 1);
        assert_eq!(delivered, 0);
        assert!(hub.drain_pending(1).is_empty());
    }

    #[test]
    fn test_notify_with_empty_payload() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.listen(1, "ch");

        hub.notify("ch", "", 1);
        let p = hub.drain_pending(1);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].payload, "");
    }

    #[test]
    fn test_drain_pending_clears_queue() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.listen(1, "ch");

        hub.notify("ch", "first", 1);
        hub.notify("ch", "second", 1);

        let p1 = hub.drain_pending(1);
        assert_eq!(p1.len(), 2);

        let p2 = hub.drain_pending(1);
        assert!(p2.is_empty());
    }

    #[test]
    fn test_unregister_clears_state() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.listen(1, "ch");
        hub.notify("ch", "x", 1);
        assert!(!hub.drain_pending(1).is_empty());

        hub.unregister(1);
        // 注销后状态清空，再 LISTEN 也不会有遗留
        assert!(hub.listening_channels(1).is_empty());
        assert!(hub.drain_pending(1).is_empty());
    }

    #[test]
    fn test_unregister_does_not_affect_other_sessions() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.register(2);
        hub.listen(1, "ch");
        hub.listen(2, "ch");

        hub.unregister(1);
        // pid=2 仍应能收到通知
        let delivered = hub.notify("ch", "after", 2);
        assert_eq!(delivered, 1);

        let p = hub.drain_pending(2);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].notifier_pid, 2);
    }

    #[test]
    fn test_notify_delivery_order_deterministic() {
        let hub = NotifyHub::new();
        // 注册顺序故意打乱
        hub.register(30);
        hub.register(10);
        hub.register(20);
        hub.listen(30, "ch");
        hub.listen(10, "ch");
        hub.listen(20, "ch");

        // 每个会话应按 pid 升序收到通知
        hub.notify("ch", "x", 5);

        // 由于 drain_pending 是按 pid 隔离的，每条 pending 队列只有一条通知
        // 但 notify 内部的 listener 排序应保证确定性（不影响外部观察）
        assert_eq!(hub.drain_pending(10).len(), 1);
        assert_eq!(hub.drain_pending(20).len(), 1);
        assert_eq!(hub.drain_pending(30).len(), 1);
    }

    #[test]
    fn test_multiple_channels_isolated() {
        let hub = NotifyHub::new();
        hub.register(1);
        hub.listen(1, "a");
        hub.listen(1, "b");

        hub.notify("a", "msg-a", 1);
        hub.notify("b", "msg-b", 1);

        let pending = hub.drain_pending(1);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].channel, "a");
        assert_eq!(pending[0].payload, "msg-a");
        assert_eq!(pending[1].channel, "b");
        assert_eq!(pending[1].payload, "msg-b");
    }
}
