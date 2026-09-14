const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = 'echo "=== Disk usage ==="; du -sh /www/rust/sz-rust-enterprise-target 2>&1; du -sh /www/rust/sz-rust-enterprise 2>&1; echo "=== Docker images ==="; docker images 2>&1; echo "=== Cleaning target ==="; rm -rf /www/rust/sz-rust-enterprise-target/llvm-cov-target 2>&1; echo "=== After cleanup ==="; df -h /www 2>&1';
    conn.exec(cmd, (err, stream) => {
        if (err) { console.error(err); conn.end(); return; }
        let out = '';
        stream.on('data', (d) => { out += d; });
        stream.stderr.on('data', (d) => { out += d; });
        stream.on('close', () => { console.log(out); conn.end(); });
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
    readyTimeout: 30000,
});