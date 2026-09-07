const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();

const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

const commands = [
    'echo "=== OS ===" && uname -a',
    'echo "=== Docker ===" && docker --version 2>&1 && docker ps 2>&1',
    'echo "=== Rust ===" && rustc --version 2>&1 && cargo --version 2>&1',
    'echo "=== Rust 目录 ===" && ls /www/rust/ 2>&1',
    'echo "=== sz-rust-enterprise ===" && ls /www/rust/sz-rust-enterprise/ 2>&1',
    'echo "=== llvm-cov ===" && cargo llvm-cov --version 2>&1',
];

conn.on('ready', () => {
    console.log('SSH connected');
    let cmdIndex = 0;
    const runNext = () => {
        if (cmdIndex >= commands.length) {
            conn.end();
            return;
        }
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
}).on('error', (err) => {
    console.error('SSH error:', err);
}).connect({
    host: '122.51.216.76',
    port: 22,
    username: 'root',
    privateKey: privateKey,
});
