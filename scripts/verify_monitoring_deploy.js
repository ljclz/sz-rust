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

const conn = new Client();
const sshExec = (cmd, timeout = 30000) => new Promise((resolve, reject) => {
  conn.exec(cmd, (err, stream) => {
    if (err) { reject(err); return; }
    let stdout = '', stderr = '';
    stream.on('data', (d) => stdout += d.toString());
    stream.stderr.on('data', (d) => stderr += d.toString());
    stream.on('close', (code) => resolve({ stdout, stderr, code }));
  });
});

const sftpUpload = (sftp, localPath, remotePath) => new Promise((resolve, reject) => {
  sftp.fastPut(localPath, remotePath, (err) => { if (err) reject(err); else resolve(); });
});

const reportLines = [];
const log = (s) => { reportLines.push(s); console.log(s); };

conn.on('ready', async () => {
  try {
    const sftp = new Promise((resolve, reject) => {
      conn.sftp((err, s) => { if (err) reject(err); else resolve(s); });
    });
    const sftpConn = await sftp;

    // 1. 上传新 prometheus.yml
    log('## 1. 上传新 prometheus.yml（含 bearer_token）');
    await sftpUpload(sftpConn, path.join(__dirname, '..', 'deploy', 'monitoring', 'prometheus.yml'), '/www/rust/sz-rust-new/deploy/monitoring/prometheus.yml');
    log('- 上传完成 ✅');

    // 2. 上传新启动脚本（含 bearer token）
    log('## 2. 上传新启动脚本');
    const startScript = `#!/bin/bash
cd /www/rust/sz-rust-new
export SZ_JWT_SECRET=liujieclz_test_secret_key_2026_extra_secure_32bytes
export SZ300_JWT_SECRET=liujieclz_test_secret_key_2026_extra_secure_32bytes
export SZ300_DB_PASSWORD=2sJ8BcTdSxWNN4fp
export SZ300_DB_HOST=127.0.0.1
export SZ300_DB_PORT=8802
export SZ300_DB_USER=test
export SZ300_DB_NAME=test
export RUST_LOG=info
export SZ300_METRICS_BEARER_TOKEN=prometheus-scrape-token
export SZ300_METRICS_ALLOWED_IPS=127.0.0.1,172.16.0.0/12
./target/release/sz300-server > /tmp/sz300-server.log 2>&1
`;
    const stream = sftpConn.createWriteStream('/tmp/start_sz300_metrics.sh');
    await new Promise((resolve, reject) => {
      stream.on('error', reject);
      stream.on('close', resolve);
      stream.end(startScript);
    });
    await sshExec('chmod +x /tmp/start_sz300_metrics.sh');
    log('- 上传完成 ✅');

    // 3. 重启 sz300-server
    log('## 3. 重启 sz300-server');
    await sshExec('pkill -9 -f sz300-server 2>/dev/null; sleep 2');
    await sshExec('screen -dmS sz300 bash /tmp/start_sz300_metrics.sh');
    log('- 启动中...');
    await new Promise(r => setTimeout(r, 8000));
    const ps = await sshExec('pgrep -a sz300-server');
    log(`- 进程: ${ps.stdout.trim()}`);
    const health = await sshExec('curl -s --connect-timeout 5 http://127.0.0.1:8300/health');
    log(`- health: ${health.stdout.trim()}`);

    // 4. 验证 metrics（带 bearer token）
    log('## 4. 验证 metrics（bearer token）');
    const m1 = await sshExec('curl -s --connect-timeout 5 -o /dev/null -w "%{http_code}" http://127.0.0.1:8300/metrics');
    log(`- 无 token: HTTP ${m1.stdout.trim()}（预期 403）`);
    const m2 = await sshExec('curl -s --connect-timeout 5 -o /dev/null -w "%{http_code}" -H "Authorization: Bearer prometheus-scrape-token" http://127.0.0.1:8300/metrics');
    log(`- 有 token: HTTP ${m2.stdout.trim()}（预期 200）`);

    // 5. 重启 Prometheus
    log('## 5. 重启 Prometheus');
    const restartProm = await sshExec('cd /www/rust/sz-rust-new/deploy && docker compose restart prometheus 2>&1');
    log(restartProm.stdout.trim() || restartProm.stderr.trim());
    await new Promise(r => setTimeout(r, 10000));

    // 6. 验证 Prometheus healthy
    log('## 6. Prometheus healthy');
    const promHealth = await sshExec('curl -s --connect-timeout 5 http://127.0.0.1:9090/-/healthy');
    log(`- ${promHealth.stdout.trim()}`);

    // 7. 等待 scrape
    log('## 7. 等待 20s for scrape...');
    await new Promise(r => setTimeout(r, 20000));

    // 8. 验证 targets
    log('## 8. Prometheus targets');
    const targets = await sshExec('curl -s --connect-timeout 5 http://127.0.0.1:9090/api/v1/targets');
    try {
      const td = JSON.parse(targets.stdout);
      for (const t of td.data.activeTargets) {
        log(`- ${t.scrapeUrl} | health=${t.health} | lastError=${(t.lastError || '').substring(0, 80)}`);
      }
    } catch (e) {
      log(`- parse error: ${targets.stdout.substring(0, 200)}`);
    }

    // 9. 容器状态
    log('## 9. 容器状态');
    const cps = await sshExec('docker ps --format "{{.Names}}\\t{{.Status}}\\t{{.Ports}}" | grep -E "prom|grafana|alert"');
    log(cps.stdout.trim());

    // 10. 端口绑定
    log('## 10. 端口绑定');
    const ports = await sshExec('ss -tlnp | grep -E ":(9090|3000|9093)\\b"');
    log(ports.stdout.trim());

    // 11. Grafana
    log('## 11. Grafana');
    const gf = await sshExec('curl -s --connect-timeout 5 -u admin:admin http://127.0.0.1:3000/api/health');
    log(`- ${gf.stdout.trim()}`);

    // 12. AlertManager
    log('## 12. AlertManager');
    const am = await sshExec('curl -s --connect-timeout 5 http://127.0.0.1:9093/-/healthy');
    log(`- ${am.stdout.trim()}`);

    // 13. 公网检查
    log('## 13. 公网检查');
    const pub = await sshExec('ss -tlnp | grep "0.0.0.0" | grep -E ":(9090|3000|9093)\\b"');
    log(`- ${pub.stdout.trim() || '无 0.0.0.0 绑定 ✅'}`);

    // 清理
    await sshExec('rm -f /tmp/start_sz300_metrics.sh');

    // 写报告
    const reportDir = path.join(__dirname, '..', '.codeartsdoer', 'specs', 'prod_hardening_v1', 'reports');
    fs.writeFileSync(path.join(reportDir, 'monitoring_deploy_verification.md'), reportLines.join('\n'));
    log('\n报告已写入 monitoring_deploy_verification.md');

    conn.end();
  } catch (err) {
    log(`[ERROR] ${err.message}`);
    conn.end();
    process.exit(1);
  }
});

conn.on('error', (err) => { console.error('SSH error:', err); process.exit(1); });
conn.connect({ host: '122.51.216.76', port: 22, username: 'root', privateKey, readyTimeout: 20000 });
