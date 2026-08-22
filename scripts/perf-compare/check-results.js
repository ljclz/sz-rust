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

    const files = await exec('ls -t /www/rust/perf-compare/results-*.json 2>/dev/null | head -3');
    console.log('Results files:', files);

    const raw = await exec('ls -t /www/rust/perf-compare/raw-results-*.jsonl 2>/dev/null | head -3');
    console.log('Raw files:', raw);

    const latest = files.trim().split('\n')[0];
    if (latest) {
        const size = await exec(`wc -c ${latest}`);
        console.log('File size:', size.trim());

        const okCount = await exec(`grep -c '"status": "ok"' ${latest}`);
        console.log('OK count:', okCount.trim());

        const naCount = await exec(`grep -c '"status": "N/A"' ${latest}`);
        console.log('N/A count:', naCount.trim());
    }

    const latestRaw = raw.trim().split('\n')[0];
    if (latestRaw) {
        const lines = await exec(`wc -l ${latestRaw}`);
        console.log('Raw lines:', lines.trim());
    }

    client.end();
});

client.on('error', (err) => {
    console.error('SSH error:', err.message);
});

client.connect({
    host: '122.51.216.76',
    port: 22,
    username: 'root',
    privateKey: fs.readFileSync(KEY_PATH, 'utf-8'),
});