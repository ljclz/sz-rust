import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';

const SERVER_HOST = '122.51.216.76';
const SERVER_USER = 'root';
const KEY_PATH = path.resolve(import.meta.dirname, '..', '..', 'deploy_key');
const RESULTS_DIR = path.resolve(import.meta.dirname);
const REPORT_DATE = new Date().toISOString().slice(0, 10);

function createSSH() {
    return new Promise((resolve, reject) => {
        const client = new Client();
        client.on('ready', () => resolve(client));
        client.on('error', reject);
        client.connect({
            host: SERVER_HOST, port: 22, username: SERVER_USER,
            privateKey: fs.readFileSync(KEY_PATH, 'utf-8'),
            readyTimeout: 30000,
        });
    });
}

function exec(client, cmd, timeout = 30000) {
    return new Promise((resolve, reject) => {
        let stdout = '', stderr = '';
        const timer = setTimeout(() => reject(new Error('Timeout: ' + cmd)), timeout);
        client.exec(cmd, (err, stream) => {
            if (err) { clearTimeout(timer); reject(err); return; }
            stream.on('data', d => stdout += d);
            stream.stderr.on('data', d => stderr += d);
            stream.on('close', code => { clearTimeout(timer); resolve({ stdout, stderr, exitCode: code }); });
        });
    });
}

function execBackground(client, cmd) {
    return new Promise((resolve, reject) => {
        client.exec(cmd, (err, stream) => {
            if (err) { reject(err); return; }
            stream.on('close', () => resolve());
            setTimeout(() => resolve(), 1000);
        });
    });
}

async function startMonitoring(client, durationSec) {
    const sarCpuFile = `/tmp/sar-cpu-${Date.now()}.txt`;
    const sarMemFile = `/tmp/sar-mem-${Date.now()}.txt`;
    const dstatFile = `/tmp/dstat-${Date.now()}.txt`;

    console.log('[监控] 启动 sar (CPU) + sar (内存) + dstat (网络)...');
    await execBackground(client, `setsid sar -u 1 ${durationSec} > ${sarCpuFile} 2>&1 < /dev/null &`);
    await execBackground(client, `setsid sar -r 1 ${durationSec} > ${sarMemFile} 2>&1 < /dev/null &`);
    await execBackground(client, `setsid dstat -n --nocolor --noheaders 1 ${durationSec} > ${dstatFile} 2>&1 < /dev/null &`);

    return { sarCpuFile, sarMemFile, dstatFile };
}

async function stopAndCollect(client, files) {
    await new Promise(r => setTimeout(r, 2000));

    console.log('[监控] 收集 sar CPU 数据...');
    const sarCpu = await exec(client, `cat ${files.sarCpuFile} 2>&1`, 10000);

    console.log('[监控] 收集 sar 内存数据...');
    const sarMem = await exec(client, `cat ${files.sarMemFile} 2>&1`, 10000);

    console.log('[监控] 收集 dstat 网络数据...');
    const dstat = await exec(client, `cat ${files.dstatFile} 2>&1`, 10000);

    await exec(client, `rm -f ${files.sarCpuFile} ${files.sarMemFile} ${files.dstatFile}`);

    return {
        sarCpu: parseSarCpu(sarCpu.stdout),
        sarMem: parseSarMem(sarMem.stdout),
        dstat: parseDstat(dstat.stdout),
    };
}

function parseSarCpu(output) {
    const samples = [];
    const lines = output.split('\n');
    for (const line of lines) {
        const match = line.match(/^\d{2}:\d{2}:\d{2}\s+(?:AM|PM)?\s+all\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)/);
        if (match) {
            samples.push({
                timestamp: match[0].split(/\s+/).slice(0, 3).join(' '),
                cpu_user: parseFloat(match[1]),
                cpu_nice: parseFloat(match[2]),
                cpu_sys: parseFloat(match[3]),
                cpu_iowait: parseFloat(match[4]),
                cpu_idle: parseFloat(match[6]),
            });
        }
    }
    return samples;
}

function parseSarMem(output) {
    const samples = [];
    const lines = output.split('\n');
    for (const line of lines) {
        const match = line.match(/^\d{2}:\d{2}:\d{2}\s+(?:AM|PM)?\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)\s+([\d.]+)/);
        if (match) {
            samples.push({
                timestamp: match[0].split(/\s+/).slice(0, 3).join(' '),
                mem_used_pct: parseFloat(match[4]),
            });
        }
    }
    return samples;
}

function parseDstatValue(val) {
    val = val.trim();
    if (val.endsWith('B')) return parseInt(val.slice(0, -1)) || 0;
    if (val.endsWith('k')) return Math.round((parseFloat(val.slice(0, -1)) || 0) * 1024);
    if (val.endsWith('M')) return Math.round((parseFloat(val.slice(0, -1)) || 0) * 1024 * 1024);
    return parseInt(val) || 0;
}

