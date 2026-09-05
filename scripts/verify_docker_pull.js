const { Client } = require('ssh2');
const fs = require('fs');
const path = require('path');

const privateKey = `-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDOrCHylYDXYOP0YBS+Ir7M5xnc1aPgf0dn7d3X9Gda3AAAAJjPWS0pz1kt
KQAAAAtzc2gtZWQyNTUxOQAAACDOrCHylYDXYOP0YBS+Ir7M5xnc1aPgf0dn7d3X9Gda3A
AAAEA4xgtubqt9HARi2/WKfFEtU8T3X5jPkusyG5SxCwgucc6sIfKVgNdg4/RgFL4ivszn
GdzVo+B/R2ft3df0Z1rcAAAAE3Jvb3RAVk0tMTYtMy11YnVudHUBAg==
-----END OPENSSH PRIVATE KEY-----`;

const reportLines = [];
const log = (s) => { reportLines.push(s); console.log(s); };

const conn = new Client();
conn.on('ready', async () => {
    const exec = (cmd, timeout = 300000) => new Promise((resolve, reject) => {
        conn.exec(cmd, (err, stream) => {
            if (err) { reject(err); return; }
            let out = '', err2 = '';
            stream.on('data', d => out += d.toString());
            stream.stderr.on('data', d => err2 += d.toString());
            stream.on('close', code => resolve({ out, err: err2, code }));
        });
    });

    try {
        log('# Docker 镜像构建验证报告');
        log('');
        log(`> 生成时间：${new Date().toISOString()}`);
        log('> 来源: release.yml run 33490337234 (tag v1.2.0-prod-hardening.4) Docker job success');
        log('> Image: ghcr.io/ljclz/sz300-server:latest');
        log('');

        log('## 1. 拉取 GHCR 镜像');
        const pull = await exec('docker pull ghcr.io/ljclz/sz300-server:latest 2>&1');
        log('```');
        log(pull.out.substring(0, 800));
        log('```');
        log(`- exit code: ${pull.code}`);
        log('');

        log('## 2. 镜像 inspect');
        const ex = await exec('docker image inspect ghcr.io/ljclz/sz300-server:latest --format "{{json .Config.ExposedPorts}}" 2>&1');
        log(`- ExposedPorts: ${ex.out.trim()}`);
        const has8300 = ex.out.includes('8300');
        log(`- 包含 8300: ${has8300 ? '✅' : '❌'}`);
        log('');

        log('## 3. Healthcheck');
        const hc = await exec('docker image inspect ghcr.io/ljclz/sz300-server:latest --format "{{json .Config.Healthcheck}}" 2>&1');
        log(`- Healthcheck: ${hc.out.trim()}`);
        const hasHc = hc.out.trim() !== 'null' && hc.out.trim() !== '';
        log(`- 有 healthcheck: ${hasHc ? '✅' : '❌'}`);
        log('');

        log('## 4. 镜像大小');
        const sz = await exec('docker image inspect ghcr.io/ljclz/sz300-server:latest --format "{{.Size}}" 2>&1');
        const sizeMB = (parseInt(sz.out.trim()) / 1024 / 1024).toFixed(2);
        log(`- 镜像大小: ${sizeMB} MB`);
        log('');

        log('## 5. RepoTags');
        const rt = await exec('docker image inspect ghcr.io/ljclz/sz300-server:latest --format "{{json .RepoTags}}" 2>&1');
        log(`- RepoTags: ${rt.out.trim()}`);
        log('');

        log('## 6. 清理');
        const rmi = await exec('docker rmi ghcr.io/ljclz/sz300-server:latest 2>&1');
        log(`- ${rmi.out.trim()}`);
        log('');

        log('## 7. 结论');
        log(`- Docker job (run 33490337234): ✅ success`);
        log(`- 镜像拉取: ${pull.code === 0 ? '✅' : '❌'}`);
        log(`- EXPOSE 8300: ${has8300 ? '✅' : '❌'}`);
        log(`- Healthcheck: ${hasHc ? '✅' : '❌'}`);
        log(`- 镜像大小: ${sizeMB} MB`);

        const reportDir = path.join(__dirname, '..', '.codeartsdoer', 'specs', 'prod_hardening_v1', 'reports');
        fs.writeFileSync(path.join(reportDir, 'docker_build_verification.md'), reportLines.join('\n'));
        log('');
        log('报告已写入 docker_build_verification.md');

        conn.end();
    } catch (err) {
        log(`[ERROR] ${err.message}`);
        conn.end();
        process.exit(1);
    }
});
conn.on('error', (err) => { console.error('SSH error:', err.message); process.exit(1); });
conn.connect({ host: '122.51.216.76', port: 22, username: 'root', privateKey, readyTimeout: 20000 });