const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = 'cd /www/rust/sz-rust-enterprise && CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo llvm-cov -p sz-rust-sz300 --summary-only --no-fail-fast --failure-mode all --ignore-filename-regex "main.rs" -- --include-ignored --test-threads=1 2>&1';
    console.log('Running coverage test with --failure-mode all...');
    conn.exec(cmd, (err, stream) => {
        if (err) { console.error(err); conn.end(); return; }
        let out = '';
        stream.on('data', (d) => { out += d; });
        stream.stderr.on('data', (d) => { out += d; });
        stream.on('close', (code) => {
            console.log('Exit code:', code);
            console.log(out);
            conn.end();
        });
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
    readyTimeout: 30000,
});
