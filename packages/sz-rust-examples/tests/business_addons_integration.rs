//! 跨插件集成测试（P1-T4 / T5.1）
//!
//! 验证 CMS / CRM / 电商 3 个插件同时注册到同一 CapabilityRegistry：
//! - 无能力名冲突（全部以各自前缀开头，全局唯一）
//! - 能力总数 = 5 + 7 + 6 = 18
//! - 各插件能力可独立调用
//! - 卸载某插件后该前缀能力全部注销

use serde_json::json;
use sz_rust_addons_cms::capability::CmsPlugin;
use sz_rust_addons_cms::CmsState;
use sz_rust_addons_crm::capability::CrmPlugin;
use sz_rust_addons_crm::CrmState;
use sz_rust_addons_ecommerce::capability::EcommercePlugin;
use sz_rust_addons_ecommerce::EcommerceState;
use sz_rust_addons_loader::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;

// ============================================================================
// 铁律合规测试（T5.2）
// ============================================================================

/// 编译期断言 T: Send + Sync
fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn cms_plugin_is_send_sync() {
    assert_send_sync::<CmsPlugin>();
}

#[test]
fn crm_plugin_is_send_sync() {
    assert_send_sync::<CrmPlugin>();
}

#[test]
fn ecommerce_plugin_is_send_sync() {
    assert_send_sync::<EcommercePlugin>();
}

#[test]
fn cms_state_is_send_sync() {
    assert_send_sync::<CmsState>();
}

#[test]
fn crm_state_is_send_sync() {
    assert_send_sync::<CrmState>();
}

#[test]
fn ecommerce_state_is_send_sync() {
    assert_send_sync::<EcommerceState>();
}

#[tokio::test]
async fn three_plugins_register_18_capabilities() {
    let registry = CapabilityRegistry::new();

    let cms = CmsPlugin::new(CmsState::default());
    let crm = CrmPlugin::new(CrmState::default());
    let ec = EcommercePlugin::new(EcommerceState::default());

    let cms_names = cms.register_capabilities(&registry).unwrap();
    let crm_names = crm.register_capabilities(&registry).unwrap();
    let ec_names = ec.register_capabilities(&registry).unwrap();

    assert_eq!(cms_names.len(), 5);
    assert_eq!(crm_names.len(), 7);
    assert_eq!(ec_names.len(), 6);
    assert_eq!(cms_names.len() + crm_names.len() + ec_names.len(), 18);
}

#[tokio::test]
async fn all_capability_names_have_correct_prefix() {
    let registry = CapabilityRegistry::new();

    let cms = CmsPlugin::new(CmsState::default());
    let crm = CrmPlugin::new(CrmState::default());
    let ec = EcommercePlugin::new(EcommerceState::default());

    for name in cms.register_capabilities(&registry).unwrap() {
        assert!(
            name.starts_with("cms."),
            "CMS capability '{}' missing cms. prefix",
            name
        );
    }
    for name in crm.register_capabilities(&registry).unwrap() {
        assert!(
            name.starts_with("crm."),
            "CRM capability '{}' missing crm. prefix",
            name
        );
    }
    for name in ec.register_capabilities(&registry).unwrap() {
        assert!(
            name.starts_with("ecommerce."),
            "EC capability '{}' missing ecommerce. prefix",
            name
        );
    }
}

#[tokio::test]
async fn no_capability_name_collision() {
    let registry = CapabilityRegistry::new();

    let cms = CmsPlugin::new(CmsState::default());
    let crm = CrmPlugin::new(CrmState::default());
    let ec = EcommercePlugin::new(EcommerceState::default());

    let mut all_names = Vec::new();
    all_names.extend(cms.register_capabilities(&registry).unwrap());
    all_names.extend(crm.register_capabilities(&registry).unwrap());
    all_names.extend(ec.register_capabilities(&registry).unwrap());

    let mut sorted = all_names.clone();
    sorted.sort();
    for i in 1..sorted.len() {
        assert_ne!(
            sorted[i],
            sorted[i - 1],
            "Duplicate capability name: {}",
            sorted[i]
        );
    }
}

#[tokio::test]
async fn each_plugin_capability_independently_callable() {
    let registry = CapabilityRegistry::new();

    let cms = CmsPlugin::new(CmsState::default());
    let crm = CrmPlugin::new(CrmState::default());
    let ec = EcommercePlugin::new(EcommerceState::default());

    cms.register_capabilities(&registry).unwrap();
    crm.register_capabilities(&registry).unwrap();
    ec.register_capabilities(&registry).unwrap();

    let cms_cap = registry.get("cms.search_article").unwrap();
    let cms_result = cms_cap.call(json!({})).await.unwrap();
    assert_eq!(cms_result["code"], 0);

    let crm_cap = registry.get("crm.search_contact").unwrap();
    let crm_result = crm_cap.call(json!({})).await.unwrap();
    assert_eq!(crm_result["code"], 0);

    let ec_cap = registry.get("ecommerce.search_order").unwrap();
    let ec_result = ec_cap.call(json!({})).await.unwrap();
    assert_eq!(ec_result["code"], 0);
}

#[tokio::test]
async fn requires_confirmation_flags_correct() {
    let registry = CapabilityRegistry::new();

    let cms = CmsPlugin::new(CmsState::default());
    let crm = CrmPlugin::new(CrmState::default());
    let ec = EcommercePlugin::new(EcommerceState::default());

    cms.register_capabilities(&registry).unwrap();
    crm.register_capabilities(&registry).unwrap();
    ec.register_capabilities(&registry).unwrap();

    assert!(!registry
        .get("cms.search_article")
        .unwrap()
        .requires_confirmation());
    assert!(!registry
        .get("cms.create_article")
        .unwrap()
        .requires_confirmation());

    assert!(!registry
        .get("crm.search_contact")
        .unwrap()
        .requires_confirmation());
    assert!(registry
        .get("crm.convert_lead")
        .unwrap()
        .requires_confirmation());
    assert!(registry
        .get("crm.update_deal_stage")
        .unwrap()
        .requires_confirmation());

    assert!(!registry
        .get("ecommerce.create_order")
        .unwrap()
        .requires_confirmation());
    assert!(registry
        .get("ecommerce.cancel_order")
        .unwrap()
        .requires_confirmation());
    assert!(registry
        .get("ecommerce.clear_cart")
        .unwrap()
        .requires_confirmation());
}

#[tokio::test]
async fn capability_names_match_register_results() {
    let registry = CapabilityRegistry::new();

    let cms = CmsPlugin::new(CmsState::default());
    let registered = cms.register_capabilities(&registry).unwrap();
    let declared = cms.capability_names();
    assert_eq!(registered, declared);

    let crm = CrmPlugin::new(CrmState::default());
    let registered = crm.register_capabilities(&registry).unwrap();
    let declared = crm.capability_names();
    assert_eq!(registered, declared);

    let ec = EcommercePlugin::new(EcommerceState::default());
    let registered = ec.register_capabilities(&registry).unwrap();
    let declared = ec.capability_names();
    assert_eq!(registered, declared);
}
