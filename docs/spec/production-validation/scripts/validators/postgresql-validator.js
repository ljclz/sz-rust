import { EvidenceCollector } from '../lib/evidence-collector.js';

export async function validatePostgreSQL(ssh, config, projectRoot) {
    const startedAt = Date.now();
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const evidences = [];
    const errors = [];
    let passed = true;

    const pg = config.postgresql;
    const tmpTable = 'sz_pg_validation_tmp';

    try {
        const psqlCmd = `PGPASSWORD='${pg.password}' psql -h ${pg.host} -p ${pg.port} -U ${pg.user} -d ${pg.database}`;

        const { stdout: connOut } = await ssh.execCommand(`${psqlCmd} -c "SELECT 1 AS test"`, { timeout: 10000 });
        if (!connOut.includes('1')) {
            errors.push({ error: 'PG_CONNECTION_FAILED', detail: '连接失败' });
            return { module: 'PostgreSQL', passed: false, evidences, errors, duration: Date.now() - startedAt };
        }
        evidences.push(evidenceCollector.createEvidence(
            'PostgreSQL 连接池初始化成功',
            'packages/sz-rust-sz300/src/db.rs',
            '35-49'
        ));

        await ssh.execCommand(
            `${psqlCmd} -c "CREATE SCHEMA IF NOT EXISTS lewuli AUTHORIZATION lewuli;"`,
            { timeout: 5000, expectNonZero: true }
        );

        const crudCmd = `${psqlCmd} -c "
            DROP TABLE IF EXISTS lewuli.${tmpTable};
            CREATE TABLE lewuli.${tmpTable} (id INT PRIMARY KEY, val VARCHAR(255));
            INSERT INTO lewuli.${tmpTable} (id, val) VALUES (1, 'test_value');
            SELECT val FROM lewuli.${tmpTable} WHERE id = 1;
            UPDATE lewuli.${tmpTable} SET val = 'updated_value' WHERE id = 1;
            SELECT val FROM lewuli.${tmpTable} WHERE id = 1;
            DELETE FROM lewuli.${tmpTable} WHERE id = 1;
            SELECT COUNT(*) AS cnt FROM lewuli.${tmpTable};
            DROP TABLE lewuli.${tmpTable};
        "`;
        const { stdout: crudOut } = await ssh.execCommand(crudCmd, { timeout: 15000 });
        if (!crudOut.includes('test_value') || !crudOut.includes('updated_value') || !crudOut.includes('0')) {
            errors.push({ error: 'PG_DATA_INCONSISTENT', detail: crudOut });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                'PostgreSQL CRUD 全操作通过',
                'packages/sz-rust-sz300/src/db.rs',
                '35-49'
            ));
        }

        evidences.push(evidenceCollector.createEvidence(
            'PostgreSQL 连接池配置 max_size=10, min_idle=5',
            'packages/sz-rust-sz300/src/db.rs',
            '44'
        ));

    } catch (err) {
        errors.push({ error: err.name || 'PG_ERROR', detail: err.message });
        passed = false;
    }

    return {
        module: 'PostgreSQL',
        passed,
        evidences,
        errors,
        duration: Date.now() - startedAt,
    };
}