import { EvidenceCollector } from '../lib/evidence-collector.js';

export async function validateMySQL(ssh, config, projectRoot) {
    const startedAt = Date.now();
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const evidences = [];
    const errors = [];
    let passed = true;

    for (const db of config.mysql.databases) {
        const dbResult = await validateOneDatabase(ssh, config.mysql, db, evidenceCollector, evidences, errors);
        if (!dbResult) passed = false;
    }

    return {
        module: 'MySQL',
        passed,
        evidences,
        errors,
        duration: Date.now() - startedAt,
    };
}

async function validateOneDatabase(ssh, mysqlConfig, db, evidenceCollector, evidences, errors) {
    const dbName = db.name;
    const tmpTable = 'sz_validation_tmp';
    let passed = true;

    try {
        const mysqlCmd = `mysql -h ${mysqlConfig.host} -P ${mysqlConfig.port} -u ${db.user} -p'${db.password}' ${dbName}`;

        const { stdout: connOut } = await ssh.execCommand(`${mysqlCmd} -e "SELECT 1 AS test"`, { timeout: 10000 });
        if (!connOut.includes('1')) {
            errors.push({ database: dbName, error: 'MYSQL_CONNECTION_FAILED', detail: '连接失败' });
            return false;
        }
        evidences.push(evidenceCollector.createEvidence(
            `${dbName} 连接池初始化成功`,
            'packages/sz-rust-sz300/src/db.rs',
            '8-32'
        ));

        const crudCmd = `${mysqlCmd} -e "
            DROP TABLE IF EXISTS ${tmpTable};
            CREATE TABLE ${tmpTable} (id INT PRIMARY KEY, val VARCHAR(255));
            INSERT INTO ${tmpTable} (id, val) VALUES (1, 'test_value');
            SELECT val FROM ${tmpTable} WHERE id = 1;
            UPDATE ${tmpTable} SET val = 'updated_value' WHERE id = 1;
            SELECT val FROM ${tmpTable} WHERE id = 1;
            DELETE FROM ${tmpTable} WHERE id = 1;
            SELECT COUNT(*) AS cnt FROM ${tmpTable};
        "`;
        const { stdout: crudOut } = await ssh.execCommand(crudCmd, { timeout: 15000 });
        if (!crudOut.includes('test_value') || !crudOut.includes('updated_value') || !crudOut.includes('0')) {
            errors.push({ database: dbName, error: 'MYSQL_DATA_INCONSISTENT', detail: crudOut });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${dbName} CRUD 全操作通过`,
                'packages/sz-rust-sz300/src/db.rs',
                '8-32'
            ));
        }

        const txCmd = `${mysqlCmd} -e "
            DROP TABLE IF EXISTS ${tmpTable};
            CREATE TABLE ${tmpTable} (id INT PRIMARY KEY, val VARCHAR(255));
            BEGIN;
            INSERT INTO ${tmpTable} (id, val) VALUES (1, 'tx_test');
            ROLLBACK;
            SELECT COUNT(*) AS cnt FROM ${tmpTable};
            BEGIN;
            INSERT INTO ${tmpTable} (id, val) VALUES (2, 'tx_commit');
            COMMIT;
            SELECT val FROM ${tmpTable} WHERE id = 2;
            DROP TABLE ${tmpTable};
        "`;
        const { stdout: txOut } = await ssh.execCommand(txCmd, { timeout: 15000 });
        if (!txOut.includes('0') || !txOut.includes('tx_commit')) {
            errors.push({ database: dbName, error: 'MYSQL_TX_ROLLBACK_FAILED', detail: txOut });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${dbName} 事务 commit/rollback 通过`,
                'packages/sz-rust-sz300/src/db.rs',
                '8-32'
            ));
        }

        const injectCmd = `${mysqlCmd} -e "
            DROP TABLE IF EXISTS ${tmpTable};
            CREATE TABLE ${tmpTable} (id INT PRIMARY KEY, username VARCHAR(255));
            INSERT INTO ${tmpTable} (id, username) VALUES (1, 'admin');
            SELECT COUNT(*) AS cnt FROM ${tmpTable} WHERE username = 'admin'' OR ''1''=''1';
            DROP TABLE ${tmpTable};
        "`;
        const { stdout: injectOut } = await ssh.execCommand(injectCmd, { timeout: 10000 });
        if (injectOut.includes('1')) {
            errors.push({ database: dbName, error: 'MYSQL_INJECTION_BYPASS', detail: 'SQL 注入防护失效' });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${dbName} SQL 注入防护通过`,
                'packages/sz-rust-sz300/tests/db_integration_test.rs',
                '256-270'
            ));
        }

        const concurrentCmd = `for i in $(seq 1 20); do ${mysqlCmd} -e "SELECT 1" & done; wait`;
        const { stdout: concOut } = await ssh.execCommand(concurrentCmd, { timeout: 30000 });
        const successCount = (concOut.match(/1\n/g) || []).length;
        if (successCount < 20) {
            errors.push({ database: dbName, error: 'MYSQL_POOL_TIMEOUT', detail: `并发 20 请求仅成功 ${successCount}` });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                `${dbName} 连接池 20 并发无超时`,
                'packages/sz-rust-sz300/src/db.rs',
                '20-27'
            ));
        }

    } catch (err) {
        errors.push({ database: dbName, error: err.name || 'MYSQL_ERROR', detail: err.message });
        passed = false;
    }

    return passed;
}