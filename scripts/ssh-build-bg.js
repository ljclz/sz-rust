const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

conn.on('ready', () => {
    const cmd = `cd /www/rust/sz-rust-enterprise && nohup bash -c 'CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo build --tests -p sz-rust-sz300 --jobs 4 > /www/rust/build.log 2>&1; echo "BUILD_DONE exit=$?" >> /www/rust/build.log' & echo "started"`;
    conn.exec(cmd, (err, stream) => {
        if (err) { console.error(err); conn.end(); return; }
        let out = '';
        stream.on('data', (d) => out += d);
        stream.on('close', () => {
            console.log('Output:', out);
            conn.end();
        });
    });
}).on('error', (err) => console.error('SSH error:', err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});
