import { EvidenceCollector } from '../lib/evidence-collector.js';

const EVIDENCE_FILE = 'packages/sz-rust-cache-facade/Cargo.toml';
const EVIDENCE_LINE = '11';

export async function validateRedis(ssh, config, projectRoot) {
    const startedAt = Date.now();
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const evidences = [];
    const errors = [];
    let passed = true;

    const redis = config.redis;
    const redisCmd = redis.password
        ? `redis-cli -h ${redis.host} -p ${redis.port} -a '${redis.password}'`
        : `redis-cli -h ${redis.host} -p ${redis.port}`;

    try {
        const { stdout: pingOut } = await ssh.execCommand(`${redisCmd} PING`, { timeout: 5000 });
        if (!pingOut.includes('PONG')) {
            errors.push({ error: 'REDIS_CONNECTION_REFUSED', detail: 'PING 未返回 PONG' });
            return { module: 'Redis', passed: false, evidences, errors, duration: Date.now() - startedAt };
        }
        evidences.push(evidenceCollector.createEvidence(
            'Redis 连接 PING/PONG 通过',
            EVIDENCE_FILE,
            EVIDENCE_LINE
        ));

        const { stdout: crudOut } = await ssh.execCommand(
            `${redisCmd} SET sz_val_test "hello" && ${redisCmd} GET sz_val_test && ${redisCmd} DEL sz_val_test && (${redisCmd} GET sz_val_test || true)`,
            { timeout: 5000, expectNonZero: true }
        );
        const crudLines = crudOut.trim().split('\n');
        const hasSetOk = crudLines[0] === 'OK';
        const hasGetHello = crudLines[1] === 'hello';
        const hasDelOne = crudLines[2] === '1';
        const hasGetEmpty = crudLines[3] === '' || crudLines[3] === undefined;
        if (hasSetOk && hasGetHello && hasDelOne && hasGetEmpty) {
            evidences.push(evidenceCollector.createEvidence(
                'Redis SET/GET/DEL 操作通过',
                EVIDENCE_FILE,
                EVIDENCE_LINE
            ));
        } else {
            errors.push({ error: 'REDIS_CRUD_FAILED', detail: `SET=${crudLines[0]}, GET=${crudLines[1]}, DEL=${crudLines[2]}, GET_nil=${crudLines[3]}` });
            passed = false;
        }

        const { stdout: ttlOut } = await ssh.execCommand(
            `${redisCmd} SET sz_ttl_test "temp" EX 1 && sleep 2 && (${redisCmd} GET sz_ttl_test || true)`,
            { timeout: 10000, expectNonZero: true }
        );
        const ttlLines = ttlOut.trim().split('\n');
        const ttlSetOk = ttlLines[0] === 'OK';
        const ttlGetEmpty = ttlLines[1] === '' || ttlLines[1] === undefined;
        if (ttlSetOk && ttlGetEmpty) {
            evidences.push(evidenceCollector.createEvidence(
                'Redis TTL 过期自动删除通过',
                EVIDENCE_FILE,
                EVIDENCE_LINE
            ));
        } else {
            errors.push({ error: 'REDIS_TTL_FAILED', detail: `SET=${ttlLines[0]}, GET_after_ttl=${ttlLines[1]}` });
            passed = false;
        }

        const { stdout: lockOut } = await ssh.execCommand(
            `${redisCmd} SET sz_lock_test "owner_A" NX EX 10 && (${redisCmd} SET sz_lock_test "owner_B" NX EX 10 || true)`,
            { timeout: 5000, expectNonZero: true }
        );
        const lockLines = lockOut.trim().split('\n');
        const lock1Ok = lockLines[0] === 'OK';
        const lock2Empty = lockLines[1] === '' || lockLines[1] === undefined;
        if (lock1Ok && lock2Empty) {
            evidences.push(evidenceCollector.createEvidence(
                'Redis 分布式锁互斥性通过',
                EVIDENCE_FILE,
                EVIDENCE_LINE
            ));
        } else {
            errors.push({ error: 'REDIS_LOCK_MUTEX_FAILED', detail: `lock1=${lockLines[0]}, lock2=${lockLines[1]}` });
            passed = false;
        }

        await ssh.execCommand(`${redisCmd} DEL sz_val_test sz_ttl_test sz_lock_test`, { timeout: 5000, expectNonZero: true });

    } catch (err) {
        errors.push({ error: err.name || 'REDIS_ERROR', detail: err.message });
        passed = false;
    }

    return {
        module: 'Redis',
        passed,
        evidences,
        errors,
        duration: Date.now() - startedAt,
    };
}
