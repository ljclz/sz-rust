// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 AI 智能分类端到端测试（spec §5.4）
//!
//! 验证 6 个业务规则：
//! - 规则 1: 创建商品含名称/描述 → 返回 AI 分类建议 + 置信度
//! - 规则 2: 人工 override → 最终分类=人工指定
//! - 规则 3: 置信度≥0.8 自动采用；<0.8 待人工确认
//! - 规则 4: AI 服务超时 → 降级人工分类
//! - 规则 5: AI 请求 payload 不含敏感字段
//! - 规则 6: 人工已指定 + AI 再分类 → 保留人工指定

#![cfg(feature = "v19-ai-classify")]

use std::sync::Mutex;

use async_trait::async_trait;
use sz_rust_sz300::services::ai_classifier::{
    sanitize_classification_input, AiClassifier, AiError, AiFacade, ClassificationDecision,
    ClassificationRequest, ClassificationResult,
};

struct MockAiFacade {
    results: Mutex<Vec<Result<ClassificationResult, AiError>>>,
}

impl MockAiFacade {
    fn new(results: Vec<Result<ClassificationResult, AiError>>) -> Self {
        Self {
            results: Mutex::new(results),
        }
    }
}

#[async_trait]
impl AiFacade for MockAiFacade {
    async fn classify(
        &self,
        _req: &ClassificationRequest,
    ) -> Result<ClassificationResult, AiError> {
        self.results
            .lock()
            .unwrap()
            .pop()
            .unwrap_or(Err(AiError::ServiceError("no more results".into())))
    }
}

#[tokio::test]
async fn test_rule1_ai_returns_classification_with_confidence() {
    let facade = MockAiFacade::new(vec![Ok(ClassificationResult {
        category_id: 5,
        confidence: 0.9,
    })]);
    let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
    let req = ClassificationRequest {
        product_name: "苹果".into(),
        product_desc: "新鲜水果".into(),
    };
    let result = classifier.classify(&req).await.unwrap();
    assert!(result.confidence >= 0.0 && result.confidence <= 1.0);
    assert!(result.category_id > 0);
}

#[tokio::test]
async fn test_rule3_high_confidence_auto_adopt() {
    let facade = MockAiFacade::new(vec![Ok(ClassificationResult {
        category_id: 3,
        confidence: 0.9,
    })]);
    let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
    let req = ClassificationRequest {
        product_name: "测试".into(),
        product_desc: "描述".into(),
    };
    let result = classifier.classify(&req).await.unwrap();
    let decision = classifier.decide(result, None);
    assert!(matches!(decision, ClassificationDecision::AutoAdopt { .. }));
}

#[tokio::test]
async fn test_rule3_low_confidence_pending_manual() {
    let facade = MockAiFacade::new(vec![Ok(ClassificationResult {
        category_id: 3,
        confidence: 0.6,
    })]);
    let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
    let req = ClassificationRequest {
        product_name: "测试".into(),
        product_desc: "描述".into(),
    };
    let result = classifier.classify(&req).await.unwrap();
    let decision = classifier.decide(result, None);
    assert!(matches!(
        decision,
        ClassificationDecision::PendingManual { .. }
    ));
}

#[tokio::test]
async fn test_rule2_manual_override_preserved() {
    let facade = MockAiFacade::new(vec![Ok(ClassificationResult {
        category_id: 3,
        confidence: 0.9,
    })]);
    let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
    let req = ClassificationRequest {
        product_name: "测试".into(),
        product_desc: "描述".into(),
    };
    let result = classifier.classify(&req).await.unwrap();
    let decision = classifier.decide(result, Some((7, 100)));
    match decision {
        ClassificationDecision::ManualOverride {
            original,
            overridden_to,
            overridden_by,
            ..
        } => {
            assert_eq!(original, 3);
            assert_eq!(overridden_to, 7);
            assert_eq!(overridden_by, 100);
        }
        _ => panic!("期望 ManualOverride"),
    }
}

#[tokio::test]
async fn test_rule4_ai_timeout_degrades_to_manual() {
    struct SlowFacade;
    #[async_trait]
    impl AiFacade for SlowFacade {
        async fn classify(
            &self,
            _req: &ClassificationRequest,
        ) -> Result<ClassificationResult, AiError> {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            Ok(ClassificationResult {
                category_id: 1,
                confidence: 0.9,
            })
        }
    }
    let classifier = AiClassifier::new(0.8, std::sync::Arc::new(SlowFacade))
        .with_timeout(std::time::Duration::from_millis(50));
    let req = ClassificationRequest {
        product_name: "测试".into(),
        product_desc: "描述".into(),
    };
    let result = classifier.classify(&req).await;
    assert!(result.is_err());
    let decision = classifier.degrade();
    assert!(matches!(decision, ClassificationDecision::DegradedToManual));
}

#[test]
fn test_rule5_sanitized_input_no_sensitive_fields() {
    let sanitized = sanitize_classification_input(
        "商品 password=secret123 token=abc",
        "barcode api_key=key456",
        "desc with access_token=tok789",
    );
    let combined = format!(
        "{} {} {}",
        sanitized.name, sanitized.barcode, sanitized.description
    );
    assert!(!combined.contains("password"));
    assert!(!combined.contains("token"));
    assert!(!combined.contains("secret"));
    assert!(!combined.contains("api_key"));
    assert!(!combined.contains("access_token"));
    assert!(combined.contains("<redacted>"));
}

#[tokio::test]
async fn test_rule6_manual_override_takes_precedence_over_ai() {
    let facade = MockAiFacade::new(vec![Ok(ClassificationResult {
        category_id: 3,
        confidence: 0.95,
    })]);
    let classifier = AiClassifier::new(0.8, std::sync::Arc::new(facade));
    let req = ClassificationRequest {
        product_name: "测试".into(),
        product_desc: "描述".into(),
    };
    let result = classifier.classify(&req).await.unwrap();
    let decision = classifier.decide(result, Some((5, 200)));
    match decision {
        ClassificationDecision::ManualOverride { overridden_to, .. } => {
            assert_eq!(overridden_to, 5);
        }
        ClassificationDecision::AutoAdopt { category_id, .. } => {
            panic!(
                "AI 不应自动采用，人工 override 应优先，但得到 AutoAdopt category_id={}",
                category_id
            );
        }
        _ => panic!("期望 ManualOverride"),
    }
}
