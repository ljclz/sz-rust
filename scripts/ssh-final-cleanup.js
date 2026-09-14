const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = 'docker ps -a --filter "ancestor=mysql:9.6" -q 2>&1 | xargs -r docker rm -f 2>&1; rm -f /www/rust/cov-final.log /www/rust/cov-all.log /www/rust/test-fix.log /www/rust/test-fix2.log /www/rust/test-fix3.log /www/rust/test-single.log /www/rust/test-all-success.log /www/rust/build.log /www/rust/cov.log /www/rust/cov2.log /www/rust/cov3.log 2>&1; echo "Server cleaned"; df -h /www 2>&1';
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