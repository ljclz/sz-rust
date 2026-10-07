#!/usr/bin/env node
'use strict';
/**
 * v1.9.0 N+1 检测门禁脚本（spec §5.8.1 规则 4/6）
 *
 * 用法：
 *   node scripts/nplus1-detect.js                    # 扫描 sz300 src，0 违规退出码 0
 *   node scripts/nplus1-detect.js --dir <path>       # 扫描指定目录
 *   node scripts/nplus1-detect.js --json             # 输出 JSON 格式
 *
 * 检测规则：
 *   循环体（for / while / loop）内出现以下调用视为 N+1 违规：
 *     - fetch_related
 *     - query_with_params
 *     - conn.query
 *     - pool.acquire（循环内逐条获取连接）
 *
 * 禁止项（spec §5.8.1 规则 6）：热路径含 N+1 → CI 阻塞
 */
const fs = require('fs');
const path = require('path');

const DEFAULT_SCAN_DIR = path.resolve(__dirname, '..', 'packages', 'sz-rust-sz300', 'src');
const NPLUS1_PATTERNS = [
    'fetch_related',
    'query_with_params',
    'conn.query',
    'pool.acquire',
];
const LOOP_KEYWORDS = /^\s*(for\s+\w+.*\bin\b|while\b|loop\b)/;

function parseArgs(argv) {
    const args = { dir: DEFAULT_SCAN_DIR, json: false };
    for (let i = 2; i < argv.length; i++) {
        if (argv[i] === '--dir') args.dir = argv[++i];
        else if (argv[i] === '--json') args.json = true;
    }
    return args;
}

function listRustFiles(dir) {
    const result = [];
    if (!fs.existsSync(dir)) return result;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const fullPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            result.push(...listRustFiles(fullPath));
        } else if (entry.name.endsWith('.rs')) {
            result.push(fullPath);
        }
    }
    return result;
}

function findLoopRanges(lines) {
    const loops = [];
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        const match = line.match(LOOP_KEYWORDS);
        if (!match) continue;
        const keyword = line.trim().startsWith('for') ? 'for'
            : line.trim().startsWith('while') ? 'while'
                : 'loop';
        let braceStart = -1;
        for (let j = i; j < Math.min(i + 5, lines.length); j++) {
            const idx = lines[j].indexOf('{');
            if (idx !== -1) {
                braceStart = j;
                break;
            }
        }
        if (braceStart === -1) continue;

        let depth = 0;
        let endLine = -1;
        for (let j = braceStart; j < lines.length; j++) {
            for (const ch of lines[j]) {
                if (ch === '{') depth++;
                else if (ch === '}') {
                    depth--;
                    if (depth === 0) {
                        endLine = j;
                        break;
                    }
                }
            }
            if (endLine !== -1) break;
        }
        if (endLine !== -1) {
            loops.push({ startLine: i, endLine, keyword });
        }
    }
    return loops;
}

function detectNPlus1(filePath) {
    const content = fs.readFileSync(filePath, 'utf8');
    const lines = content.split('\n');
    const loops = findLoopRanges(lines);
    const violations = [];

    for (const loop of loops) {
        for (let i = loop.startLine; i <= loop.endLine; i++) {
            const line = lines[i];
            for (const pattern of NPLUS1_PATTERNS) {
                if (line.includes(pattern)) {
                    const trimmed = line.trim();
                    if (trimmed.startsWith('//') || trimmed.startsWith('*') || trimmed.startsWith('/*')) continue;
                    violations.push({
                        file: filePath,
                        line: i + 1,
                        pattern,
                        loop_keyword: loop.keyword,
                        loop_start: loop.startLine + 1,
                        snippet: trimmed.substring(0, 120),
                    });
                }
            }
        }
    }
    return violations;
}

function main() {
    const args = parseArgs(process.argv);
    const files = listRustFiles(args.dir);

    if (files.length === 0) {
        console.error(`[nplus1-detect] FAIL: 未找到 .rs 文件，扫描目录: ${args.dir}`);
        process.exit(2);
    }

    const allViolations = [];
    for (const file of files) {
        const violations = detectNPlus1(file);
        allViolations.push(...violations);
    }

    if (args.json) {
        console.log(JSON.stringify({ violations: allViolations, count: allViolations.length, scanned_files: files.length }, null, 2));
    } else {
        console.log(`[nplus1-detect] 扫描 ${files.length} 个 .rs 文件 (${args.dir})`);
        if (allViolations.length === 0) {
            console.log('[nplus1-detect] PASS: 0 违规');
        } else {
            console.log(`[nplus1-detect] FAIL: 发现 ${allViolations.length} 处 N+1 违规\n`);
            console.log('| 文件 | 行号 | 模式 | 循环 | 代码片段 |');
            console.log('|------|------|------|------|----------|');
            for (const v of allViolations) {
                const relPath = path.relative(path.resolve(__dirname, '..'), v.file);
                console.log(`| ${relPath} | ${v.line} | ${v.pattern} | ${v.loop_keyword} | ${v.snippet} |`);
            }
        }
    }

    process.exit(allViolations.length > 0 ? 1 : 0);
}

main();