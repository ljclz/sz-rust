/// 认证服务模块（对齐 PHP AuthService）
pub mod auth_service;
/// 设备服务模块（封装设备 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod device_service;
/// 文件服务模块（对齐 PHP FileService）
pub mod file_service;
/// 商户服务模块（封装商户 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod merchant_service;
/// MQTT 监听服务模块
pub mod mqtt_listener;
/// MQTT 业务服务模块
pub mod mqtt_service;
/// 订单服务模块（封装订单 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod order_service;
/// 商品服务模块（封装商品 SQL 操作，2026-07-25 新增 — 修复控制器分层违反）
pub mod product_service;
