# sz-rust-upload

SZ-Rust 文件上传/分片能力包（v1.7.0 新增，经根包 `api-upload` feature 门控，默认不启用）。

## 组成

| 模块 | 说明 |
|------|------|
| `sz_rust_upload::engine` | 标准上传引擎 re-export（来自 `sz-rust-infra-facade::upload`）：`File` / `UploadedFile` / 5 种存储引擎（Local / 阿里云 OSS / 腾讯云 COS / 七牛 Kodo / AWS S3）/ 校验 / 图像处理，等价于 `sz_rust_core::upload` |
| `sz_rust_upload::chunk` | 分片上传 + 断点续传引擎：sha256 分片校验、幂等去重、磁盘持久化、乱序到达后按序组装 |
| `sz_rust_upload::UploadError` | 本包错误类型（错误码 17150 起） |

## 示例（分片上传）

```rust,ignore
use sz_rust_upload::{Chunk, ChunkUploadStore};

let store = ChunkUploadStore::new("/data/uploads");
let session = store.create_session("video.mp4", 1_000_000, 4, 250_000).await?;

for (idx, part) in parts.iter().enumerate() {
    store.upload_chunk(&Chunk {
        upload_id: session.upload_id.clone(),
        chunk_index: idx as u32,
        total_chunks: session.total_chunks,
        data: part.clone(),
        checksum: Some(sz_rust_upload::sha256_hex(part)),
    }).await?;
}

let final_path = store.assemble(&session.upload_id).await?;
```

## 安全约束

- 文件名拒绝 `..` / 路径分隔符 / 控制字符（防目录遍历）
- 全部 IO 使用 `tokio::fs`，外部 IO 包裹 5s 超时
- 不持有锁跨 `.await`