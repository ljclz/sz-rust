export async function cleanAll(ssh, config) {
    const startedAt = Date.now();
    const cleaned = [];
    const failed = [];

    try {
        const { stdout: scriptClean } = await ssh.execCommand(
            `rm -f /tmp/verify_mysql_*.sql /tmp/verify_pg_*.sql /tmp/verify_redis_*.sh /tmp/verify_mqtt_*.sh /tmp/mqtt_*.txt 2>/dev/null; ls /tmp/verify_* /tmp/mqtt_*.txt 2>/dev/null || echo "clean"`,
            { timeout: 5000, expectNonZero: true }
        );
        if (scriptClean.includes('clean')) {
            cleaned.push({ artifact: '服务器测试脚本', status: '已删除' });
        } else {
            failed.push({ artifact: '服务器测试脚本', reason: `残留: ${scriptClean}` });
        }
    } catch (err) {
        failed.push({ artifact: '服务器测试脚本', reason: err.message });
    }

    for (const db of config.mysql.databases) {
        try {
            await ssh.execCommand(
                `mysql -h ${config.mysql.host} -P ${config.mysql.port} -u ${db.user} -p'${db.password}' ${db.name} -e "DROP TABLE IF EXISTS sz_validation_tmp"`,
                { timeout: 5000 }
            );
            cleaned.push({ artifact: `MySQL ${db.name}.sz_validation_tmp`, status: '已删除' });
        } catch (err) {
            failed.push({ artifact: `MySQL ${db.name}.sz_validation_tmp`, reason: err.message });
        }
    }

    try {
        const redisCmd = config.redis.password
            ? `redis-cli -h ${config.redis.host} -p ${config.redis.port} -a '${config.redis.password}'`
            : `redis-cli -h ${config.redis.host} -p ${config.redis.port}`;
        await ssh.execCommand(`${redisCmd} DEL sz_val_test sz_ttl_test sz_lock_test`, { timeout: 5000 });
        cleaned.push({ artifact: 'Redis sz_*_test keys', status: '已删除' });
    } catch (err) {
        failed.push({ artifact: 'Redis sz_*_test keys', reason: err.message });
    }

    try {
        const pg = config.postgresql;
        await ssh.execCommand(
            `PGPASSWORD='${pg.password}' psql -h ${pg.host} -p ${pg.port} -U ${pg.user} -d ${pg.database} -c "DROP TABLE IF EXISTS sz_pg_validation_tmp"`,
            { timeout: 5000 }
        );
        cleaned.push({ artifact: 'PostgreSQL sz_pg_validation_tmp', status: '已删除' });
    } catch (err) {
        failed.push({ artifact: 'PostgreSQL sz_pg_validation_tmp', reason: err.message });
    }

    try {
        await ssh.execCommand('pkill -f "mosquitto_sub.*sz/" 2>/dev/null; true', { timeout: 5000, expectNonZero: true });
        cleaned.push({ artifact: 'mosquitto_sub 验证进程', status: '已终止' });
    } catch (err) {
        failed.push({ artifact: 'mosquitto_sub 验证进程', reason: err.message });
    }

    return {
        module: 'Cleaner',
        passed: failed.length === 0,
        cleaned,
        failed,
        duration: Date.now() - startedAt,
    };
}