const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = 'docker ps -a --filter "ancestor=mysql:9.6" -q 2>&1 | xargs -r docker rm -f 2>&1; nohup bash -c \'cd /www/rust/sz-rust-enterprise && CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo test -p sz-rust-sz300 --test success_path_test -- --include-ignored --test-threads=1\' > /www/rust/test-all-success.log 2>&1 & echo "PID: $!"';
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