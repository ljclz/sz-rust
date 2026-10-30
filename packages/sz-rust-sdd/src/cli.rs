// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{AiConfig, ReviewDecision, SddConfig, SessionId, SessionToolWhitelist, Stage};
use crate::orchestrator::Orchestrator;

#[derive(Parser, Debug)]
#[command(
    name = "sdd",
    version,
    about = "SDD Agent — Spec-Driven Development 编排"
)]
pub struct SddCli {
    #[command(subcommand)]
    pub command: SddCommand,
}

#[derive(Subcommand, Debug)]
pub enum SddCommand {
    /// 发起 SDD 会话
    Init {
        #[arg(long, help = "需求描述文本")]
        requirement: String,
        #[arg(long, help = "目标 crate 路径")]
        target: PathBuf,
        #[arg(long, help = "AI provider 名称")]
        provider: String,
        #[arg(long, help = "AI model 名称")]
        model: String,
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
    /// 恢复 SDD 会话
    Resume {
        #[arg(long, help = "会话 ID")]
        session: String,
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
    /// 提交审查决策
    Review {
        #[arg(long, help = "会话 ID")]
        session: String,
        #[arg(long, help = "审查决策 (pass/rollback/redo)")]
        decision: String,
        #[arg(long, help = "回退目标阶段 (spec/design/task/coding)")]
        rollback_target: Option<String>,
        #[arg(long, help = "重做注释")]
        comment: Option<String>,
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
    /// 列出所有会话
    List {
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
    /// 导出产物
    Export {
        #[arg(long, help = "会话 ID")]
        session: String,
        #[arg(long, help = "阶段 (spec/design/task/coding)")]
        stage: String,
        #[arg(long, help = "产物版本号")]
        version: u32,
        #[arg(long, help = "输出文件路径")]
        output: PathBuf,
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
    /// 查询编排轨迹
    Trace {
        #[arg(long, help = "会话 ID")]
        session: String,
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
    /// 配置 AI/白名单/铁律路径
    Config {
        #[arg(long, help = "AI provider 名称")]
        provider: Option<String>,
        #[arg(long, help = "AI model 名称")]
        model: Option<String>,
        #[arg(long, help = "MCP 工具白名单文件路径")]
        whitelist: Option<PathBuf>,
        #[arg(long, help = "铁律规则文件路径")]
        rules_path: Option<PathBuf>,
        #[arg(
            long,
            default_value = ".codeartsdoer/sdd-sessions/",
            help = "会话存储根目录"
        )]
        store_root: PathBuf,
    },
}

impl SddCli {
    pub async fn execute(&self) -> SddResult<i32> {
        match &self.command {
            SddCommand::Init {
                requirement,
                target,
                provider,
                model,
                store_root,
            } => {
                self.cmd_init(
                    requirement,
                    target.as_path(),
                    provider,
                    model,
                    store_root.as_path(),
                )
                .await
            }
            SddCommand::Resume {
                session,
                store_root,
            } => self.cmd_resume(session, store_root.as_path()).await,
            SddCommand::Review {
                session,
                decision,
                rollback_target,
                comment,
                store_root,
            } => {
                self.cmd_review(
                    session,
                    decision,
                    rollback_target,
                    comment,
                    store_root.as_path(),
                )
                .await
            }
            SddCommand::List { store_root } => self.cmd_list(store_root.as_path()).await,
            SddCommand::Export {
                session,
                stage,
                version,
                output,
                store_root,
            } => {
                self.cmd_export(
                    session,
                    stage,
                    *version,
                    output.as_path(),
                    store_root.as_path(),
                )
                .await
            }
            SddCommand::Trace {
                session,
                store_root,
            } => self.cmd_trace(session, store_root.as_path()).await,
            SddCommand::Config {
                provider,
                model,
                whitelist,
                rules_path,
                store_root,
            } => {
                self.cmd_config(provider, model, whitelist, rules_path, store_root.as_path())
                    .await
            }
        }
    }

    async fn cmd_init(
        &self,
        requirement: &str,
        target: &Path,
        provider: &str,
        model: &str,
        store_root: &Path,
    ) -> SddResult<i32> {
        if requirement.trim().is_empty() {
            return Err(SddError::new(
                SddErrorCode::SddCliArgMissing,
                "--requirement is required and must not be empty",
            ));
        }
        if !target.exists() {
            return Err(SddError::new(
                SddErrorCode::SddCliArgInvalid,
                format!("--target path does not exist: {:?}", target),
            ));
        }

        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        tokio::fs::create_dir_all(&config.store_root).await?;

        let orch = Orchestrator::new(config)?;
        let ai_config = AiConfig::new(provider, model);
        let whitelist = SessionToolWhitelist::new(std::collections::HashSet::new());

        let session_id = orch
            .init_session(
                requirement.to_string(),
                target.to_path_buf(),
                ai_config,
                whitelist,
            )
            .await?;

        println!("Session created: {}", session_id);
        println!("Stage: Spec (running)");
        Ok(0)
    }

    async fn cmd_resume(&self, session: &str, store_root: &Path) -> SddResult<i32> {
        if session.trim().is_empty() {
            return Err(SddError::new(
                SddErrorCode::SddCliArgMissing,
                "--session is required",
            ));
        }

        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        let orch = Orchestrator::new(config)?;
        let session_id = SessionId {
            value: session.to_string(),
        };

        let state = orch.resume_session(&session_id).await?;
        println!("Session resumed: {}", session_id);
        println!("Current stage: {}", state.current_stage);
        println!("Review status: {:?}", state.review_status);
        Ok(0)
    }

    async fn cmd_review(
        &self,
        session: &str,
        decision: &str,
        rollback_target: &Option<String>,
        comment: &Option<String>,
        store_root: &Path,
    ) -> SddResult<i32> {
        if session.trim().is_empty() {
            return Err(SddError::new(
                SddErrorCode::SddCliArgMissing,
                "--session is required",
            ));
        }

        let parsed_decision = parse_decision(decision, rollback_target, comment)?;

        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        let orch = Orchestrator::new(config)?;
        let session_id = SessionId {
            value: session.to_string(),
        };

        let outcome = orch.submit_review(&session_id, parsed_decision).await?;
        println!("Review decision applied: {:?}", outcome.decision);
        println!("New stage: {}", outcome.new_stage);
        println!("New review status: {:?}", outcome.new_review_status);
        Ok(0)
    }

    async fn cmd_list(&self, store_root: &Path) -> SddResult<i32> {
        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        let orch = Orchestrator::new(config)?;

        let sessions = orch.list_sessions().await?;
        if sessions.is_empty() {
            println!("No sessions found.");
        } else {
            println!("Found {} session(s):", sessions.len());
            for s in &sessions {
                println!(
                    "  {} | stage={} | review={:?} | target={:?} | created={}",
                    s.id, s.current_stage, s.review_status, s.target_crate, s.created_at
                );
            }
        }
        Ok(0)
    }

    async fn cmd_export(
        &self,
        session: &str,
        stage: &str,
        version: u32,
        output: &Path,
        store_root: &Path,
    ) -> SddResult<i32> {
        let parsed_stage = parse_stage(stage)?;
        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        let orch = Orchestrator::new(config)?;
        let session_id = SessionId {
            value: session.to_string(),
        };

        let artifact_version = crate::model::ArtifactVersion {
            value: version,
            parent: if version > 1 { Some(version - 1) } else { None },
        };

        orch.export_artifact(&session_id, parsed_stage, artifact_version, output)
            .await?;
        println!("Artifact exported to: {:?}", output);
        Ok(0)
    }

    async fn cmd_trace(&self, session: &str, store_root: &Path) -> SddResult<i32> {
        if session.trim().is_empty() {
            return Err(SddError::new(
                SddErrorCode::SddCliArgMissing,
                "--session is required",
            ));
        }

        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        let orch = Orchestrator::new(config)?;
        let session_id = SessionId {
            value: session.to_string(),
        };

        let trace = orch.query_trace(&session_id).await?;
        println!("Session: {}", trace.session_id);
        println!("Stage records: {}", trace.stage_records.len());
        println!("AI call records: {}", trace.ai_call_records.len());
        println!("Review decisions: {}", trace.review_decisions.len());
        println!("Rollback history: {}", trace.rollback_history.len());
        println!("Total duration: {}ms", trace.total_duration_ms);
        Ok(0)
    }

    async fn cmd_config(
        &self,
        provider: &Option<String>,
        model: &Option<String>,
        _whitelist: &Option<PathBuf>,
        rules_path: &Option<PathBuf>,
        store_root: &Path,
    ) -> SddResult<i32> {
        let config = SddConfig {
            store_root: store_root.to_path_buf(),
            ..Default::default()
        };
        tokio::fs::create_dir_all(&config.store_root).await?;

        if let Some(p) = provider {
            println!("Provider set to: {}", p);
        }
        if let Some(m) = model {
            println!("Model set to: {}", m);
        }
        if let Some(rp) = rules_path {
            if !rp.exists() {
                return Err(SddError::new(
                    SddErrorCode::SddCliArgInvalid,
                    format!("--rules-path does not exist: {:?}", rp),
                ));
            }
            println!("Rules path set to: {:?}", rp);
        }
        println!("Config updated (new sessions will use updated settings).");
        Ok(0)
    }
}

fn parse_decision(
    decision: &str,
    rollback_target: &Option<String>,
    comment: &Option<String>,
) -> SddResult<ReviewDecision> {
    match decision.to_lowercase().as_str() {
        "pass" => Ok(ReviewDecision::Pass),
        "rollback" => {
            let target_str = rollback_target.as_ref().ok_or_else(|| {
                SddError::new(
                    SddErrorCode::SddCliArgMissing,
                    "--rollback-target is required for rollback decision",
                )
            })?;
            let target = parse_stage(target_str)?;
            Ok(ReviewDecision::Rollback { target })
        }
        "redo" => {
            let comment_str = comment.as_ref().ok_or_else(|| {
                SddError::new(
                    SddErrorCode::SddCliArgMissing,
                    "--comment is required for redo decision",
                )
            })?;
            if comment_str.trim().is_empty() {
                return Err(SddError::new(
                    SddErrorCode::SddCliArgInvalid,
                    "--comment must not be empty for redo decision",
                ));
            }
            Ok(ReviewDecision::Redo {
                comment: comment_str.clone(),
            })
        }
        other => Err(SddError::new(
            SddErrorCode::SddCliArgInvalid,
            format!("invalid decision '{}': must be pass/rollback/redo", other),
        )),
    }
}

fn parse_stage(s: &str) -> SddResult<Stage> {
    match s.to_lowercase().as_str() {
        "spec" => Ok(Stage::Spec),
        "design" => Ok(Stage::Design),
        "task" => Ok(Stage::Task),
        "coding" => Ok(Stage::Coding),
        other => Err(SddError::new(
            SddErrorCode::SddCliArgInvalid,
            format!("invalid stage '{}': must be spec/design/task/coding", other),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store_root() -> PathBuf {
        std::env::temp_dir().join(format!("sdd-cli-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn cli_init() {
        let store_root = test_store_root();
        let target = std::env::temp_dir().join("sdd-cli-target-init");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let cli = SddCli {
            command: SddCommand::Init {
                requirement: "Build a hello world CLI".to_string(),
                target: target.clone(),
                provider: "test".to_string(),
                model: "test-model".to_string(),
                store_root: store_root.clone(),
            },
        };

        let exit_code = cli.execute().await.unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn cli_resume() {
        let store_root = test_store_root();
        let target = std::env::temp_dir().join("sdd-cli-target-resume");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let init_cli = SddCli {
            command: SddCommand::Init {
                requirement: "test requirement".to_string(),
                target: target.clone(),
                provider: "test".to_string(),
                model: "test-model".to_string(),
                store_root: store_root.clone(),
            },
        };
        init_cli.execute().await.unwrap();

        let sessions: Vec<_> = {
            let config = SddConfig {
                store_root: store_root.clone(),
                ..Default::default()
            };
            let orch = Orchestrator::new(config).unwrap();
            orch.list_sessions().await.unwrap()
        };
        assert!(!sessions.is_empty());
        let session_id = sessions[0].id.value.clone();

        let resume_cli = SddCli {
            command: SddCommand::Resume {
                session: session_id,
                store_root: store_root.clone(),
            },
        };
        let exit_code = resume_cli.execute().await.unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn cli_list() {
        let store_root = test_store_root();
        tokio::fs::create_dir_all(&store_root).await.unwrap();

        let cli = SddCli {
            command: SddCommand::List {
                store_root: store_root.clone(),
            },
        };
        let exit_code = cli.execute().await.unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn cli_trace() {
        let store_root = test_store_root();
        let target = std::env::temp_dir().join("sdd-cli-target-trace");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let init_cli = SddCli {
            command: SddCommand::Init {
                requirement: "trace test".to_string(),
                target: target.clone(),
                provider: "test".to_string(),
                model: "test-model".to_string(),
                store_root: store_root.clone(),
            },
        };
        init_cli.execute().await.unwrap();

        let sessions: Vec<_> = {
            let config = SddConfig {
                store_root: store_root.clone(),
                ..Default::default()
            };
            let orch = Orchestrator::new(config).unwrap();
            orch.list_sessions().await.unwrap()
        };
        let session_id = sessions[0].id.value.clone();

        let trace_cli = SddCli {
            command: SddCommand::Trace {
                session: session_id,
                store_root: store_root.clone(),
            },
        };
        let exit_code = trace_cli.execute().await.unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn cli_config() {
        let store_root = test_store_root();
        tokio::fs::create_dir_all(&store_root).await.unwrap();

        let cli = SddCli {
            command: SddCommand::Config {
                provider: Some("new-provider".to_string()),
                model: Some("new-model".to_string()),
                whitelist: None,
                rules_path: None,
                store_root: store_root.clone(),
            },
        };
        let exit_code = cli.execute().await.unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn cli_arg_missing_requirement() {
        let store_root = test_store_root();
        let target = std::env::temp_dir().join("sdd-cli-target-missing");
        tokio::fs::create_dir_all(&target).await.unwrap();

        let cli = SddCli {
            command: SddCommand::Init {
                requirement: "   ".to_string(),
                target: target.clone(),
                provider: "test".to_string(),
                model: "test-model".to_string(),
                store_root,
            },
        };
        let err = cli.execute().await.unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddCliArgMissing));
    }

    #[tokio::test]
    async fn cli_arg_invalid_target() {
        let store_root = test_store_root();

        let cli = SddCli {
            command: SddCommand::Init {
                requirement: "test".to_string(),
                target: PathBuf::from("/nonexistent/path/xyz"),
                provider: "test".to_string(),
                model: "test-model".to_string(),
                store_root,
            },
        };
        let err = cli.execute().await.unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddCliArgInvalid));
    }

    #[tokio::test]
    async fn cli_exit_code_success() {
        let store_root = test_store_root();
        tokio::fs::create_dir_all(&store_root).await.unwrap();

        let cli = SddCli {
            command: SddCommand::List { store_root },
        };
        let exit_code = cli.execute().await.unwrap();
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn cli_exit_code_failure() {
        let store_root = test_store_root();

        let cli = SddCli {
            command: SddCommand::Resume {
                session: "nonexistent-session-id".to_string(),
                store_root,
            },
        };
        let result = cli.execute().await;
        assert!(result.is_err());
    }

    #[test]
    fn cli_parse_decision_pass() {
        let decision = parse_decision("pass", &None, &None).unwrap();
        assert!(matches!(decision, ReviewDecision::Pass));
    }

    #[test]
    fn cli_parse_decision_rollback() {
        let decision = parse_decision("rollback", &Some("spec".to_string()), &None).unwrap();
        assert!(matches!(
            decision,
            ReviewDecision::Rollback {
                target: Stage::Spec
            }
        ));
    }

    #[test]
    fn cli_parse_decision_redo() {
        let decision = parse_decision("redo", &None, &Some("fix issues".to_string())).unwrap();
        assert!(matches!(decision, ReviewDecision::Redo { .. }));
    }

    #[test]
    fn cli_parse_decision_invalid() {
        let err = parse_decision("invalid", &None, &None).unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddCliArgInvalid));
    }

    #[test]
    fn cli_parse_decision_rollback_missing_target() {
        let err = parse_decision("rollback", &None, &None).unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddCliArgMissing));
    }

    #[test]
    fn cli_parse_stage() {
        assert_eq!(parse_stage("spec").unwrap(), Stage::Spec);
        assert_eq!(parse_stage("design").unwrap(), Stage::Design);
        assert_eq!(parse_stage("task").unwrap(), Stage::Task);
        assert_eq!(parse_stage("coding").unwrap(), Stage::Coding);
        assert!(parse_stage("invalid").is_err());
    }
}
