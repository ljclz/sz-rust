#!/usr/bin/env node
'use strict';

/**
 * Markdown link / version / feature-name consistency checker
 *
 * Complements doc-code-consistency.js (gate 19). Adds three checks:
 *   1. Link existence: every relative Markdown link in *.md must resolve to an existing file
 *      (or directory) in the repo. External http(s)/mailto/tel/anchor links are skipped.
 *   2. Version consistency: README.md / README.en.md "Current version" claim must equal
 *      [workspace.package] version in Cargo.toml.
 *   3. Feature-name consistency: feature names cited in docs as `--features <name>`
 *      must exist in at least one Cargo.toml [features] section in the workspace.
 *
 * Usage: node scripts/audit/link-check.js
 * Exit code: 0 = no ERROR, 1 = broken links / version mismatch / unknown feature found.
 *
 * Note (2026-10-27): on machines without node this script cannot run; it is wired into
 * .github/workflows/ci.yml so it executes in CI.
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function walkMd(dir) {
    let out = [];
    if (!fs.existsSync(dir)) return out;
    for (const entry of fs.readdirSync(dir)) {
        if (entry === 'target' || entry === 'node_modules' || entry === '.git' || entry === '.trae' || entry === 'archive') continue;
        const full = path.join(dir, entry);
        let st;
        try { st = fs.statSync(full); } catch (_) { continue; }
        if (st.isDirectory()) {
            out = out.concat(walkMd(full));
        } else if (entry.endsWith('.md')) {
            out.push(full);
        }
    }
    return out;
}

function workspaceVersion() {
    const manifest = fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8');
    const m = manifest.match(/\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
    return m ? m[1] : null;
}

function collectFeatures() {
    // Returns a Set of all feature names declared across workspace Cargo.toml files.
    const features = new Set();
    const walkDir = (dir) => {
        if (!fs.existsSync(dir)) return;
        for (const entry of fs.readdirSync(dir)) {
            if (entry === 'target' || entry === 'node_modules') continue;
            const full = path.join(dir, entry);
            let st;
            try { st = fs.statSync(full); } catch (_) { continue; }
            if (st.isDirectory()) {
                walkDir(full);
            } else if (entry === 'Cargo.toml') {
                const txt = fs.readFileSync(full, 'utf8');
                const inFeatures = txt.match(/\[features\]/);
                if (inFeatures) {
                    // Collect lines that look like `name = [...]` or `name = "..."` or `name = []`
                    const section = txt.slice(inFeatures.index);
                    for (const line of section.split('\n')) {
                        const fm = line.match(/^([a-zA-Z0-9_-]+)\s*=\s*(\[|"|\{)/);
                        if (fm) features.add(fm[1]);
                        if (line.startsWith('[') && line !== '[features]' && line.endsWith(']')) break;
                    }
                }
            }
        }
    };
    walkDir(path.join(ROOT, 'packages'));
    walkDir(ROOT); // root Cargo.toml features too
    return features;
}

function stripFragmentAndQuery(target) {
    return target.split('#')[0].split('?')[0];
}

function targetExists(target) {
    // Already checked: not a URL/anchor/mailto.
    if (target.startsWith('/')) {
        // Absolute repo-relative path (e.g. /docs/README.md) -> resolve from ROOT
        const p = path.join(ROOT, target.replace(/^\//, ''));
        return fs.existsSync(p);
    }
    return false; // caller resolves relative
}

function decodePercent(s) {
    try { return decodeURIComponent(s); } catch (_) { return s; }
}

// True if `index` lies inside a fenced code block (``` fence lines before it are odd).
function inFencedCodeBlock(content, index) {
    const prefix = content.slice(0, index);
    const fences = prefix.match(/^```/gm);
    return fences ? fences.length % 2 === 1 : false;
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

function checkLinks(files) {
    const errors = [];
    let totalLinks = 0;
    for (const file of files) {
        const content = fs.readFileSync(file, 'utf8');
        const dir = path.dirname(file);
        const re = /\[[^\]]*\]\(([^)]+)\)/g;
        let m;
        while ((m = re.exec(content)) !== null) {
            let target = m[1].trim();
            if (!target) continue;
            if (/^(https?|mailto|tel|file):/i.test(target)) continue;
            if (target.startsWith('#')) continue; // anchor
            // 跳过行内代码/代码块中的链接（如文档示例 `[English](README.en.md)`）
            const lineStart = content.lastIndexOf('\n', m.index) + 1;
            const linePrefix = content.slice(lineStart, m.index);
            const backtickCount = (linePrefix.match(/`/g) || []).length;
            if (backtickCount % 2 === 1) continue;
            if (inFencedCodeBlock(content, m.index)) continue;
            target = stripFragmentAndQuery(target);
            if (!target) continue;
            totalLinks++;
            const decoded = decodePercent(target);
            let exists = false;
            if (decoded.startsWith('/')) {
                exists = targetExists(decoded);
            } else {
                const resolved = path.resolve(dir, decoded);
                exists = fs.existsSync(resolved);
            }
            if (!exists) {
                const rel = path.relative(ROOT, file).replace(/\\/g, '/');
                errors.push(`${rel}: broken link -> ${target}`);
            }
        }
    }
    return { errors, totalLinks };
}

function checkVersions(files) {
    const errors = [];
    const actual = workspaceVersion();
    if (!actual) {
        errors.push('Cargo.toml: cannot find [workspace.package] version');
        return errors;
    }
    const targets = files.filter((f) => {
        const base = path.basename(f);
        return base === 'README.md' || base === 'README.en.md';
    });
    for (const file of targets) {
        const content = fs.readFileSync(file, 'utf8');
        // Chinese: "当前版本：v2.0.0"; English: "Current version v2.0.0"
        const re = /(?:当前版本[:：]\s*v|Current version\s+v)(\d+\.\d+\.\d+)/i;
        const m = content.match(re);
        if (m && m[1] !== actual) {
            const rel = path.relative(ROOT, file).replace(/\\/g, '/');
            errors.push(`${rel}: version claim "${m[1]}" != workspace version "${actual}"`);
        }
    }
    return errors;
}

function checkFeatures(files) {
    const errors = [];
    const known = collectFeatures();
    const seen = new Set();
    const re = /--features?\s+([a-zA-Z0-9_,-]+)/g;
    for (const file of files) {
        const content = fs.readFileSync(file, 'utf8');
        let m;
        while ((m = re.exec(content)) !== null) {
            if (inFencedCodeBlock(content, m.index)) continue;
            const chunk = m[1].split(',');
            for (let raw of chunk) {
                raw = raw.trim();
                if (!raw || raw === 'all' || raw === 'default') continue;
                // skip obviously-placeholder tokens
                if (/[{}]/.test(raw)) continue;
                if (seen.has(raw)) continue;
                seen.add(raw);
                if (!known.has(raw)) {
                    const rel = path.relative(ROOT, file).replace(/\\/g, '/');
                    errors.push(`${rel}: unknown feature "${raw}" not found in any workspace Cargo.toml [features]`);
                }
            }
        }
    }
    return errors;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function main() {
    const files = walkMd(ROOT);
    const linkResult = checkLinks(files);
    const versionErrors = checkVersions(files);
    const featureErrors = checkFeatures(files);

    console.log(`link-check: scanned ${files.length} .md files, ${linkResult.totalLinks} relative links`);
    for (const e of linkResult.errors) console.log(`  [LINK] ${e}`);
    for (const e of versionErrors) console.log(`  [VERSION] ${e}`);
    for (const e of featureErrors) console.log(`  [FEATURE] ${e}`);

    const allErrors = linkResult.errors.concat(versionErrors, featureErrors);
    if (allErrors.length > 0) {
        console.error(`\nlink-check: ${allErrors.length} issue(s) found`);
        process.exit(1);
    }
    console.log('\nlink-check: OK (all relative links resolve, versions consistent, features known)');
    process.exit(0);
}

if (require.main === module) {
    main();
}

module.exports = {
    ROOT,
    walkMd,
    checkLinks,
    checkVersions,
    checkFeatures,
};