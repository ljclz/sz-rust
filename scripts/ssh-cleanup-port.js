const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = [
        'echo "=== Containers using port 3306 ==="',
        'docker ps -a --filter "publish=3306" 2>&1',
        'echo "=== All mysql containers ==="',
        'docker ps -a --filter "ancestor=mysql:9.6" 2>&1',
        'echo "=== Cleanup all mysql containers ==="',
        'docker ps -a --filter "ancestor=mysql:9.6" -q 2>&1 | xargs -r docker rm -f 2>&1',
        'docker ps -a --filter "publish=3306" -q 2>&1 | xargs -r docker rm -f 2>&1',
        'echo "=== Check port 3306 ==="',
        'ss -tlnp | grep 3306 2>&1 || echo "port 3306 free"',
        'echo "=== Docker ps ==="',
        'docker ps -a 2>&1',
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