// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{
    AiCallRecord, OrchestrationTrace, ReviewDecision, ReviewDecisionRecord, RollbackEvent,
    SessionId, Stage, StageRecord,
};

const TRACE_FILE: &str = "trace.jsonl";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum TraceEntry {
    StageStart {
        stage: Stage,
        timestamp: DateTime<Utc>,
    },
    StageEnd {
        stage: Stage,
        timestamp: DateTime<Utc>,
        duration_ms: u64,
        artifact_version: u32,
    },
    AiCall {
        record: AiCallRecord,
        timestamp: DateTime<Utc>,
    },
    ReviewDecision {
        stage: Stage,
        decision: ReviewDecision,
        timestamp: DateTime<Utc>,
    },
    Rollback {
        from_stage: Stage,
        to_stage: Stage,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TraceLine {
    timestamp: DateTime<Utc>,
    level: String,
    session_id: String,
    entry: TraceEntry,
}

pub struct Trace {
    store_root: PathBuf,
}

impl Trace {
    pub fn new(store_root: PathBuf) -> Self {
        Self { store_root }
    }

    pub fn from_config(config: &crate::model::SddConfig) -> Self {
        Self::new(config.store_root.clone())
    }

    pub fn trace_path(&self, session_id: &SessionId) -> PathBuf {
        self.store_root.join(&session_id.value).join(TRACE_FILE)
    }

    async fn append(&self, session_id: &SessionId, entry: TraceEntry) -> SddResult<()> {
        let path = self.trace_path(session_id);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let line = TraceLine {
            timestamp: Utc::now(),
            level: "INFO".to_string(),
            session_id: session_id.value.clone(),
            entry,
        };

        let mut json = serde_json::to_string(&line)?;
        json.push('\n');

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(json.as_bytes()).await?;
        file.flush().await?;

        tracing::info!(
            session_id = %session_id,
            "trace entry appended"
        );

        Ok(())
    }

    pub async fn record_stage_start(&self, session_id: &SessionId, stage: Stage) -> SddResult<()> {
        self.append(
            session_id,
            TraceEntry::StageStart {
                stage,
                timestamp: Utc::now(),
            },
        )
        .await
    }

    pub async fn record_stage_end(
        &self,
        session_id: &SessionId,
        stage: Stage,
        duration_ms: u64,
        artifact_version: u32,
    ) -> SddResult<()> {
        self.append(
            session_id,
            TraceEntry::StageEnd {
                stage,
                timestamp: Utc::now(),
                duration_ms,
                artifact_version,
            },
        )
        .await
    }

    pub async fn record_ai_call(
        &self,
        session_id: &SessionId,
        record: AiCallRecord,
    ) -> SddResult<()> {
        self.append(
            session_id,
            TraceEntry::AiCall {
                record,
                timestamp: Utc::now(),
            },
        )
        .await
    }

    pub async fn record_review_decision(
        &self,
        session_id: &SessionId,
        stage: Stage,
        decision: ReviewDecision,
    ) -> SddResult<()> {
        self.append(
            session_id,
            TraceEntry::ReviewDecision {
                stage,
                decision,
                timestamp: Utc::now(),
            },
        )
        .await
    }

    pub async fn record_rollback(
        &self,
        session_id: &SessionId,
        from_stage: Stage,
        to_stage: Stage,
    ) -> SddResult<()> {
        self.append(
            session_id,
            TraceEntry::Rollback {
                from_stage,
                to_stage,
                timestamp: Utc::now(),
            },
        )
        .await
    }

    pub async fn query_trace(&self, session_id: &SessionId) -> SddResult<OrchestrationTrace> {
        let path = self.trace_path(session_id);

        let content = fs::read_to_string(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SddError::new(
                    SddErrorCode::SddSessionNotFound,
                    format!("trace for session {} not found", session_id),
                )
            } else {
                SddError::from(e)
            }
        })?;

        let mut trace = OrchestrationTrace::new(session_id.clone());
        let mut total_duration_ms = 0u64;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let trace_line: TraceLine = serde_json::from_str(line).map_err(|e| {
                SddError::new(
                    SddErrorCode::SddSessionCorrupted,
                    format!("trace line corrupted: {e}"),
                )
            })?;

            match trace_line.entry {
                TraceEntry::StageStart { .. } => {}
                TraceEntry::StageEnd {
                    stage,
                    duration_ms,
                    artifact_version,
                    ..
                } => {
                    total_duration_ms += duration_ms;
                    trace.stage_records.push(StageRecord {
                        stage,
                        start_time: trace_line.timestamp,
                        end_time: trace_line.timestamp,
                        duration_ms,
                        artifact_version: crate::model::ArtifactVersion {
                            value: artifact_version,
                            parent: if artifact_version > 1 {
                                Some(artifact_version - 1)
                            } else {
                                None
                            },
                        },
                    });
                }
                TraceEntry::AiCall { record, .. } => {
                    trace.ai_call_records.push(record);
                }
                TraceEntry::ReviewDecision {
                    stage,
                    decision,
                    timestamp,
                } => {
                    trace.review_decisions.push(ReviewDecisionRecord {
                        stage,
                        decision,
                        decided_at: timestamp,
                    });
                }
                TraceEntry::Rollback {
                    from_stage,
                    to_stage,
                    timestamp,
                } => {
                    trace.rollback_history.push(RollbackEvent {
                        from_stage,
                        to_stage,
                        rolled_back_at: timestamp,
                    });
                }
            }
        }

        trace.total_duration_ms = total_duration_ms;
        Ok(trace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AiCallRecord, TokenUsage};

    fn test_store_root() -> PathBuf {
        std::env::temp_dir().join(format!("sdd-trace-test-{}", uuid::Uuid::new_v4()))
    }

    async fn setup_trace() -> (Trace, SessionId) {
        let root = test_store_root();
        fs::create_dir_all(&root).await.unwrap();
        let trace = Trace::new(root);
        let session_id = SessionId::new();
        fs::create_dir_all(trace.store_root.join(&session_id.value))
            .await
            .unwrap();
        (trace, session_id)
    }

    fn sample_ai_call() -> AiCallRecord {
        AiCallRecord {
            provider: "claude".to_string(),
            model: "claude-3".to_string(),
            prompt_summary: "generate spec".to_string(),
            token_usage: TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 200,
                total_tokens: 300,
            },
            duration_ms: 5000,
            retry_count: 0,
            failure_reason: None,
        }
    }

    #[tokio::test]
    async fn trace_append_query() {
        let (trace, session_id) = setup_trace().await;

        trace
            .record_stage_start(&session_id, Stage::Spec)
            .await
            .unwrap();
        trace
            .record_stage_end(&session_id, Stage::Spec, 5000, 1)
            .await
            .unwrap();
        trace
            .record_ai_call(&session_id, sample_ai_call())
            .await
            .unwrap();
        trace
            .record_review_decision(&session_id, Stage::Spec, ReviewDecision::Pass)
            .await
            .unwrap();

        let result = trace.query_trace(&session_id).await.unwrap();
        assert_eq!(result.session_id, session_id);
        assert_eq!(result.stage_records.len(), 1);
        assert_eq!(result.ai_call_records.len(), 1);
        assert_eq!(result.review_decisions.len(), 1);
        assert_eq!(result.total_duration_ms, 5000);
    }

    #[tokio::test]
    async fn trace_rollback() {
        let (trace, session_id) = setup_trace().await;

        trace
            .record_rollback(&session_id, Stage::Design, Stage::Spec)
            .await
            .unwrap();

        let result = trace.query_trace(&session_id).await.unwrap();
        assert_eq!(result.rollback_history.len(), 1);
        assert_eq!(result.rollback_history[0].from_stage, Stage::Design);
        assert_eq!(result.rollback_history[0].to_stage, Stage::Spec);
    }

    #[tokio::test]
    async fn trace_multiple_stages() {
        let (trace, session_id) = setup_trace().await;

        for (stage, duration) in [
            (Stage::Spec, 1000u64),
            (Stage::Design, 2000),
            (Stage::Task, 3000),
            (Stage::Coding, 4000),
        ] {
            trace.record_stage_start(&session_id, stage).await.unwrap();
            trace
                .record_stage_end(&session_id, stage, duration, 1)
                .await
                .unwrap();
        }

        let result = trace.query_trace(&session_id).await.unwrap();
        assert_eq!(result.stage_records.len(), 4);
        assert_eq!(result.total_duration_ms, 10000);
    }

    #[tokio::test]
    async fn trace_not_found() {
        let (trace, _) = setup_trace().await;
        let fake_id = SessionId::new();

        let err = trace.query_trace(&fake_id).await.unwrap_err();
        assert!(err.is_session_not_found());
    }

    #[tokio::test]
    async fn trace_query_timeout() {
        let (trace, session_id) = setup_trace().await;

        for i in 0..100 {
            trace
                .record_stage_end(&session_id, Stage::Spec, i, 1)
                .await
                .unwrap();
        }

        let start = std::time::Instant::now();
        let result = trace.query_trace(&session_id).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(result.stage_records.len(), 100);
        assert!(
            elapsed.as_secs() < 2,
            "trace query took {:?}, expected < 2s",
            elapsed
        );
    }
}
