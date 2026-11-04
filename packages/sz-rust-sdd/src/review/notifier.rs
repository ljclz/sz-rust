// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs;

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::ReviewRequest;
use crate::redactor::SddRedactor;

#[async_trait]
pub trait ReviewNotifier: Send + Sync + 'static {
    async fn notify(&self, request: &ReviewRequest) -> SddResult<()>;
}

pub struct CliReviewNotifier {
    store_root: PathBuf,
    redactor: SddRedactor,
    max_retries: u32,
}

impl CliReviewNotifier {
    pub fn new(store_root: PathBuf) -> Self {
        Self {
            store_root,
            redactor: SddRedactor::new(),
            max_retries: 3,
        }
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    async fn try_notify(&self, request: &ReviewRequest) -> SddResult<()> {
        let session_dir = self.store_root.join(&request.session_id.value);
        let queue_dir = session_dir.join("review_queue");
        fs::create_dir_all(&queue_dir).await?;

        let filename = format!(
            "{}_v{}_{}.json",
            request.stage,
            request.artifact_version.value,
            request.created_at.timestamp_millis()
        );
        let queue_path = queue_dir.join(&filename);

        let notification = self.build_notification(request);
        let redacted = self.redactor.redact_json_string(&notification);

        let tmp_path = queue_path.with_extension("json.tmp");
        fs::write(&tmp_path, &redacted).await?;
        fs::rename(&tmp_path, &queue_path).await?;

        tracing::info!(
            session_id = %request.session_id,
            stage = %request.stage,
            "review notification written to {:?}",
            queue_path
        );

        Ok(())
    }

    fn build_notification(&self, request: &ReviewRequest) -> String {
        let redacted_diff = self.redactor.redact_text(&request.diff_preview);
        format!(
            r#"{{"session_id":"{}","stage":"{}","artifact_version":{},"artifact_path":"{:?}","summary":{{"content_length":{},"section_titles_count":{}}},"diff_preview":"{}","created_at":"{}"}}"#,
            request.session_id.value,
            request.stage,
            request.artifact_version.value,
            request.artifact_path,
            request.summary.content_length,
            request.summary.section_titles.len(),
            redacted_diff.replace('"', "\\\""),
            request.created_at
        )
    }
}

#[async_trait]
impl ReviewNotifier for CliReviewNotifier {
    async fn notify(&self, request: &ReviewRequest) -> SddResult<()> {
        let mut last_error = None;

        for attempt in 1..=self.max_retries {
            match self.try_notify(request).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        max_retries = self.max_retries,
                        "review notification attempt failed: {e}"
                    );
                    last_error = Some(e);

                    if attempt < self.max_retries {
                        let delay_ms = 1000u64 * (1 << (attempt - 1));
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        Err(SddError::new(
            SddErrorCode::SddReviewNotifyFailed,
            format!(
                "review notification failed after {} retries: {:?}",
                self.max_retries, last_error
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactSummary, ArtifactVersion, SessionId, Stage};

    async fn setup_notifier() -> (CliReviewNotifier, SessionId) {
        let root = std::env::temp_dir().join(format!("sdd-notifier-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).await.unwrap();
        let notifier = CliReviewNotifier::new(root.clone());
        let session_id = SessionId::new();
        fs::create_dir_all(root.join(&session_id.value))
            .await
            .unwrap();
        (notifier, session_id)
    }

    fn sample_request(session_id: &SessionId) -> ReviewRequest {
        ReviewRequest::new(
            session_id.clone(),
            Stage::Spec,
            ArtifactVersion::initial(),
            PathBuf::from("/tmp/spec_v1.md"),
            ArtifactSummary {
                stage: Stage::Spec,
                version: ArtifactVersion::initial(),
                content_length: 100,
                section_titles: vec!["章节1".to_string()],
                task_count: None,
                code_change_lines: None,
            },
            "diff preview".to_string(),
        )
    }

    #[tokio::test]
    async fn review_notifier_cli() {
        let (notifier, session_id) = setup_notifier().await;
        let request = sample_request(&session_id);

        notifier.notify(&request).await.unwrap();

        let queue_dir = notifier
            .store_root
            .join(&session_id.value)
            .join("review_queue");
        let mut entries = fs::read_dir(&queue_dir).await.unwrap();
        let entry = entries.next_entry().await.unwrap().unwrap();
        assert!(entry.file_name().to_str().unwrap().ends_with(".json"));
    }

    #[tokio::test]
    async fn review_notifier_redacted() {
        let (notifier, session_id) = setup_notifier().await;
        let mut request = sample_request(&session_id);
        request.diff_preview = r#"api_key = "sk-secret1234567890abcdef""#.to_string();

        notifier.notify(&request).await.unwrap();

        let queue_dir = notifier
            .store_root
            .join(&session_id.value)
            .join("review_queue");
        let mut entries = fs::read_dir(&queue_dir).await.unwrap();
        let entry = entries.next_entry().await.unwrap().unwrap();
        let content = fs::read_to_string(entry.path()).await.unwrap();
        assert!(!content.contains("sk-secret1234567890abcdef"));
    }
}
