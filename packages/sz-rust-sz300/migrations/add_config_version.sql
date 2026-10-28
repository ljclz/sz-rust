-- v1.9.0: 配置版本追溯表
-- 对齐 spec 5.6.1：动态配置热更新 + 版本追溯 + 多实例一致

CREATE TABLE IF NOT EXISTS config_version (
  version    BIGINT       NOT NULL,
  content    JSON         NOT NULL,
  updated_at DATETIME(3)  NOT NULL DEFAULT CURRENT_TIMESTAMP(3),
  updated_by VARCHAR(64)  NOT NULL,
  PRIMARY KEY (version)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='配置版本追溯';