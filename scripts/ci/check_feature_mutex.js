#!/usr/bin/env node
// v1.7.0 feature gate 互斥组检查
// 用法: node scripts/ci/check_feature_mutex.js
const { execSync } = require('child_process');

const MUTEX_GROUPS = [
    { name: 'Z-zerocopy', items: ['perf-zerocopy-rkyv', 'perf-zerocopy-zerocopy'], message: '零拷贝后端互斥：rkyv 与 zerocopy 不可同时启用' },
    { name: 'S-upload-storage', items: ['api-upload-local', 'api-upload-s3', 'api-upload-minio', 'api-upload-oss'], message: '文件上传存储后端互斥：local/s3/minio/oss 仅可选其一' },
];

function getEnabledFeatures() {
    try {
        const output = execSync('cargo metadata --format-version 1 --no-deps 2>nul', { encoding: 'utf-8', maxBuffer: 1024 * 1024 * 100 });
        const meta = JSON.parse(output);
        const rootPkg = meta.packages.find(p => p.name === 'sz-rust');
        if (!rootPkg) return [];
        return Object.keys(rootPkg.features || {}).filter(f => f !== 'default');
    } catch {
        return [];
    }
}

const allFeatures = getEnabledFeatures();
let hasViolation = false;

for (const group of MUTEX_GROUPS) {
    const enabled = group.items.filter(f => allFeatures.includes(f));
    if (enabled.length > 1) {
        console.error(`[MUTEX VIOLATION] ${group.name}: ${enabled.join(', ')} — ${group.message}`);
        hasViolation = true;
    }
}

if (hasViolation) {
    process.exit(1);
} else {
    console.log('[OK] feature gate 互斥组检查通过');
    process.exit(0);
}