-- SZ-300 核心数据库 Schema v1.0
-- 注意：使用 MySQL，端口 8802

-- 1. 市场表
CREATE TABLE IF NOT EXISTS `market` (
    `market_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `name` VARCHAR(100) NOT NULL COMMENT '市场名称',
    `address` VARCHAR(255) DEFAULT '' COMMENT '地址',
    `contact_name` VARCHAR(50) DEFAULT '' COMMENT '联系人',
    `contact_phone` VARCHAR(20) DEFAULT '' COMMENT '联系电话',
    `status` TINYINT DEFAULT 1 COMMENT '1启用 0停用',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='市场表';

-- 2. 商户表
CREATE TABLE IF NOT EXISTS `merchant` (
    `merchant_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `market_id` INT UNSIGNED NOT NULL COMMENT '所属市场',
    `name` VARCHAR(100) NOT NULL COMMENT '商户名称',
    `stall_no` VARCHAR(50) DEFAULT '' COMMENT '摊位号',
    `contact_phone` VARCHAR(20) DEFAULT '' COMMENT '联系电话',
    `category` VARCHAR(50) DEFAULT '' COMMENT '经营品类（蔬菜/水果/肉类等）',
    `status` TINYINT DEFAULT 1 COMMENT '1启用 0停用',
    `bank_account` VARCHAR(50) DEFAULT '' COMMENT '银行结算账号',
    `bank_name` VARCHAR(100) DEFAULT '' COMMENT '开户行',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX `idx_market` (`market_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='商户表';

-- 3. 商品分类表
CREATE TABLE IF NOT EXISTS `category` (
    `cat_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `name` VARCHAR(50) NOT NULL COMMENT '分类名称',
    `parent_id` INT UNSIGNED DEFAULT 0 COMMENT '父分类ID',
    `sort_order` TINYINT DEFAULT 50 COMMENT '排序',
    `status` TINYINT DEFAULT 1,
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='商品分类表';

-- 4. 商品表
CREATE TABLE IF NOT EXISTS `good` (
    `good_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `merchant_id` INT UNSIGNED NOT NULL COMMENT '所属商户',
    `cat_id` INT UNSIGNED DEFAULT 0 COMMENT '分类ID',
    `name` VARCHAR(100) NOT NULL COMMENT '商品名称',
    `barcode` VARCHAR(50) DEFAULT '' COMMENT '条码',
    `price` INT UNSIGNED NOT NULL COMMENT '单价（分）',
    `unit` VARCHAR(10) DEFAULT '斤' COMMENT '单位',
    `ai_class_id` INT UNSIGNED DEFAULT 0 COMMENT 'AI识别分类ID',
    `image` VARCHAR(255) DEFAULT '' COMMENT '商品图片URL',
    `status` TINYINT DEFAULT 1 COMMENT '1上架 0下架',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX `idx_merchant` (`merchant_id`),
    INDEX `idx_ai_class` (`ai_class_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='商品表';

-- 5. AI 识别分类表
CREATE TABLE IF NOT EXISTS `ai_category` (
    `ai_class_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `name` VARCHAR(50) NOT NULL COMMENT '品类名称',
    `cat_id` INT UNSIGNED DEFAULT 0 COMMENT '关联系统分类',
    `model_version` VARCHAR(20) DEFAULT '' COMMENT '所属模型版本',
    `status` TINYINT DEFAULT 1,
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='AI识别分类表';

-- 6. 设备表
CREATE TABLE IF NOT EXISTS `device` (
    `device_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `merchant_id` INT UNSIGNED DEFAULT 0 COMMENT '绑定商户',
    `device_sn` VARCHAR(50) NOT NULL UNIQUE COMMENT '设备序列号',
    `device_model` VARCHAR(20) DEFAULT 'SZ-300' COMMENT '设备型号',
    `fw_version` VARCHAR(20) DEFAULT '' COMMENT '当前固件版本',
    `status` TINYINT DEFAULT 0 COMMENT '0离线 1在线',
    `signal_strength` INT DEFAULT 0 COMMENT '信号强度 dBm',
    `bind_at` DATETIME DEFAULT NULL COMMENT '绑定时间',
    `last_online_at` DATETIME DEFAULT NULL COMMENT '最后在线时间',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX `idx_merchant` (`merchant_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='设备表';

-- 7. 订单表
CREATE TABLE IF NOT EXISTS `order` (
    `order_id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `order_no` VARCHAR(32) NOT NULL UNIQUE COMMENT '订单号',
    `merchant_id` INT UNSIGNED NOT NULL COMMENT '商户ID',
    `device_id` INT UNSIGNED DEFAULT 0 COMMENT '设备ID',
    `total_fen` INT UNSIGNED NOT NULL COMMENT '总金额（分）',
    `total_weight_g` INT UNSIGNED DEFAULT 0 COMMENT '总重量（克）',
    `item_count` SMALLINT UNSIGNED DEFAULT 0 COMMENT '商品种类数',
    `status` TINYINT DEFAULT 0 COMMENT '0待支付 1已支付 2已退款 3已取消',
    `pay_method` TINYINT DEFAULT 0 COMMENT '支付方式 0扫码 1现金 2其他',
    `pay_at` DATETIME DEFAULT NULL COMMENT '支付时间',
    `offline_seq` VARCHAR(50) DEFAULT '' COMMENT '离线序列号（设备本地）',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX `idx_merchant` (`merchant_id`),
    INDEX `idx_device` (`device_id`),
    INDEX `idx_status` (`status`),
    INDEX `idx_created` (`created_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='订单表';

-- 8. 订单商品表
CREATE TABLE IF NOT EXISTS `order_item` (
    `item_id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `order_id` BIGINT UNSIGNED NOT NULL COMMENT '订单ID',
    `good_id` INT UNSIGNED DEFAULT 0 COMMENT '商品ID',
    `good_name` VARCHAR(100) NOT NULL COMMENT '商品名称',
    `price_fen` INT UNSIGNED NOT NULL COMMENT '单价（分）',
    `weight_g` INT UNSIGNED DEFAULT 0 COMMENT '重量（克）',
    `total_fen` INT UNSIGNED NOT NULL COMMENT '小计（分）',
    `quantity` SMALLINT UNSIGNED DEFAULT 1 COMMENT '数量',
    INDEX `idx_order` (`order_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='订单商品表';

-- 9. 结算记录表
CREATE TABLE IF NOT EXISTS `settlement` (
    `settle_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `merchant_id` INT UNSIGNED NOT NULL COMMENT '商户ID',
    `settle_date` DATE NOT NULL COMMENT '结算日期',
    `total_fen` INT UNSIGNED NOT NULL COMMENT '结算金额（分）',
    `order_count` INT UNSIGNED DEFAULT 0 COMMENT '订单数',
    `fee_fen` INT UNSIGNED DEFAULT 0 COMMENT '手续费（分）',
    `status` TINYINT DEFAULT 0 COMMENT '0待结算 1已结算',
    `settle_at` DATETIME DEFAULT NULL COMMENT '实际结算时间',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    INDEX `idx_merchant_date` (`merchant_id`, `settle_date`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='结算记录表';

-- 10. OTA 版本表
CREATE TABLE IF NOT EXISTS `ota_version` (
    `ota_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `version` VARCHAR(20) NOT NULL COMMENT '版本号',
    `device_model` VARCHAR(20) DEFAULT 'SZ-300' COMMENT '适用型号',
    `url` VARCHAR(255) NOT NULL COMMENT '固件下载URL',
    `md5` VARCHAR(32) DEFAULT '' COMMENT '固件MD5',
    `changelog` TEXT COMMENT '更新日志',
    `size` INT UNSIGNED DEFAULT 0 COMMENT '固件大小（字节）',
    `forced` TINYINT DEFAULT 0 COMMENT '是否强制升级',
    `status` TINYINT DEFAULT 1 COMMENT '1发布 0草稿',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='OTA版本表';

-- 11. 操作日志表
CREATE TABLE IF NOT EXISTS `operate_log` (
    `log_id` BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `merchant_id` INT UNSIGNED DEFAULT 0 COMMENT '商户ID',
    `operator` VARCHAR(50) DEFAULT '' COMMENT '操作人',
    `action` VARCHAR(50) NOT NULL COMMENT '操作类型',
    `detail` TEXT COMMENT '操作详情（JSON）',
    `ip` VARCHAR(45) DEFAULT '' COMMENT '操作IP',
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    INDEX `idx_merchant` (`merchant_id`),
    INDEX `idx_action` (`action`),
    INDEX `idx_created` (`created_at`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='操作日志表';

-- 12. 商户用户表
CREATE TABLE IF NOT EXISTS `merchant_user` (
    `user_id` INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,
    `merchant_id` INT UNSIGNED NOT NULL COMMENT '所属商户',
    `username` VARCHAR(50) NOT NULL UNIQUE COMMENT '用户名',
    `password_hash` VARCHAR(255) NOT NULL COMMENT '密码hash',
    `phone` VARCHAR(20) DEFAULT '' COMMENT '手机号',
    `role` TINYINT DEFAULT 1 COMMENT '1管理员 2操作员',
    `status` TINYINT DEFAULT 1,
    `last_login_at` DATETIME DEFAULT NULL,
    `created_at` DATETIME DEFAULT CURRENT_TIMESTAMP,
    INDEX `idx_merchant` (`merchant_id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='商户用户表';

-- 13. 系统配置表
CREATE TABLE IF NOT EXISTS `system_config` (
    `key_name` VARCHAR(100) PRIMARY KEY,
    `value` TEXT COMMENT '配置值',
    `description` VARCHAR(255) DEFAULT '' COMMENT '描述',
    `updated_at` DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COMMENT='系统配置表';
