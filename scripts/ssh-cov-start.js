const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    conn.exec(`cd /www/rust/sz-rust-enterprise && nohup bash -c 'CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo llvm-cov -p sz-rust-sz300 --lib --summary-only -- --include-ignored --test-threads=1 > /www/rust/cov.log 2>&1; echo "COV_DONE exit=$?" >> /www/rust/cov.log' &`, () => {
        console.log('started');
        conn.end();
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});
