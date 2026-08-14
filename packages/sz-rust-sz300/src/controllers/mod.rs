/// 管理监控控制器（admin feature 门控）
#[cfg(feature = "admin")]
pub mod admin;
/// AI 控制器（LLM 聊天接口）
pub mod ai;
/// 认证相关控制器（对齐 PHP AuthController）
pub mod auth;
/// Capability 控制器（能力注册表查询接口）
pub mod capabilities;
/// 控制器公共辅助函数（分页解析等）
pub mod common;
/// 设备相关控制器（对齐 PHP DeviceController）
pub mod device;
/// 文件上传控制器（对齐 PHP FileController）
pub mod file;
/// 静态文件服务控制器
pub mod file_serve;
/// 健康检查控制器
pub mod health;
/// 商户管理控制器（对齐 PHP MerchantController）
pub mod merchant;
/// 订单管理控制器（对齐 PHP OrderController）
pub mod order;
/// 商品管理控制器（对齐 PHP ProductController）
pub mod product;
/// 视图模板控制器（对接 sz-rust-core::view）
pub mod view;
