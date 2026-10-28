#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
// 错误码冲突检测 CI 门禁脚本
// 扫描所有 Rust 源码中的 error_code() 返回值，检测冲突

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const PACKAGES_DIR = path.join(ROOT, 'packages');

const errorCodeMap = new Map();
let conflicts = [];
let totalCodes = 0;

function scanFile(filePath) {
    const content = fs.readFileSync(filePath, 'utf8');
    const lines = content.split('\n');

    // Match patterns like: "AI_PROVIDER_UNAVAILABLE" or 'CODEGEN_PARSE' in error_code() methods
    const pattern = /error_code\(\)\s*->\s*&'static\s+str|"(AI_[A-Z_]+|CODEGEN_[A-Z_]+|HTTP_[A-Z_]+|ORM_[A-Z_]+|CONFIG_[A-Z_]+)"|'(AI_[A-Z_]+|CODEGEN_[A-Z_]+|HTTP_[A-Z_]+|ORM_[A-Z_]+|CONFIG_[A-Z_]+)'/g;

    // Simpler approach: find all quoted error code strings
    const codePattern = /"((?:AI|CODEGEN|HTTP|ORM|CONFIG|CACHE|AUTH|EVENT|QUEUE|SERVICE|DX|AGENT|RAG|EMBED|VECTOR|TOOL|MCP|RATE|CONTEXT|LOCAL)_[A-Z_]+)"/g;

    let match;
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        let m;
        while ((m = codePattern.exec(line)) !== null) {
            const code = m[1];
            const relativePath = path.relative(ROOT, filePath);
            const location = `${relativePath}:${i + 1}`;

            if (errorCodeMap.has(code)) {
                const existing = errorCodeMap.get(code);
                const existingCrate = existing.file.split(path.sep)[1] || '';
                const newCrate = relativePath.split(path.sep)[1] || '';
                if (existingCrate !== newCrate) {
                    conflicts.push({
                        code,
                        locations: [existing, { file: relativePath, line: i + 1 }],
                    });
                }
            } else {
                errorCodeMap.set(code, { file: relativePath, line: i + 1 });
                totalCodes++;
            }
        }
    }
}

function scanDirectory(dir) {
    if (!fs.existsSync(dir)) return;

    const entries = fs.readdirSync(dir, { withFileTypes: true });
    for (const entry of entries) {
        const fullPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            scanDirectory(fullPath);
        } else if (entry.isFile() && entry.name.endsWith('.rs')) {
            scanFile(fullPath);
        }
    }
}

console.log('▶ Scanning for error code conflicts...');
scanDirectory(PACKAGES_DIR);

console.log(`\n📊 Total unique error codes: ${totalCodes}`);
console.log(`📊 Conflicts found: ${conflicts.length}`);

if (conflicts.length > 0) {
    console.log('\n❌ Error code conflicts detected:');
    for (const c of conflicts) {
        console.log(`  "${c.code}":`);
        for (const loc of c.locations) {
            console.log(`    - ${loc.file}:${loc.line}`);
        }
    }
    process.exit(1);
} else {
    console.log('\n✅ No error code conflicts detected.');
    process.exit(0);
}