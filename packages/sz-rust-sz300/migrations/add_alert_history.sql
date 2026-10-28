-- v1.9.0: 告警历史记录表
-- 对齐 spec 5.7.1：告警触发记录 + 分级 + 静默管理

CREATE TABLE IF NOT EXISTS alert_history (
  id         BIGINT       NOT NULL AUTO_INCREMENT,
  rule_id    VARCHAR(128) NOT NULL,
  severity   VARCHAR(16)  NOT NULL,
  payload    JSON         NULL,
  fired_at   DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  PRIMARY KEY (id),
  INDEX idx_alert_rule (rule_id),
  INDEX idx_alert_fired (fired_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='告警历史记录';