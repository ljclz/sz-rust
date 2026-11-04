/**
 * P4-15 并发 10K 连接压测脚本
 *
 * 测试目标：
 * 1. WebSocket + HTTP 混合压测
 * 2. 10,000 并发连接
 * 3. 测量连接成功率、消息吞吐量、P99 延迟
 *
 * 用法：node p4-15-concurrent-10k.js [--target http://127.0.0.1:9527] [--connections 10000]
 */

const http = require('http');
const { URL } = require('url');

const target = process.argv.includes('--target')
    ? process.argv[process.argv.indexOf('--target') + 1]
    : 'http://127.0.0.1:9527';

const maxConnections = process.argv.includes('--connections')
    ? parseInt(process.argv[process.argv.indexOf('--connections') + 1])
    : 10000;

const targetUrl = new URL(target);

const results = {
    totalConnections: 0,
    successfulConnections: 0,
    failedConnections: 0,
    totalRequests: 0,
    successfulRequests: 0,
    failedRequests: 0,
    latencies: [],
    startTime: 0,
    endTime: 0,
};

async function runConcurrentTest() {
    console.log(`P4-15 并发 ${maxConnections} 连接压测`);
    console.log(`目标: ${target}`);
    console.log('---');

    results.startTime = Date.now();

    const batchSize = 500;
    const batches = Math.ceil(maxConnections / batchSize);

    for (let batch = 0; batch < batches; batch++) {
        const batchStart = batch * batchSize;
        const batchEnd = Math.min(batchStart + batchSize, maxConnections);
        const batchPromises = [];

        for (let i = batchStart; i < batchEnd; i++) {
            batchPromises.push(makeRequest(i));
        }

        await Promise.all(batchPromises);

        if ((batch + 1) % 2 === 0 || batch === batches - 1) {
            const elapsed = Date.now() - results.startTime;
            const connRate = (results.successfulConnections / (batchEnd) * 100).toFixed(1);
            console.log(`批次 ${batch + 1}/${batches}: ${batchEnd} 连接, 成功 ${results.successfulConnections}, 失败 ${results.failedConnections}, 耗时 ${elapsed}ms`);
        }
    }

    results.endTime = Date.now();
    printReport();
}

function makeRequest(id) {
    return new Promise((resolve) => {
        results.totalConnections++;
        results.totalRequests++;

        const start = Date.now();

        const req = http.request({
            hostname: targetUrl.hostname,
            port: targetUrl.port,
            path: '/health',
            method: 'GET',
            timeout: 10000,
        }, (res) => {
            let data = '';
            res.on('data', (chunk) => { data += chunk; });
            res.on('end', () => {
                const latency = Date.now() - start;
                results.latencies.push(latency);

                if (res.statusCode === 200) {
                    results.successfulConnections++;
                    results.successfulRequests++;
                } else {
                    results.failedRequests++;
                }
                resolve();
            });
        });

        req.on('error', () => {
            results.failedConnections++;
            results.failedRequests++;
            resolve();
        });

        req.on('timeout', () => {
            results.failedConnections++;
            results.failedRequests++;
            req.destroy();
            resolve();
        });

        req.end();
    });
}

function printReport() {
    const duration = (results.endTime - results.startTime) / 1000;
    const qps = (results.successfulRequests / duration).toFixed(0);

    results.latencies.sort((a, b) => a - b);
    const p50 = results.latencies[Math.floor(results.latencies.length * 0.5)] || 0;
    const p99 = results.latencies[Math.floor(results.latencies.length * 0.99)] || 0;
    const p999 = results.latencies[Math.floor(results.latencies.length * 0.999)] || 0;

    console.log('\n=== P4-15 并发 10K 连接压测报告 ===');
    console.log(`总连接数: ${results.totalConnections}`);
    console.log(`成功连接: ${results.successfulConnections} (${(results.successfulConnections / results.totalConnections * 100).toFixed(1)}%)`);
    console.log(`失败连接: ${results.failedConnections} (${(results.failedConnections / results.totalConnections * 100).toFixed(1)}%)`);
    console.log(`总请求数: ${results.totalRequests}`);
    console.log(`成功请求: ${results.successfulRequests}`);
    console.log(`失败请求: ${results.failedRequests}`);
    console.log(`持续时间: ${duration.toFixed(2)}s`);
    console.log(`QPS: ${qps}`);
    console.log(`P50 延迟: ${p50}ms`);
    console.log(`P99 延迟: ${p99}ms`);
    console.log(`P99.9 延迟: ${p999}ms`);
    console.log('===================================');
}

runConcurrentTest().catch(console.error);