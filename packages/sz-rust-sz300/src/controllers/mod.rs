/// 认证相关控制器（对齐 PHP AuthController）
pub mod auth;
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