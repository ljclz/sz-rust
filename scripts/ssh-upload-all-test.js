const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

const files = [
    {
        local: 'E:/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/common/db_fixture.rs',
        remote: '/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/common/db_fixture.rs'
    },
    {
        local: 'E:/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/common/request.rs',
        remote: '/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/common/request.rs'
    },
    {
        local: 'E:/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/success_path_test.rs',
        remote: '/www/rust/sz-rust-enterprise/packages/sz-rust-sz300/tests/success_path_test.rs'
    }
];

conn.on('ready', () => {
    console.log('Uploading', files.length, 'files...');
    conn.sftp((err, sftp) => {
        if (err) { console.error(err); conn.end(); return; }
        let uploaded = 0;
        files.forEach(f => {
            const data = fs.readFileSync(f.local);
            const ws = sftp.createWriteStream(f.remote);
            ws.on('close', () => {
                uploaded++;
                if (uploaded === files.length) {
                    console.log('All files uploaded. Running test...');
                    const cmd = 'docker ps -a --filter "ancestor=mysql:9.6" -q 2>&1 | xargs -r docker rm -f 2>&1; cd /www/rust/sz-rust-enterprise && CARGO_TARGET_DIR=/www/rust/sz-rust-enterprise-target cargo test -p sz-rust-sz300 --test success_path_test device_list_returns_owned_devices -- --include-ignored --test-threads=1 2>&1 | tail -20';
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
                }
            });
            ws.write(data);
            ws.end();
        });
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
    readyTimeout: 30000,
});