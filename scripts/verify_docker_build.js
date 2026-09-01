const { Client } = require('ssh2');
const fs = require('fs');
const path = require('path');

const GITHUB_TOKEN = process.env.GH_TOKEN || process.env.GITHUB_TOKEN || '';
const REPO = 'ljclz/sz-rust';
const RUN_ID = process.argv[2] || '33484512301';
const TAG = process.argv[3] || 'v1.2.0-prod-hardening.2';
const IMAGE = `ghcr.io/ljclz/sz300-server:${TAG}`;

const privateKey = `-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACDOrCHylYDXYOP0YBS+Ir7M5xnc1aPgf0dn7d3X9Gda3AAAAJjPWS0pz1kt
KQAAAAtzc2gtZWQyNTUxOQAAACDOrCHylYDXYOP0YBS+Ir7M5xnc1aPgf0dn7d3X9Gda3A
AAAEA4xgtubqt9HARi2/WKfFEtU8T3X5jPkusyG5SxCwgucc6sIfKVgNdg4/RgFL4ivszn
GdzVo+B/R2ft3df0Z1rcAAAAE3Jvb3RAVk0tMTYtMy11YnVudHUBAg==
-----END OPENSSH PRIVATE KEY-----`;

const reportLines = [];
const log = (s) => { reportLines.push(s); console.log(s); };

async function checkGitHubAPI() {
  const https = require('https');
  const options = {
    hostname: 'api.github.com',
    path: `/repos/${REPO}/actions/runs/${RUN_ID}/jobs`,
    headers: {
      'Authorization': `token ${GITHUB_TOKEN}`,
      'Accept': 'application/vnd.github+json',
      'User-Agent': 'node'
    }
  };
  return new Promise((resolve, reject) => {
    https.get(options, (res) => {
      let data = '';
      res.on('data', (d) => data += d);
      res.on('end', () => resolve(JSON.parse(data)));
    }).on('error', reject);
  });
}

(async () => {
  try {
    log('# Docker 镜像构建验证报告');
    log('');
    log(`> 生成时间：${new Date().toISOString()}`);
    log(`> Run ID: ${RUN_ID}`);
    log(`> Tag: ${TAG}`);
    log(`> Image: ${IMAGE}`);
    log('');

    log('## 1. 检查 release.yml docker job 状态');
    const jobsData = await checkGitHubAPI();
    const dockerJob = jobsData.jobs.find(j => j.name.includes('Docker'));
    const testJob = jobsData.jobs.find(j => j.name.includes('Test'));

    log(`- Test job: ${testJob?.status} / ${testJob?.conclusion || 'N/A'}`);
    log(`- Docker job: ${dockerJob?.status || 'not found'} / ${dockerJob?.conclusion || 'N/A'}`);

    if (!dockerJob || dockerJob.status !== 'completed') {
      log('- Docker job 尚未完成，等待中...');
      log('');
      log('## 结论');
      log('- ⏳ release.yml docker job 未完成，需等待后重新运行此脚本');
      const reportDir = path.join(__dirname, '..', '.codeartsdoer', 'specs', 'prod_hardening_v1', 'reports');
      fs.writeFileSync(path.join(reportDir, 'docker_build_verification.md'), reportLines.join('\n'));
      process.exit(0);
    }

    if (dockerJob.conclusion !== 'success') {
      log(`- ❌ Docker job 失败: ${dockerJob.conclusion}`);
      log('');
      log('## 结论');
      log('- ❌ release.yml docker job 失败，Docker 镜像未构建');
      const reportDir = path.join(__dirname, '..', '.codeartsdoer', 'specs', 'prod_hardening_v1', 'reports');
      fs.writeFileSync(path.join(reportDir, 'docker_build_verification.md'), reportLines.join('\n'));
      process.exit(1);
    }

    log('- ✅ Docker job 成功');
    log('');

    log('## 2. SSH 到服务器拉取 GHCR 镜像并验证');
    const conn = new Client();
    const sshExec = (cmd, timeout = 60000) => new Promise((resolve, reject) => {
      conn.exec(cmd, (err, stream) => {
        if (err) { reject(err); return; }
        let stdout = '', stderr = '';
        stream.on('data', (d) => stdout += d.toString());
        stream.stderr.on('data', (d) => stderr += d.toString());
        stream.on('close', (code) => resolve({ stdout, stderr, code }));
      });
    });

    await new Promise((resolve, reject) => {
      conn.on('ready', resolve);
      conn.on('error', reject);
      conn.connect({ host: '122.51.216.76', port: 22, username: 'root', privateKey, readyTimeout: 20000 });
    });

    log('');
    log('### 2.1 拉取镜像');
    const pull = await sshExec(`docker pull ${IMAGE} 2>&1`, 120000);
    log('```');
    log(pull.stdout.trim().substring(0, 500));
    log('```');

    log('');
    log('### 2.2 检查镜像是否存在');
    const inspect = await sshExec(`docker image inspect ${IMAGE} --format '{{.Config.ExposedPorts}} | {{.RepoTags}}' 2>&1`);
    log(`- ExposedPorts + RepoTags: ${inspect.stdout.trim()}`);

    log('');
    log('### 2.3 检查 EXPOSE 端口（预期 8300）');
    const expose = await sshExec(`docker image inspect ${IMAGE} --format '{{json .Config.ExposedPorts}}' 2>&1`);
    log(`- ExposedPorts: ${expose.stdout.trim()}`);
    const has8300 = expose.stdout.includes('8300');
    log(`- 包含 8300: ${has8300 ? '✅' : '❌'}`);

    log('');
    log('### 2.4 检查 Healthcheck');
    const healthcheck = await sshExec(`docker image inspect ${IMAGE} --format '{{json .Config.Healthcheck}}' 2>&1`);
    log(`- Healthcheck: ${healthcheck.stdout.trim()}`);

    log('');
    log('### 2.5 镜像大小');
    const size = await sshExec(`docker image inspect ${IMAGE} --format '{{.Size}}' 2>&1`);
    const sizeMB = (parseInt(size.stdout.trim()) / 1024 / 1024).toFixed(2);
    log(`- 镜像大小: ${sizeMB} MB`);

    log('');
    log('### 2.6 清理镜像');
    const rmi = await sshExec(`docker rmi ${IMAGE} 2>&1`);
    log(`- 清理: ${rmi.stdout.trim()}`);

    conn.end();

    log('');
    log('## 3. 结论');
    log(`- Docker job: ✅ 成功`);
    log(`- 镜像拉取: ${pull.code === 0 ? '✅' : '❌'}`);
    log(`- EXPOSE 8300: ${has8300 ? '✅' : '❌'}`);
    log(`- Healthcheck: ${healthcheck.stdout.trim() ? '✅' : '❌'}`);

    const reportDir = path.join(__dirname, '..', '.codeartsdoer', 'specs', 'prod_hardening_v1', 'reports');
    fs.writeFileSync(path.join(reportDir, 'docker_build_verification.md'), reportLines.join('\n'));
    log('');
    log('报告已写入 docker_build_verification.md');

  } catch (err) {
    log(`[ERROR] ${err.message}`);
    const reportDir = path.join(__dirname, '..', '.codeartsdoer', 'specs', 'prod_hardening_v1', 'reports');
    fs.writeFileSync(path.join(reportDir, 'docker_build_verification.md'), reportLines.join('\n'));
    process.exit(1);
  }
})();