import fs from 'fs';
import path from 'path';

const toml = fs.readFileSync('Cargo.toml', 'utf-8');
const m = toml.match(/members\s*=\s*\[([\s\S]*?)\]/);
if (m) {
    const items = m[1].match(/"[^"]+"/g);
    console.log('workspace members count:', items.length);
    items.forEach(i => console.log('  ', i.replace(/"/g, '')));
}

const adrFiles = fs.readdirSync('docs/adr').filter(f => f.endsWith('.md') && !f.startsWith('README'));
console.log('ADR count:', adrFiles.length);

function countLines(dir) {
    let total = 0;
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
        if (e.name === 'target' || e.name === 'node_modules' || e.name === '.git') continue;
        const p = path.join(dir, e.name);
        if (e.isDirectory()) total += countLines(p);
        else if (e.name.endsWith('.rs')) {
            const c = fs.readFileSync(p, 'utf-8').split('\n').filter(l => l.trim() && !l.trim().startsWith('//')).length;
            total += c;
        }
    }
    return total;
}
console.log('Rust code lines (excl empty/comment):', countLines('packages'));

let testCount = 0;
function walk(dir) {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
        if (e.name === 'target' || e.name === 'node_modules' || e.name === '.git') continue;
        const p = path.join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith('.rs')) {
            const c = fs.readFileSync(p, 'utf-8').split('\n').filter(l => l.includes('#[test]')).length;
            testCount += c;
        }
    }
}
walk('packages');
console.log('Test functions:', testCount);

const integTests = fs.readdirSync('tests').filter(f => f.endsWith('.rs'));
console.log('Integration test files:', integTests.length);

const benchFiles = [];
function findBench(dir) {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
        if (e.name === 'target' || e.name === 'node_modules') continue;
        const p = path.join(dir, e.name);
        if (e.isDirectory()) findBench(p);
        else if (e.name.endsWith('.rs')) {
            const c = fs.readFileSync(p, 'utf-8');
            if (c.includes('#[bench]')) benchFiles.push(p);
        }
    }
}
findBench('packages');
console.log('Bench files:', benchFiles.length);