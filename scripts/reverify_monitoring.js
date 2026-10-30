const { Client } = require('ssh2');
const privateKey = `process.env.SSH_PRIVATE_KEY`;
const conn = new Client();
conn.on('ready', async () => {
    const exec = (cmd) => new Promise((resolve) => {
        conn.exec(cmd, (err, stream) => {
            let out = '';
            stream.on('data', d => out += d);
            stream.stderr.on('data', d => out += d);
            stream.on('close', () => resolve(out.trim()));
        });
    });
    console.log('## 监控栈重新验证');
    console.log('1. containers:', await exec('docker ps --format "{{.Names}} {{.Status}}" | grep -E "prom|grafana|alert"'));
    console.log('2. ports:', await exec('ss -tlnp | grep -E ":(9090|3000|9093)"'));
    console.log('3. prometheus:', await exec('curl -s --connect-timeout 5 http://127.0.0.1:9090/-/healthy'));
    console.log('4. grafana:', await exec('curl -s --connect-timeout 5 -u admin:admin http://127.0.0.1:3000/api/health'));
    console.log('5. alertmanager:', await exec('curl -s --connect-timeout 5 http://127.0.0.1:9093/-/healthy'));
    const pub = await exec('ss -tlnp | grep "0.0.0.0" | grep -E ":(9090|3000|9093)"');
    console.log('6. public_binding:', pub || 'none (all 127.0.0.1)');
    conn.end();
});
conn.on('error', e => console.error('SSH error:', e.message));
conn.connect({ host: '122.51.216.76', port: 22, username: 'root', privateKey, readyTimeout: 20000 });