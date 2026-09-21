/**
 * P4-16 100W Token 签发基准测试
 *
 * 测试目标：
 * 1. 批量签发 1,000,000 个 JWT Token
 * 2. 批量校验 1,000,000 个 JWT Token
 * 3. 测量签发/校验吞吐量、内存占用
 *
 * 用法：node p4-16-token-1m.js [--count 1000000]
 */

const crypto = require('crypto');

const count = process.argv.includes('--count')
    ? parseInt(process.argv[process.argv.indexOf('--count') + 1])
    : 1000000;

const secret = 'test-secret-key-for-benchmark';
const header = Buffer.from(JSON.stringify({ alg: 'HS256', typ: 'JWT' })).toString('base64url');

function signToken(userId, iat, exp) {
    const payload = Buffer.from(JSON.stringify({ sub: userId, iat, exp })).toString('base64url');
    const data = `${header}.${payload}`;
    const signature = crypto.createHmac('sha256', secret).update(data).digest('base64url');
    return `${data}.${signature}`;
}

function verifyToken(token) {
    const parts = token.split('.');
    if (parts.length !== 3) return false;
    const data = `${parts[0]}.${parts[1]}`;
    const expectedSig = crypto.createHmac('sha256', secret).update(data).digest('base64url');
    return expectedSig === parts[2];
}

function measureMemory() {
    const mem = process.memoryUsage();
    return {
        rss: (mem.rss / 1024 / 1024).toFixed(1),
        heapUsed: (mem.heapUsed / 1024 / 1024).toFixed(1),
        heapTotal: (mem.heapTotal / 1024 / 1024).toFixed(1),
    };
}

console.log(`P4-16 100W Token 签发基准测试`);
console.log(`签发数量: ${count.toLocaleString()}`);
console.log(`初始内存: RSS=${measureMemory().rss}MB, Heap=${measureMemory().heapUsed}MB`);
console.log('---');

const memBefore = measureMemory();
const startSign = Date.now();

const tokens = new Array(count);
const iat = Math.floor(Date.now() / 1000);
const exp = iat + 3600;

for (let i = 0; i < count; i++) {
    tokens[i] = signToken(`user_${i}`, iat, exp);
}

const signDuration = Date.now() - startSign;
const signQps = (count / (signDuration / 1000)).toFixed(0);
const memAfterSign = measureMemory();

console.log(`签发完成: ${count.toLocaleString()} tokens in ${signDuration}ms`);
console.log(`签发吞吐量: ${signQps} tokens/s`);
console.log(`签发后内存: RSS=${memAfterSign.rss}MB, Heap=${memAfterSign.heapUsed}MB`);

const startVerify = Date.now();
let verified = 0;
let failed = 0;

for (let i = 0; i < count; i++) {
    if (verifyToken(tokens[i])) {
        verified++;
    } else {
        failed++;
    }
}

const verifyDuration = Date.now() - startVerify;
const verifyQps = (count / (verifyDuration / 1000)).toFixed(0);
const memAfterVerify = measureMemory();

console.log(`校验完成: verified=${verified}, failed=${failed} in ${verifyDuration}ms`);
console.log(`校验吞吐量: ${verifyQps} tokens/s`);
console.log(`校验后内存: RSS=${memAfterVerify.rss}MB, Heap=${memAfterVerify.heapUsed}MB`);

console.log('\n=== P4-16 100W Token 基准报告 ===');
console.log(`Token 数量: ${count.toLocaleString()}`);
console.log(`签发耗时: ${signDuration}ms (${signQps} tokens/s)`);
console.log(`校验耗时: ${verifyDuration}ms (${verifyQps} tokens/s)`);
console.log(`内存增长: RSS ${memBefore.rss}→${memAfterVerify.rss}MB (+${(parseFloat(memAfterVerify.rss) - parseFloat(memBefore.rss)).toFixed(1)}MB)`);
console.log(`内存增长: Heap ${memBefore.heapUsed}→${memAfterVerify.heapUsed}MB (+${(parseFloat(memAfterVerify.heapUsed) - parseFloat(memBefore.heapUsed)).toFixed(1)}MB)`);
console.log(`校验成功率: ${(verified / count * 100).toFixed(2)}%`);
console.log('=====================================');