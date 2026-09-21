const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');

conn.on('ready', () => {
    console.log('SSH connected, uploading via SFTP...');
    conn.sftp((err, sftp) => {
        if (err) { console.error('SFTP error:', err); conn.end(); return; }

        const localPath = 'E:\\www\\rust\\sz-rust-enterprise.tar.gz';
        const remotePath = '/www/rust/sz-rust-enterprise.tar.gz';

        const readStream = fs.createReadStream(localPath);
        const writeStream = sftp.createWriteStream(remotePath);

        writeStream.on('close', () => {
            console.log('Upload complete');

            // 解压并设置环境
            const commands = [
                'mkdir -p /www/rust/sz-rust-enterprise',
                'tar -xzf /www/rust/sz-rust-enterprise.tar.gz -C /www/rust/sz-rust-enterprise',
                'ls /www/rust/sz-rust-enterprise/packages/',
                'echo "=== Installing cargo-llvm-cov ===" && cargo install cargo-llvm-cov 2>&1 | tail -5',
            ];

            let cmdIndex = 0;
            const runNext = () => {
                if (cmdIndex >= commands.length) {
                    console.log('All done');
                    conn.end();
                    return;
                }
                const cmd = commands[cmdIndex++];
                console.log(`Running: ${cmd.substring(0, 80)}...`);
                conn.exec(cmd, (err, stream) => {
                    if (err) { console.error(err); runNext(); return; }
                    let stdout = '', stderr = '';
                    stream.on('data', (d) => stdout += d);
                    stream.stderr.on('data', (d) => stderr += d);
                    stream.on('close', (code) => {
                        console.log(stdout);
                        if (stderr) console.log('STDERR:', stderr);
                        console.log(`Exit code: ${code}`);
                        runNext();
                    });
                });
            };
            runNext();
        });

        writeStream.on('error', (err) => {
            console.error('Write error:', err);
            conn.end();
        });

        readStream.pipe(writeStream);
    });
}).on('error', (err) => console.error('SSH error:', err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});