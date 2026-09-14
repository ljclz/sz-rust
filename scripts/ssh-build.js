const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

conn.on('ready', () => {
    console.log('SSH connected, building tests...');
    const cmd = 'cd /www/rust/sz-rust-enterprise && CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo build --tests -p sz-rust-sz300 --jobs 4 2>&1 | tail -20';
    conn.exec(cmd, (err, stream) => {
        if (err) { console.error(err); conn.end(); return; }
        let stdout = '', stderr = '';
        stream.on('data', (d) => stdout += d);
        stream.stderr.on('data', (d) => stderr += d);
        stream.on('close', (code) => {
            console.log(stdout);
            if (stderr) console.log('STDERR:', stderr);
            console.log(`Exit code: ${code}`);
            conn.end();
        });
    });
}).on('error', (err) => console.error('SSH error:', err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});