function parseDstat(output) {
    const samples = [];
    const lines = output.split('\n');
    for (const line of lines) {
        if (line.includes('net') || line.includes('recv') || line.trim() === '') continue;
        const parts = line.trim().split(/\s+/);
        if (parts.length === 2 && /[0-9]/.test(parts[0]) && /[0-9]/.test(parts[1])) {
            samples.push({
                net_rx_bytes: parseDstatValue(parts[0]),
                net_tx_bytes: parseDstatValue(parts[1]),
                net_rx_packets: 0,
                net_tx_packets: 0,
            });
        }
    }
    return samples;
}

function calcStats(values) {
    if (values.length === 0) return { avg: 0, max: 0, min: 0 };
    const sorted = [...values].sort((a, b) => a - b);
    const sum = values.reduce((a, b) => a + b, 0);
    return {
        avg: sum / values.length,
        max: sorted[sorted.length - 1],
        min: sorted[0],
    };
}

function generateResourceReport(framework, route, concurrency, data) {
    const cpuUser = data.sarCpu.map(s => s.cpu_user);
    const cpuSys = data.sarCpu.map(s => s.cpu_sys);
    const cpuIdle = data.sarCpu.map(s => s.cpu_idle);
    const cpuIowait = data.sarCpu.map(s => s.cpu_iowait);
    const memUsed = data.sarMem.map(s => s.mem_used_pct);
    const netRx = data.dstat.map(s => s.net_rx_bytes);
    const netTx = data.dstat.map(s => s.net_tx_bytes);

    const lines = [];
    lines.push(`### ${framework} ${route} C=${concurrency}`);
    lines.push('');
    lines.push('| 指标 | avg | max | min |');
    lines.push('|------|-----|-----|-----|');
    lines.push(`| CPU user (%) | ${calcStats(cpuUser).avg.toFixed(2)} | ${calcStats(cpuUser).max.toFixed(2)} | ${calcStats(cpuUser).min.toFixed(2)} |`);
    lines.push(`| CPU sys (%) | ${calcStats(cpuSys).avg.toFixed(2)} | ${calcStats(cpuSys).max.toFixed(2)} | ${calcStats(cpuSys).min.toFixed(2)} |`);
    lines.push(`| CPU idle (%) | ${calcStats(cpuIdle).avg.toFixed(2)} | ${calcStats(cpuIdle).max.toFixed(2)} | ${calcStats(cpuIdle).min.toFixed(2)} |`);
    lines.push(`| CPU iowait (%) | ${calcStats(cpuIowait).avg.toFixed(2)} | ${calcStats(cpuIowait).max.toFixed(2)} | ${calcStats(cpuIowait).min.toFixed(2)} |`);
    lines.push(`| 内存使用率 (%) | ${calcStats(memUsed).avg.toFixed(2)} | ${calcStats(memUsed).max.toFixed(2)} | ${calcStats(memUsed).min.toFixed(2)} |`);
    lines.push(`| 网络 RX (bytes/s) | ${calcStats(netRx).avg.toFixed(0)} | ${calcStats(netRx).max.toFixed(0)} | ${calcStats(netRx).min.toFixed(0)} |`);
    lines.push(`| 网络 TX (bytes/s) | ${calcStats(netTx).avg.toFixed(0)} | ${calcStats(netTx).max.toFixed(0)} | ${calcStats(netTx).min.toFixed(0)} |`);
    lines.push('');
    return lines.join('\n');
}

async function main() {
    console.log('=== 资源利用率监控 ===');

    const duration = parseInt(process.argv[2] || '20');
    const framework = process.argv[3] || 'sz-rust';
    const route = process.argv[4] || '/simple';
    const concurrency = parseInt(process.argv[5] || '32');

    console.log(`监控时长: ${duration}s`);
    console.log(`目标: ${framework} ${route} C=${concurrency}`);
    console.log('');

    const client = await createSSH();

    const files = await startMonitoring(client, duration);
    console.log(`[监控] 采集中... (${duration}s)`);

    await new Promise(r => setTimeout(r, duration * 1000 + 2000));

    const data = await stopAndCollect(client, files);
    console.log(`[监控] CPU 样本: ${data.sarCpu.length}`);
    console.log(`[监控] 内存样本: ${data.sarMem.length}`);
    console.log(`[监控] 网络样本: ${data.dstat.length}`);

    const report = generateResourceReport(framework, route, concurrency, data);
    const reportPath = path.join(RESULTS_DIR, `resource-${framework}-${route.replace('/', '')}-${concurrency}-${REPORT_DATE}.md`);
    fs.writeFileSync(reportPath, `# 资源利用率报告\n\n> 生成时间：${new Date().toISOString()}\n\n${report}`, 'utf-8');

    console.log(`✅ 报告已生成: ${reportPath}`);

    client.end();
}

main().catch(err => { console.error('Error:', err.message); process.exit(1); });

export { startMonitoring, stopAndCollect };