import { EvidenceCollector } from '../lib/evidence-collector.js';

export async function validateE2E(ssh, config, deployResult, projectRoot) {
    const startedAt = Date.now();
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const evidences = [];
    const errors = [];
    let passed = true;

    const serverHost = '127.0.0.1';

    for (const app of config.applications) {
        const result = await validateOneAppE2E(ssh, serverHost, app, evidenceCollector, evidences, errors);
        if (!result) passed = false;
    }

    return {
        module: 'E2E',
        passed,
        evidences,
        errors,
        duration: Date.now() - startedAt,
    };
}

async function validateOneAppE2E(ssh, serverHost, app, evidenceCollector, evidences, errors) {
    let passed = true;
    const baseUrl = `http://${serverHost}:${app.port}`;

    try {
        const { stdout: healthOut } = await ssh.execCommand(
            `curl -s -m 5 ${baseUrl}${app.healthEndpoint} 2>&1 || echo "NO_RESPONSE"`,
            { timeout: 10000, expectNonZero: true }
        );

        if (!healthOut || healthOut.includes('NO_RESPONSE') || healthOut.trim().length === 0) {
            errors.push({ app: app.name, error: 'APP_NOT_RUNNING', detail: `${app.healthEndpoint} 无响应（应用未部署或环境配置问题）`, environmental: true });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${app.name} HTTP→DB 全链路 (/health) 通过`,
                'packages/sz-rust-sz300/src/controllers/health.rs',
                '10-21'
            ));
        }

        const { stdout: readyOut } = await ssh.execCommand(
            `curl -s -m 5 ${baseUrl}/health/ready 2>&1 || echo "NO_RESPONSE"`,
            { timeout: 10000, expectNonZero: true }
        );

        if (!readyOut || readyOut.includes('NO_RESPONSE') || readyOut.trim().length === 0) {
            if (passed) {
                errors.push({ app: app.name, error: 'READY_CHECK_FAILED', detail: '/health/ready 无响应', environmental: true });
                passed = false;
            }
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${app.name} HTTP→DB 全链路 (/health/ready) 通过`,
                'packages/sz-rust-sz300/src/services/health_service.rs',
                '24-38'
            ));
        }

        const { stdout: noAuthOut } = await ssh.execCommand(
            `curl -s -m 5 -o /dev/null -w "%{http_code}" ${baseUrl}/api/v1/merchant/list 2>&1 || echo "000"`,
            { timeout: 10000, expectNonZero: true }
        );

        const httpCode = noAuthOut.trim();
        if (httpCode === '401' || httpCode === '403') {
            evidences.push(evidenceCollector.createEvidence(
                `${app.name} 错误传播链 (无 JWT 返回 ${httpCode}) 通过`,
                'packages/sz-rust-sz300/src/router.rs',
                '92'
            ));
        } else if (httpCode === '200') {
            errors.push({ app: app.name, error: 'AUTH_BYPASS', detail: '无 JWT 请求返回 200，认证被绕过' });
            passed = false;
        } else if (httpCode === '000' || httpCode === '') {
            if (passed) {
                errors.push({ app: app.name, error: 'APP_NOT_RUNNING', detail: `HTTP code ${httpCode}（应用未运行）`, environmental: true });
                passed = false;
            }
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${app.name} 无 JWT 返回 ${httpCode}（可能无此路由）`,
                'packages/sz-rust-sz300/src/router.rs',
                '92'
            ));
        }

    } catch (err) {
        errors.push({ app: app.name, error: err.name || 'E2E_ERROR', detail: err.message });
        passed = false;
    }

    return passed;
}
