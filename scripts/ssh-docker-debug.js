const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = [
        'echo "=== Docker version ==="',
        'docker version --format "{{.Server.Version}}" 2>&1',
        'echo "=== MySQL image ==="',
        'docker images mysql 2>&1',
        'echo "=== Port 3306 ==="',
        'ss -tlnp | grep 3306 2>&1 || echo "port 3306 free"',
        'echo "=== Test: start MySQL container ==="',
        'time docker run -d --name test-mysql-cov -e MYSQL_ROOT_PASSWORD=test123 -e MYSQL_DATABASE=sz300_test -p 3306:3306 mysql:9.6 2>&1',
        'echo "=== Wait for ready ==="',
        'for i in $(seq 1 30); do if docker logs test-mysql-cov 2>&1 | grep -q "ready for connections"; then echo "Ready after ${i}s"; break; fi; sleep 1; done',
        'echo "=== Container status ==="',
        'docker ps -a --filter "name=test-mysql-cov" 2>&1',
        'echo "=== Cleanup ==="',
        'docker rm -f test-mysql-cov 2>&1',
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