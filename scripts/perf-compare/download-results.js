import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';

const KEY_PATH = path.resolve(import.meta.dirname, '..', '..', 'deploy_key');
const client = new Client();

client.on('ready', async () => {
    const exec = (cmd) => new Promise((resolve) => {
        let out = '';
        client.exec(cmd, (err, stream) => {
            if (err) { resolve(''); return; }
            stream.on('data', d => out += d.toString());
            stream.stderr.on('data', d => out += d.toString());
            stream.on('close', () => resolve(out));
        });
    });

    const rawFile = '/www/rust/perf-compare/raw-results-2026-08-09T23-11-43.jsonl';
    console.log('Downloading raw results...');

    const content = await exec(`cat ${rawFile}`);
    const lines = content.trim().split('\n').filter(l => l.trim());

    console.log(`Total lines: ${lines.length}`);

    const results = [];
    let okCount = 0, naCount = 0;

    for (const line of lines) {
        try {
            const r = JSON.parse(line);
            results.push(r);
            if (r.status === 'ok') okCount++;
            else if (r.status === 'N/A') naCount++;
        } catch (e) {
            console.error('Parse error:', e.message);
        }
    }

    console.log(`OK: ${okCount}, N/A: ${naCount}, Total: ${results.length}`);

    const summary = {
        timestamp: new Date().toISOString(),
        wrkDuration: 10,
        results,
    };

    const localPath = path.resolve(import.meta.dirname, 'results-v070.json');
    fs.writeFileSync(localPath, JSON.stringify(summary, null, 2), 'utf-8');
    console.log(`Saved to: ${localPath}`);

    for (const r of results) {
        if (r.status === 'ok') {
            console.log(`  ${r.framework} ${r.route} c=${r.concurrency}: RPS=${r.rps} P99=${r.p99}`);
        } else {
            console.log(`  ${r.framework} ${r.route} c=${r.concurrency}: ${r.status} (${r.reason})`);
        }
    }

    client.end();
});

client.connect({
    host: '122.51.216.76',
    port: 22,
    username: 'root',
    privateKey: fs.readFileSync(KEY_PATH, 'utf-8'),
});