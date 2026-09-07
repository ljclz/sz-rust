const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

const commands = [
    'echo "=== sz300 src ===" && ls /www/rust/sz-rust/packages/sz-rust-sz300/src/ 2>&1',
    'echo "=== sz300 tests ===" && ls /www/rust/sz-rust/packages/sz-rust-sz300/tests/ 2>&1',
    'echo "=== sz300 Cargo.toml ===" && head -5 /www/rust/sz-rust/packages/sz-rust-sz300/Cargo.toml 2>&1',
    'echo "=== git log ===" && cd /www/rust/sz-rust && git log --oneline -3 2>&1',
    'echo "=== lib.rs exists ===" && test -f /www/rust/sz-rust/packages/sz-rust-sz300/src/lib.rs && echo "YES" || echo "NO"',
    'echo "=== bootstrap.rs exists ===" && test -f /www/rust/sz-rust/packages/sz-rust-sz300/src/bootstrap.rs && echo "YES" || echo "NO"',
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