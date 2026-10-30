-- v1.9.0: AI 分类人工 override 记录表
-- 对齐 spec 5.4.1：人工修正 AI 推荐分类，版本追溯

CREATE TABLE IF NOT EXISTS ai_classification_override (
  product_id        BIGINT       NOT NULL,
  original_category BIGINT       NOT NULL,
  new_category      BIGINT       NOT NULL,
  operator_id       BIGINT       NOT NULL,
  at                DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  PRIMARY KEY (product_id, at),
  INDEX idx_ai_override_operator (operator_id),
  INDEX idx_ai_override_category (new_category)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI 分类人工 override 记录';