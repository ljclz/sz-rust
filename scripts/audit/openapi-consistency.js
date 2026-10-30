#!/usr/bin/env node
'use strict';

/**
 * OpenAPI-路由一致性审计脚本（v1.8.0 P5-5.3）
 *
 * 对比 `src/router.rs` 中定义的路由与 `src/openapi.rs` 中注册的 OpenAPI 端点：
 *   - ERROR：路由存在但 OpenAPI spec 缺失（前端无法发现该端点）
 *   - WARN ：OpenAPI spec 存在但路由未定义（幽灵端点）
 *
 * 排除规则：
 *   - /api-docs* 端点（文档自身，不需要出现在 spec 中）
 *   - 通配路由 /{*path}（文件服务回退）
 *   - 嵌套路由通过 .merge() / .nest() 引入的（需手动标注）
 *
 * 用法：node scripts/audit/openapi-consistency.js
 * 退出码：0 = 一致，1 = 存在 ERROR
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const ROUTER_FILE = path.join(ROOT, 'packages', 'sz-rust-sz300', 'src', 'router.rs');
const OPENAPI_FILE = path.join(ROOT, 'packages', 'sz-rust-sz300', 'src', 'openapi.rs');

const EXCLUDE_PATTERNS = [
    /^\/api-docs/,
    /^\/\{\*path\}$/,
    /^\/uploads\//,
    /^\/$/,
];

function extractRoutes(content) {
    const routes = new Set();
    const routeRegex = /\.route\(\s*["']([^"']+)["']/g;
    let match;
    while ((match = routeRegex.exec(content)) !== null) {
        const routePath = match[1];
        if (!EXCLUDE_PATTERNS.some(p => p.test(routePath))) {
            routes.add(routePath);
        }
    }
    return routes;
}

function extractOpenApiPaths(content) {
    const paths = new Set();
    const pathRegex = /\.path\(\s*["']([^"']+)["']/g;
    let match;
    while ((match = pathRegex.exec(content)) !== null) {
        paths.add(match[1]);
    }
    return paths;
}

function main() {
    if (!fs.existsSync(ROUTER_FILE)) {
        console.error(`ERROR: 路由文件不存在: ${ROUTER_FILE}`);
        process.exit(1);
    }
    if (!fs.existsSync(OPENAPI_FILE)) {
        console.error(`ERROR: OpenAPI 文件不存在: ${OPENAPI_FILE}`);
        process.exit(1);
    }

    const routerContent = fs.readFileSync(ROUTER_FILE, 'utf-8');
    const openapiContent = fs.readFileSync(OPENAPI_FILE, 'utf-8');

    const routerPaths = extractRoutes(routerContent);
    const openapiPaths = extractOpenApiPaths(openapiContent);

    const missingInOpenApi = [...routerPaths].filter(p => !openapiPaths.has(p)).sort();
    const missingInRouter = [...openapiPaths].filter(p => !routerPaths.has(p)).sort();

    let hasError = false;

    console.log('=== OpenAPI-路由一致性审计 ===');
    console.log(`路由定义: ${routerPaths.size} 个端点`);
    console.log(`OpenAPI spec: ${openapiPaths.size} 个端点`);
    console.log('');

    if (missingInOpenApi.length > 0) {
        hasError = true;
        console.error(`ERROR: 路由存在但 OpenAPI spec 缺失 (${missingInOpenApi.length} 个):`);
        for (const p of missingInOpenApi) {
            console.error(`  - ${p}`);
        }
        console.error('');
    }

    if (missingInRouter.length > 0) {
        console.warn(`WARN: OpenAPI spec 存在但路由未定义 (${missingInRouter.length} 个):`);
        for (const p of missingInRouter) {
            console.warn(`  - ${p}`);
        }
        console.warn('');
    }

    if (!hasError && missingInRouter.length === 0) {
        console.log('PASS: 路由与 OpenAPI spec 完全一致');
    }

    process.exit(hasError ? 1 : 0);
}

main();