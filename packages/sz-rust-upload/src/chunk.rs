// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 分片上传 + 断点续传引擎
//!
//! 本模块提供与 [`crate::engine`] 正交的「分片上传」能力：
//!
//! - 分片校验：每个分片可选 sha256 校验（不匹配返回 [`UploadError::ChecksumFailed`]）
//! - 断点续传：已接收分片幂等去重，重复上传直接返回当前进度
//! - 磁盘持久化：分片落盘到 `<root>/<upload_id>/`，全部分片到达后按序组装
//!
//! # 安全约束（项目铁律）
//!
//! - 文件名校验：拒绝空名、路径分隔符、`..`、控制字符（防目录遍历，铁律 8）
//! - 全部 IO 使用 `tokio::fs`（禁 `std::fs`，铁律 4），外部 IO 包裹 5s 超时（铁律 5）
//! - 不持有锁跨 `.await`（铁律 6）：会话状态读取/更新均在锁内短临界区完成

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::error::UploadError;

/// 默认 IO 超时（铁律 5：外部 IO 必须包裹 timeout，默认 5s）
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// 一次分片上传的单个分片
#[derive(Debug, Clone)]
pub struct Chunk {
    /// 上传会话 ID（由 [`ChunkUploadStore::create_session`] 生成）
    pub upload_id: String,
    /// 分片序号（从 0 开始）
    pub chunk_index: u32,
    /// 客户端声明的总分片数（与会话记录不一致时拒绝）
    pub total_chunks: u32,
    /// 分片原始数据
    pub data: Vec<u8>,
    /// 分片 sha256（十六进制小写）。提供则校验，不提供则跳过。
    pub checksum: Option<String>,
}

/// 上传会话状态
#[derive(Debug, Clone)]
pub struct ChunkSession {
    /// 会话 ID
    pub upload_id: String,
    /// 原始文件名（已通过安全校验）
    pub file_name: String,
    /// 文件总大小（字节）
    pub total_size: u64,
    /// 总分片数
    pub total_chunks: u32,
    /// 每个分片的大小（字节）
    pub chunk_size: u64,
    /// 各分片是否已接收（索引 = chunk_index）
    pub received: Vec<bool>,
}

impl ChunkSession {
    /// 已接收分片数
    pub fn received_count(&self) -> usize {
        self.received.iter().filter(|b| **b).count()
    }

    /// 是否全部分片已到达（可组装）
    pub fn is_complete(&self) -> bool {
        self.received.iter().all(|b| *b)
    }
}

/// 上传进度（幂等返回给客户端）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkProgress {
    /// 会话 ID
    pub upload_id: String,
    /// 已接收分片数
    pub received: u32,
    /// 总分片数
    pub total_chunks: u32,
    /// 是否已完整（可组装）
    pub complete: bool,
}

/// 分片上传存储引擎
///
/// 所有分片持久化在 `root/<upload_id>/` 目录下，最终文件组装为
/// `root/<upload_id>/<file_name>`。
pub struct ChunkUploadStore {
    root: PathBuf,
    sessions: Mutex<HashMap<String, ChunkSession>>,
}

impl ChunkUploadStore {
    /// 创建分片存储引擎
    ///
    /// `root` 为分片持久化根目录（调用方需确保其可写）。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 生成上传会话 ID：文件名 + 纳秒时间戳的 sha256（不引入额外依赖）
    fn generate_upload_id(&self, file_name: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(file_name.as_bytes());
        hasher.update(nanos.to_le_bytes());
        sha256_hex_of(&hasher.finalize())
    }

    /// 校验文件名：拒绝空名、路径分隔符、`..`、控制字符（铁律 8 防目录遍历）
    fn validate_file_name(file_name: &str) -> Result<(), UploadError> {
        if file_name.is_empty() {
            return Err(UploadError::InvalidParam("文件名不能为空".to_string()));
        }
        if file_name.contains('/') || file_name.contains('\\') || file_name.contains("..") {
            return Err(UploadError::InvalidParam(format!(
                "非法文件名: {file_name}"
            )));
        }
        if file_name.chars().any(|c| c.is_control()) {
            return Err(UploadError::InvalidParam("文件名包含控制字符".to_string()));
        }
        Ok(())
    }

    /// 分片目录：`root/<upload_id>`
    fn chunk_dir(&self, upload_id: &str) -> PathBuf {
        self.root.join(upload_id)
    }

