const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    console.log('Starting coverage with --no-fail-fast...');
    conn.exec(`cd /www/rust/sz-rust-enterprise && nohup bash -c 'CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo llvm-cov -p sz-rust-sz300 --summary-only --no-fail-fast --ignore-filename-regex "main.rs" -- --include-ignored --test-threads=1 --skip bin_e2e_test > /www/rust/cov3.log 2>&1; echo "COV3_DONE exit=$?" >> /www/rust/cov3.log' &`, () => {
        console.log('started');
        conn.end();
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});