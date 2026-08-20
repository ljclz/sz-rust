import { execCommand } from './_ssh.js';

export async function sampleResource(sshClient, pid, intervalMs = 2000, durationMs = 30000) {
    const rssSamples = [];
    const cpuSamples = [];
    const iterations = Math.ceil(durationMs / intervalMs);

    for (let i = 0; i < iterations; i++) {
        const { stdout: statusOut } = await execCommand(sshClient, `cat /proc/${pid}/status 2>/dev/null`, 5000);
        const rssMatch = statusOut.match(/VmRSS:\s*(\d+)\s*kB/);
        if (rssMatch) rssSamples.push(parseInt(rssMatch[1], 10) / 1024);

        const { stdout: topOut } = await execCommand(sshClient, `top -b -n 1 -p ${pid} 2>/dev/null | tail -1`, 5000);
        const cols = topOut.trim().split(/\s+/);
        if (cols.length >= 9) cpuSamples.push(parseFloat(cols[8]));

        if (i < iterations - 1) await new Promise(r => setTimeout(r, intervalMs));
    }

    const peakRssMb = rssSamples.length > 0 ? Math.max(...rssSamples) : 0;
    const avgCpuPercent = cpuSamples.length > 0 ? cpuSamples.reduce((a, b) => a + b, 0) / cpuSamples.length : 0;

    return { rssSamples, cpuSamples, peakRssMb, avgCpuPercent, sampleCount: rssSamples.length };
}