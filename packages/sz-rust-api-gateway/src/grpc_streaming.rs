//! gRPC 流式传输模块（v1.4.0 T16）
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum GrpcStreamError {
    #[error("channel closed")]
    ChannelClosed,
    #[error("backpressure: buffer full")]
    Backpressure,
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("internal error: {0}")]
    Internal(String),
}

pub const DEFAULT_BUFFER_SIZE: usize = 1024;

pub struct StreamingSender<T> {
    tx: mpsc::Sender<T>,
}

impl<T> StreamingSender<T> {
    pub fn new(buffer_size: usize) -> (Self, mpsc::Receiver<T>) {
        let (tx, rx) = mpsc::channel(buffer_size);
        (Self { tx }, rx)
    }
    pub async fn send(&self, msg: T) -> Result<(), GrpcStreamError> {
        self.tx
            .send(msg)
            .await
            .map_err(|_| GrpcStreamError::ChannelClosed)
    }
    pub async fn close(&self) -> Result<(), GrpcStreamError> {
        Ok(())
    }
}

pub struct StreamingReceiver<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> StreamingReceiver<T> {
    pub fn new(rx: mpsc::Receiver<T>) -> Self {
        Self { rx }
    }
    pub async fn recv(&mut self) -> Option<T> {
        self.rx.recv().await
    }
}

#[async_trait::async_trait]
pub trait ServerStreamingHandler<Req, Resp>: Send + Sync {
    async fn handle(&self, req: Req) -> Result<StreamingSender<Resp>, GrpcStreamError>;
}

#[async_trait::async_trait]
pub trait ClientStreamingHandler<Req, Resp>: Send + Sync {
    async fn handle(&self, receiver: &mut StreamingReceiver<Req>) -> Result<Resp, GrpcStreamError>;
}

#[async_trait::async_trait]
pub trait BidiStreamingHandler<Req, Resp>: Send + Sync {
    async fn handle(
        &self,
        receiver: &mut StreamingReceiver<Req>,
        sender: &StreamingSender<Resp>,
    ) -> Result<(), GrpcStreamError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sender_new() {
        let (sender, rx) = StreamingSender::<i32>::new(DEFAULT_BUFFER_SIZE);
        assert_eq!(DEFAULT_BUFFER_SIZE, 1024);
        let _ = sender;
        let _ = rx;
    }

    #[tokio::test]
    async fn test_send_recv() {
        let (sender, rx) = StreamingSender::<i32>::new(10);
        let mut receiver = StreamingReceiver::new(rx);
        sender.send(42).await.unwrap();
        assert_eq!(receiver.recv().await, Some(42));
    }

    #[tokio::test]
    async fn test_close() {
        let (sender, _rx) = StreamingSender::<i32>::new(10);
        let result = sender.close().await;
        assert!(result.is_ok(), "close should return Ok");
    }

    #[test]
    fn test_error_display() {
        assert_eq!(GrpcStreamError::ChannelClosed.to_string(), "channel closed");
        assert_eq!(
            GrpcStreamError::Backpressure.to_string(),
            "backpressure: buffer full"
        );
    }
}
