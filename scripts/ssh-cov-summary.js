const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

const passingTargets = [
    'cov_supplement_test', 'endpoint_coverage_test', 'shutdown_config_test',
    'windows_memory_baseline', 'config_test', 'metrics_test',
    'metrics_auth_router_test', 'metrics_auth_config_test', 'rate_limit_config_test',
    'health_check_config_test', 'circuit_breaker_config_test', 'capability_registry_test',
    'builders_test', 'redaction_test', 'services_test', 'mqtt_dispatch_test',
    'audit_remediation_v3_high_risk_test', 'audit_remediation_v2_tiktoken_test',
    'audit_remediation_v2_rag_pipeline_test', 'addon_deploy_ci_v3_cms_test',
    'addon_deploy_ci_v3_crm_test', 'audit_remediation_v2_local_embedding_test',
    'audit_remediation_v2_agent_test', 'ai_vector_db_test', 'ai_integration_test',
    'addon_deep_wiring_v1_test', 'plugin_interop_bench', 'plugin_interop_e2e',
    'rag_integration_test', 'jobs_integration_test', 'ecommerce_integration_test'
];

const testArgs = passingTargets.map(t => `--test ${t}`).join(' ');

conn.on('ready', () => {
    const cmd = `cd /www/rust/sz-rust-enterprise && CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo llvm-cov -p sz-rust-sz300 --summary-only --ignore-filename-regex "main.rs" --lib ${testArgs} 2>&1 | tail -30`;
    console.log('Getting coverage summary...');
    conn.exec(cmd, (err, stream) => {
        if (err) { console.error(err); conn.end(); return; }
        let out = '';
        stream.on('data', (d) => { out += d; });
        stream.stderr.on('data', (d) => { out += d; });
        stream.on('close', (code) => {
            console.log(out);
            conn.end();
        });
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
    readyTimeout: 30000,
});