    /// 创建上传会话
    ///
    /// 返回会话（含 `upload_id`）；客户端需在后续分片请求中携带 `upload_id`。
    pub async fn create_session(
        &self,
        file_name: &str,
        total_size: u64,
        total_chunks: u32,
        chunk_size: u64,
    ) -> Result<ChunkSession, UploadError> {
        Self::validate_file_name(file_name)?;
        if total_chunks == 0 {
            return Err(UploadError::InvalidParam(
                "total_chunks 必须大于 0".to_string(),
            ));
        }
        if chunk_size == 0 {
            return Err(UploadError::InvalidParam(
                "chunk_size 必须大于 0".to_string(),
            ));
        }

        let upload_id = self.generate_upload_id(file_name);
        let session = ChunkSession {
            upload_id: upload_id.clone(),
            file_name: file_name.to_string(),
            total_size,
            total_chunks,
            chunk_size,
            received: vec![false; total_chunks as usize],
        };

        let dir = self.chunk_dir(&upload_id);
        timeout_io(fs::create_dir_all(&dir)).await?;

        self.sessions
            .lock()
            .await
            .insert(upload_id.clone(), session.clone());
        Ok(session)
    }

    /// 查询会话状态
    pub async fn get_session(&self, upload_id: &str) -> Option<ChunkSession> {
        self.sessions.lock().await.get(upload_id).cloned()
    }

    /// 上传一个分片（幂等）
    ///
    /// 若分片已接收，直接返回当前进度（断点续传去重，客户端可安全重试）。
    pub async fn upload_chunk(&self, chunk: &Chunk) -> Result<ChunkProgress, UploadError> {
        // 1. 读取会话（不持锁跨 await）
        let session = self.sessions.lock().await.get(&chunk.upload_id).cloned();
        let Some(session) = session else {
            return Err(UploadError::InvalidParam(format!(
                "上传会话不存在: {}",
                chunk.upload_id
            )));
        };

        if chunk.chunk_index >= session.total_chunks {
            return Err(UploadError::InvalidParam(format!(
                "分片序号越界: {} >= {}",
                chunk.chunk_index, session.total_chunks
            )));
        }
        if chunk.total_chunks != session.total_chunks {
            return Err(UploadError::InvalidParam(format!(
                "总分片数不一致: 客户端声明 {}，会话记录 {}",
                chunk.total_chunks, session.total_chunks
            )));
        }

        // 已接收则幂等返回（断点续传）
        if session.received[chunk.chunk_index as usize] {
            return Ok(Self::progress(&session));
        }

        // 2. 可选 sha256 校验
        if let Some(expected) = &chunk.checksum {
            let actual = sha256_hex(&chunk.data);
            if &actual != expected {
                return Err(UploadError::ChecksumFailed(format!(
                    "分片 {} 校验失败: expected={expected}, actual={actual}",
                    chunk.chunk_index
                )));
            }
        }

        // 3. 落盘（tokio::fs + 超时）
        let part_path = self
            .chunk_dir(&chunk.upload_id)
            .join(format!("{}.part", chunk.chunk_index));
        timeout_io(fs::write(&part_path, &chunk.data)).await?;

        // 4. 标记已接收（重新加锁）
        {
            let mut guard = self.sessions.lock().await;
            if let Some(s) = guard.get_mut(&chunk.upload_id) {
                s.received[chunk.chunk_index as usize] = true;
            }
        }

        let updated = self.sessions.lock().await.get(&chunk.upload_id).cloned();
        let updated = updated.unwrap_or(session);
        Ok(Self::progress(&updated))
    }

    /// 组装最终文件
    ///
    /// 全部分片到达后按序拼接，写入 `root/<upload_id>/<file_name>`。
    /// 返回最终文件路径；组装后校验文件总大小。
    pub async fn assemble(&self, upload_id: &str) -> Result<PathBuf, UploadError> {
        let session = self.sessions.lock().await.get(upload_id).cloned();
        let Some(session) = session else {
            return Err(UploadError::InvalidParam(format!(
                "上传会话不存在: {upload_id}"
            )));
        };
        if !session.is_complete() {
            return Err(UploadError::IncompleteChunks);
        }

        let dir = self.chunk_dir(upload_id);
        let final_path = dir.join(&session.file_name);

        // 创建/清空最终文件，按序拼接分片
        let mut file = timeout_io(fs::File::create(&final_path)).await?;
        for idx in 0..session.total_chunks {
            let part_path = dir.join(format!("{idx}.part"));
            let data = timeout_io(fs::read(&part_path)).await?;
            timeout_io(file.write_all(&data)).await?;
        }
        timeout_io(file.flush()).await?;
        drop(file);

        // 校验组装后大小与声明一致
        let meta = timeout_io(fs::metadata(&final_path)).await?;
        if meta.len() != session.total_size {
            return Err(UploadError::ChecksumFailed(format!(
                "组装大小不符: expected={}, actual={}",
                session.total_size,
                meta.len()
            )));
        }

        Ok(final_path)
    }

    /// 中止上传会话并清理分片目录
    pub async fn abort(&self, upload_id: &str) -> Result<(), UploadError> {
        self.sessions.lock().await.remove(upload_id);
        let dir = self.chunk_dir(upload_id);
        timeout_io(fs::remove_dir_all(&dir)).await?;
        Ok(())
    }

