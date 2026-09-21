const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

const commands = [
    'echo "=== sz-rust ===" && ls /www/rust/sz-rust/ 2>&1',
    'echo "=== sz-rust packages ===" && ls /www/rust/sz-rust/packages/ 2>&1',
    'echo "=== sz300 ===" && ls /www/rust/sz300/ 2>&1',
    'echo "=== sz-rust-new ===" && ls /www/rust/sz-rust-new/ 2>&1',
    'echo "=== Docker MySQL 可用性 ===" && docker pull mysql:9.6 --quiet 2>&1 && echo "MySQL image ready"',
    'echo "=== 磁盘空间 ===" && df -h /www 2>&1',
];

conn.on('ready', () => {
    let cmdIndex = 0;
    const runNext = () => {
        if (cmdIndex >= commands.length) { conn.end(); return; }
        const cmd = commands[cmdIndex++];
        conn.exec(cmd, (err, stream) => {
            if (err) { console.error(err); runNext(); return; }
            let stdout = '', stderr = '';
            stream.on('data', (d) => stdout += d);
            stream.stderr.on('data', (d) => stderr += d);
            stream.on('close', () => {
                console.log(stdout);
                if (stderr) console.log('STDERR:', stderr);
                runNext();
            });
        });
    };
    runNext();
}).on('error', (err) => console.error('SSH error:', err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});