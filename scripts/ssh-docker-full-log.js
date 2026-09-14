const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = [
        'docker run -d --name test-mysql-full -e MYSQL_ROOT_PASSWORD=test123 -e MYSQL_DATABASE=sz300_test -p 3307:3306 mysql:9.6 2>&1',
        'sleep 15',
        'echo "=== FULL STDERR ==="',
        'docker logs test-mysql-full 2>&1 1>/dev/null',
        'echo "=== FULL STDOUT ==="',
        'docker logs test-mysql-full 2>/dev/null',
        'echo "=== Cleanup ==="',
        'docker rm -f test-mysql-full 2>&1',
    ].join(' && ');
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