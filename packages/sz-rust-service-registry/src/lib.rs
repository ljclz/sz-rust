// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 服务注册发现 — ServiceRegistry trait + Consul/Nacos/K8s 适配 + 五种 LB 策略
//!
//! # 架构
//!
//! - `registry`：核心 trait + `ServiceInstance` + `LoadBalanceStrategy`
//! - `consul`：Consul HTTP API 适配（feature = "consul"）
//! - `nacos`：Nacos HTTP API 适配（feature = "nacos"）
//! - `kubernetes`：K8s Service 降级适配（feature = "kubernetes"）
//! - `load_balancer`：五种 LB 策略实现
//! - `local_cache`：注册中心断连降级缓存
//! - `health_check`：服务注册中心健康检查（readiness 探针适配）

#![forbid(unsafe_code)]

pub mod error;
pub mod health_check;
pub mod registry;

pub use health_check::{
    HealthCheck as ServiceRegistryHealthCheckTrait, ServiceRegistryHealthCheck,
};

#[cfg(feature = "consul")]
pub mod consul;
#[cfg(feature = "kubernetes")]
pub mod kubernetes;
#[cfg(feature = "nacos")]
pub mod nacos;

pub mod load_balancer;
pub mod local_cache;

pub use error::RegistryError;
pub use load_balancer::{LoadBalancer, SharedLoadBalancer};
pub use local_cache::LocalCache;
pub use registry::{
    ConnectionCounter, InstanceStatus, LoadBalanceStrategy, ServiceInstance, ServiceRegistry,
};