    /// 计算进度
    fn progress(session: &ChunkSession) -> ChunkProgress {
        ChunkProgress {
            upload_id: session.upload_id.clone(),
            received: session.received_count() as u32,
            total_chunks: session.total_chunks,
            complete: session.is_complete(),
        }
    }
}

/// 计算字节切片的 sha256（十六进制小写）
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    sha256_hex_of(&digest)
}

/// 将 sha256 摘要格式化为十六进制小写
fn sha256_hex_of(digest: &[u8]) -> String {
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 包裹 5s 超时的 IO 操作（铁律 5）
async fn timeout_io<T>(
    future: impl std::future::Future<Output = std::io::Result<T>>,
) -> Result<T, UploadError> {
    tokio::time::timeout(IO_TIMEOUT, future)
        .await
        .map_err(|_| UploadError::Internal("IO 操作超时".to_string()))?
        .map_err(|e| UploadError::Internal(format!("IO 错误: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时钟应晚于 UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir().join(format!("sz-rust-upload-test-{tag}-{nanos}"))
    }

    async fn cleanup(root: &Path) {
        let _ = fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn test_create_session_rejects_path_traversal() {
        let root = temp_root("traversal");
        let store = ChunkUploadStore::new(root.clone());
        for bad in ["../evil.txt", "a/b", "a\\b", "..", "bad\x00name"] {
            let err = store
                .create_session(bad, 10, 1, 10)
                .await
                .expect_err("非法文件名必须被拒绝");
            assert!(
                matches!(err, UploadError::InvalidParam(_)),
                "期望 InvalidParam，实际 {err:?}"
            );
        }
        cleanup(&root).await;
    }

    #[tokio::test]
    async fn test_chunk_upload_and_assemble_out_of_order() {
        let root = temp_root("ok");
        let store = ChunkUploadStore::new(root.clone());

        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let total_chunks = 4;
        let chunk_size = 250;
        let cs = chunk_size as usize;

        let session = store
            .create_session("photo.bin", data.len() as u64, total_chunks, chunk_size)
            .await
            .expect("创建会话应成功");

        // 乱序上传分片（模拟网络乱序到达）
        for idx in [2u32, 0, 3, 1] {
            let start = (idx as usize * cs).min(data.len());
            let end = ((idx + 1) as usize * cs).min(data.len());
            let part = data[start..end].to_vec();
            let progress = store
                .upload_chunk(&Chunk {
                    upload_id: session.upload_id.clone(),
                    chunk_index: idx,
                    total_chunks,
                    data: part.clone(),
                    checksum: Some(sha256_hex(&part)),
                })
                .await
                .expect("上传分片应成功");
            assert_eq!(progress.total_chunks, total_chunks);
        }

        // 幂等去重：重复上传分片 2，仍返回进度且不报错
        let part2 = data[(2 * cs)..(3 * cs)].to_vec();
        let progress = store
            .upload_chunk(&Chunk {
                upload_id: session.upload_id.clone(),
                chunk_index: 2,
                total_chunks,
                data: part2.clone(),
                checksum: Some(sha256_hex(&part2)),
            })
            .await
            .expect("重复上传分片应幂等成功");
        assert!(progress.complete, "全部分片到达后应 complete");

        // 组装并校验内容
        let final_path = store
            .assemble(&session.upload_id)
            .await
            .expect("组装应成功");
        let assembled = fs::read(&final_path).await.expect("读取组装文件应成功");
        assert_eq!(assembled, data, "组装内容应与原始数据一致");

        cleanup(&root).await;
    }

    #[tokio::test]
    async fn test_checksum_mismatch_rejected() {
        let root = temp_root("checksum");
        let store = ChunkUploadStore::new(root.clone());

        let session = store
            .create_session("sum.bin", 5, 1, 5)
            .await
            .expect("创建会话应成功");

        let err = store
            .upload_chunk(&Chunk {
                upload_id: session.upload_id.clone(),
                chunk_index: 0,
                total_chunks: 1,
                data: b"hello".to_vec(),
                checksum: Some("deadbeef".to_string()),
            })
            .await
            .expect_err("校验和不匹配必须被拒绝");
        assert!(matches!(err, UploadError::ChecksumFailed(_)));

        cleanup(&root).await;
    }

    #[tokio::test]
    async fn test_assemble_incomplete_rejected() {
        let root = temp_root("incomplete");
        let store = ChunkUploadStore::new(root.clone());

        let session = store
            .create_session("partial.bin", 10, 2, 5)
            .await
            .expect("创建会话应成功");

        let err = store
            .assemble(&session.upload_id)
            .await
            .expect_err("分片不完整必须拒绝组装");
        assert!(matches!(err, UploadError::IncompleteChunks));

        cleanup(&root).await;
    }
}
