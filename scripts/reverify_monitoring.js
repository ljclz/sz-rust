const { Client } = require('ssh2');
const privateKey = `-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDOrCHylYDXYOP0YBS+Ir7M5xnc1aPgf0dn7d3X9Gda3AAAAJjPWS0pz1kt
KQAAAAtzc2gtZWQyNTUxOQAAACDOrCHylYDXYOP0YBS+Ir7M5xnc1aPgf0dn7d3X9Gda3A
AAAEA4xgtubqt9HARi2/WKfFEtU8T3X5jPkusyG5SxCwgucc6sIfKVgNdg4/RgFL4ivszn
GdzVo+B/R2ft3df0Z1rcAAAAE3Jvb3RAVk0tMTYtMy11YnVudHUBAg==
-----END OPENSSH PRIVATE KEY-----`;
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