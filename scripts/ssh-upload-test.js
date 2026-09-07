const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

const localFile = fs.readFileSync('E:/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/common/db_fixture.rs');

conn.on('ready', () => {
    console.log('Uploading db_fixture.rs...');
    conn.sftp((err, sftp) => {
        if (err) { console.error(err); conn.end(); return; }
        const remotePath = '/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/common/db_fixture.rs';
        const writeStream = sftp.createWriteStream(remotePath);
        writeStream.on('close', () => {
            console.log('Upload done. Running success_path_test...');
            conn.exec('cd /www/rust/sz-rust-enterprise && CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo test -p sz-rust-sz300 --test success_path_test -- --include-ignored --test-threads=1 2>&1 | tail -30', (err, stream) => {
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
        });
        writeStream.write(localFile);
        writeStream.end();
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
    readyTimeout: 30000,
});