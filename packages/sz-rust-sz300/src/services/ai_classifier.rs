// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 AI 智能分类（spec §5.4）
//!
//! 商品创建/更新时调用 AI 服务返回分类建议与置信度，
//! 置信度 ≥ 阈值自动采用，< 阈值待人工确认，人工 override 保留人工指定。

use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

/// AI 分类请求（spec §5.4.1 规则 1）
#[derive(Debug, Clone)]
pub struct ClassificationRequest {
    /// 商品名称
    pub product_name: String,
    /// 商品描述
    pub product_desc: String,
}

/// AI 分类结果（spec §5.4.1 规则 1）
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// 分类 ID
    pub category_id: i64,
    /// 置信度（0.0 - 1.0）
    pub confidence: f64,
}

/// 分类决策（spec §5.4.1 规则 2/3/6）
#[derive(Debug, Clone)]
pub enum ClassificationDecision {
    /// 置信度 ≥ 阈值，自动采用（规则 3）
    AutoAdopt {
        /// 采用的分类 ID
        category_id: i64,
        /// 置信度
        confidence: f64,
    },
    /// 置信度 < 阈值，待人工确认（规则 3）
    PendingManual {
        /// AI 建议结果
        suggestion: ClassificationResult,
    },
    /// 人工 override，保留人工指定（规则 2/6）
    ManualOverride {
        /// 原始 AI 分类
        original: i64,
        /// 人工指定分类
        overridden_to: i64,
        /// 操作人 ID
        overridden_by: i64,
        /// 操作时间
        at: chrono::DateTime<chrono::Utc>,
    },
    /// AI 不可用，降级为人工分类（规则 4）
    DegradedToManual,
}

/// AI 推理服务 trait（抽象 ai-facade，便于测试 mock）
#[async_trait]
pub trait AiFacade: Send + Sync {
    /// 调用 AI 服务返回分类建议 + 置信度
    async fn classify(&self, req: &ClassificationRequest) -> Result<ClassificationResult, AiError>;
}

/// AI 分类错误
#[derive(Debug, thiserror::Error)]
pub enum AiError {
    /// AI 服务超时（spec §5.4.3 异常 1）
    #[error("AI 服务超时")]
    Timeout,
    /// AI 服务返回异常（spec §5.4.3 异常 2）
    #[error("AI 服务错误: {0}")]
    ServiceError(String),
    /// AI 服务返回无效响应
    #[error("AI 响应解析失败: {0}")]
    ParseError(String),
}

/// 脱敏后的商品输入（spec §5.4.1 规则 5）
#[derive(Debug, Clone)]
pub struct SanitizedProduct {
    /// 商品名称（安全字段）
    pub name: String,
    /// 商品条形码（安全字段）
    pub barcode: String,
    /// 商品描述（安全字段）
    pub description: String,
}

/// 敏感字段黑名单（spec §5.4.1 规则 5）
const SENSITIVE_FIELDS: &[&str] = &[
    "password",
    "token",
    "secret",
    "api_key",
    "access_token",
    "refresh_token",
    "id_card",
    "bank_account",
];

/// 脱敏分类输入（spec §5.4.1 规则 5）
///
/// 移除 password/token/secret 等敏感字段，仅保留安全字段传递给 AI 服务。
pub fn sanitize_classification_input(
    name: &str,
    barcode: &str,
    description: &str,
) -> SanitizedProduct {
    SanitizedProduct {
        name: mask_sensitive_in_text(name),
        barcode: mask_sensitive_in_text(barcode),
        description: mask_sensitive_in_text(description),
    }
}

/// 扫描文本中敏感字段名并替换为 `<redacted>`
fn mask_sensitive_in_text(text: &str) -> String {
    let mut result = text.to_string();
    for field in SENSITIVE_FIELDS {
        let lower = result.to_lowercase();
        if lower.contains(field) {
            result = result.replace(field, "<redacted>");
            let capitalized = format!("{}{}", field[..1].to_uppercase(), &field[1..]);
            result = result.replace(&capitalized, "<redacted>");
        }
    }
    result
}

/// v1.9.0 AI 分类器（spec §5.4）
pub struct AiClassifier {
    /// 置信度阈值（默认 0.8，spec §5.4.1 规则 3）
    threshold: f64,
    /// AI 超时时间（默认 2s，spec §5.4.3 异常 1）
    timeout: Duration,
    /// AI 推理服务句柄
    facade: std::sync::Arc<dyn AiFacade>,
}

impl AiClassifier {
    /// 创建 AI 分类器
    pub fn new(threshold: f64, facade: std::sync::Arc<dyn AiFacade>) -> Self {
        Self {
            threshold,
            timeout: Duration::from_secs(2),
            facade,
        }
    }

    /// 设置超时时间
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 获取置信度阈值
    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    /// 调用 AI 服务分类（spec §5.4.1 规则 1）
    ///
    /// 超时或错误时返回 `AiError`，调用方应降级为人工分类（规则 4）。
    pub async fn classify(
        &self,
        req: &ClassificationRequest,
    ) -> Result<ClassificationResult, AiError> {
        let timeout_result = tokio::time::timeout(self.timeout, self.facade.classify(req)).await;
        match timeout_result {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(
                    "AI 分类超时（{}ms），降级为人工分类",
                    self.timeout.as_millis()
                );
                Err(AiError::Timeout)
            }
        }
    }

    /// 分类决策（spec §5.4.1 规则 2/3/6）
    ///
    /// - 人工已指定 → `ManualOverride`，保留人工指定（规则 6）
    /// - 置信度 ≥ 阈值 → `AutoAdopt`（规则 3）
    /// - 置信度 < 阈值 → `PendingManual`（规则 3）
    pub fn decide(
        &self,
        result: ClassificationResult,
        manual: Option<(i64, i64)>,
    ) -> ClassificationDecision {
        if let Some((manual_category, operator_id)) = manual {
            tracing::info!(
                "人工 override: AI 建议={}, 人工指定={}, 操作人={}",
                result.category_id,
                manual_category,
                operator_id
            );
            return ClassificationDecision::ManualOverride {
                original: result.category_id,
                overridden_to: manual_category,
                overridden_by: operator_id,
                at: Utc::now(),
            };
        }
        if result.confidence >= self.threshold {
            ClassificationDecision::AutoAdopt {
                category_id: result.category_id,
                confidence: result.confidence,
            }
        } else {
            ClassificationDecision::PendingManual { suggestion: result }
        }
    }

    /// 降级分类（spec §5.4.1 规则 4 / §5.4.3 异常 1/2）
    ///
    /// AI 服务不可用时返回 `DegradedToManual`，不阻塞商品创建。
    pub fn degrade(&self) -> ClassificationDecision {
        ClassificationDecision::DegradedToManual
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockAiFacade {
        result: Mutex<Option<Result<ClassificationResult, AiError>>>,
    }

    impl MockAiFacade {
        fn success(category_id: i64, confidence: f64) -> Self {
            Self {
                result: Mutex::new(Some(Ok(ClassificationResult {
                    category_id,
                    confidence,
                }))),
            }
        }

        fn error() -> Self {
            Self {
                result: Mutex::new(Some(Err(AiError::ServiceError("mock error".into())))),
            }
        }
    }

    #[async_trait]
    impl AiFacade for MockAiFacade {
        async fn classify(
            &self,
            _req: &ClassificationRequest,
        ) -> Result<ClassificationResult, AiError> {
            self.result
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Err(AiError::ServiceError("no more results".into())))
        }
    }

    #[test]
    fn test_sanitize_removes_sensitive_fields() {
        let sanitized = sanitize_classification_input(
            "商品 password=123456",
            "barcode token=abc",
            "desc with secret data",
        );
        assert!(sanitized.name.contains("<redacted>"));
        assert!(!sanitized.name.contains("password"));
        assert!(sanitized.barcode.contains("<redacted>"));
        assert!(!sanitized.barcode.contains("token"));
        assert!(sanitized.description.contains("<redacted>"));
        assert!(!sanitized.description.contains("secret"));
    }

    #[test]
    fn test_sanitize_preserves_safe_fields() {
        let sanitized = sanitize_classification_input("苹果", "6901234567890", "新鲜水果");
        assert_eq!(sanitized.name, "苹果");
        assert_eq!(sanitized.barcode, "6901234567890");
        assert_eq!(sanitized.description, "新鲜水果");
    }

    #[test]
    fn test_decide_auto_adopt() {
        let facade = MockAiFacade::success(1, 0.9);
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
        let result = ClassificationResult {
            category_id: 1,
            confidence: 0.9,
        };
        let decision = classifier.decide(result, None);
        match decision {
            ClassificationDecision::AutoAdopt {
                category_id,
                confidence,
            } => {
                assert_eq!(category_id, 1);
                assert!((confidence - 0.9).abs() < 0.001);
            }
            _ => panic!("期望 AutoAdopt"),
        }
    }

    #[test]
    fn test_decide_pending_manual() {
        let facade = MockAiFacade::success(1, 0.6);
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
        let result = ClassificationResult {
            category_id: 1,
            confidence: 0.6,
        };
        let decision = classifier.decide(result, None);
        match decision {
            ClassificationDecision::PendingManual { suggestion } => {
                assert_eq!(suggestion.category_id, 1);
                assert!((suggestion.confidence - 0.6).abs() < 0.001);
            }
            _ => panic!("期望 PendingManual"),
        }
    }

    #[test]
    fn test_decide_manual_override() {
        let facade = MockAiFacade::success(1, 0.9);
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
        let result = ClassificationResult {
            category_id: 1,
            confidence: 0.9,
        };
        let decision = classifier.decide(result, Some((2, 100)));
        match decision {
            ClassificationDecision::ManualOverride {
                original,
                overridden_to,
                overridden_by,
                ..
            } => {
                assert_eq!(original, 1);
                assert_eq!(overridden_to, 2);
                assert_eq!(overridden_by, 100);
            }
            _ => panic!("期望 ManualOverride"),
        }
    }

    #[test]
    fn test_degrade() {
        let facade = MockAiFacade::success(1, 0.9);
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
        let decision = classifier.degrade();
        assert!(matches!(decision, ClassificationDecision::DegradedToManual));
    }

    #[tokio::test]
    async fn test_classify_success() {
        let facade = MockAiFacade::success(5, 0.95);
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
        let req = ClassificationRequest {
            product_name: "苹果".into(),
            product_desc: "新鲜水果".into(),
        };
        let result = classifier.classify(&req).await.unwrap();
        assert_eq!(result.category_id, 5);
        assert!((result.confidence - 0.95).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_classify_service_error() {
        let facade = MockAiFacade::error();
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
        let req = ClassificationRequest {
            product_name: "苹果".into(),
            product_desc: "新鲜水果".into(),
        };
        let result = classifier.classify(&req).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AiError::ServiceError(msg) => assert!(msg.contains("mock error")),
            _ => panic!("期望 ServiceError"),
        }
    }

    #[tokio::test]
    async fn test_classify_timeout() {
        struct SlowFacade;
        #[async_trait]
        impl AiFacade for SlowFacade {
            async fn classify(
                &self,
                _req: &ClassificationRequest,
            ) -> Result<ClassificationResult, AiError> {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(ClassificationResult {
                    category_id: 1,
                    confidence: 0.9,
                })
            }
        }
        let classifier = AiClassifier::new(0.8, std::sync::Arc::new(SlowFacade))
            .with_timeout(Duration::from_millis(50));
        let req = ClassificationRequest {
            product_name: "苹果".into(),
            product_desc: "新鲜水果".into(),
        };
        let result = classifier.classify(&req).await;
        assert!(matches!(result, Err(AiError::Timeout)));
    }

    #[test]
    fn test_threshold_getter() {
        let facade = MockAiFacade::success(1, 0.9);
        let classifier = AiClassifier::new(0.85, std::sync::Arc::new(facade));
        assert!((classifier.threshold() - 0.85).abs() < 0.001);
    }
}
