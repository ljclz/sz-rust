import { EvidenceCollector } from '../lib/evidence-collector.js';

export async function validateMQTT(ssh, config, projectRoot) {
    const startedAt = Date.now();
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const evidences = [];
    const errors = [];
    let passed = true;

    const mqtt = config.mqtt;

    try {
        const { stdout: checkOut } = await ssh.execCommand(
            `which mosquitto_sub || (apt-get update -qq && apt-get install -y -qq mosquitto-clients 2>/dev/null)`,
            { timeout: 60000 }
        );

        const { stdout: whichOut } = await ssh.execCommand('which mosquitto_sub', { timeout: 5000 });
        if (!whichOut.includes('mosquitto_sub')) {
            errors.push({ error: 'MQTT_CLIENT_NOT_AVAILABLE', detail: 'mosquitto-clients 安装失败' });
            return { module: 'MQTT', passed: false, evidences, errors, duration: Date.now() - startedAt };
        }

        const { stdout: dnsCheck } = await ssh.execCommand(
            `getent hosts ${mqtt.host} 2>/dev/null || echo "DNS_FAILED"`,
            { timeout: 5000, expectNonZero: true }
        );
        if (dnsCheck.includes('DNS_FAILED')) {
            errors.push({
                error: 'MQTT_DNS_UNRESOLVABLE',
                detail: `服务器无法解析 ${mqtt.host}，DNS 配置问题（非框架缺陷）`,
                environmental: true,
            });
            evidences.push(evidenceCollector.createEvidence(
                'MQTT Broker DNS 解析失败（环境问题，非框架缺陷）',
                'packages/sz-rust-sz300/src/services/mqtt_service.rs',
                '225-234'
            ));
            return { module: 'MQTT', passed: false, evidences, errors, duration: Date.now() - startedAt };
        }

        const pubSubCmd = `
            timeout 10 bash -c '
                mosquitto_sub -h ${mqtt.host} -p ${mqtt.port} -t /sz/validation/test -C 1 -W 5 > /tmp/mqtt_msg.txt 2>/dev/null &
                SUB_PID=$!
                sleep 1
                mosquitto_pub -h ${mqtt.host} -p ${mqtt.port} -t /sz/validation/test -m "hello_mqtt" 2>/dev/null
                wait $SUB_PID 2>/dev/null
                cat /tmp/mqtt_msg.txt
                rm -f /tmp/mqtt_msg.txt
            '
        `;
        const { stdout: pubSubOut } = await ssh.execCommand(pubSubCmd, { timeout: 15000 });
        if (!pubSubOut.includes('hello_mqtt')) {
            errors.push({ error: 'MQTT_MESSAGE_LOST', detail: '发布/订阅消息未收到' });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                'MQTT 发布/订阅消息一致性通过',
                'packages/sz-rust-sz300/src/services/mqtt_service.rs',
                '10-30'
            ));
        }

        const qos1Cmd = `
            timeout 10 bash -c '
                mosquitto_sub -h ${mqtt.host} -p ${mqtt.port} -t /sz/validation/qos1 -q 1 -C 1 -W 5 > /tmp/mqtt_qos1.txt 2>/dev/null &
                SUB_PID=$!
                sleep 1
                mosquitto_pub -h ${mqtt.host} -p ${mqtt.port} -t /sz/validation/qos1 -m "qos1_message" -q 1 2>/dev/null
                wait $SUB_PID 2>/dev/null
                cat /tmp/mqtt_qos1.txt
                rm -f /tmp/mqtt_qos1.txt
            '
        `;
        const { stdout: qos1Out } = await ssh.execCommand(qos1Cmd, { timeout: 15000 });
        if (!qos1Out.includes('qos1_message')) {
            errors.push({ error: 'MQTT_QOS1_LOST', detail: 'QoS 1 消息未收到' });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                'MQTT QoS 1 至少送达一次通过',
                'packages/sz-rust-sz300/src/services/mqtt_service.rs',
                '10-30'
            ));
        }

        const wildcardCmd = `
            timeout 10 bash -c '
                mosquitto_sub -h ${mqtt.host} -p ${mqtt.port} -t "/sz/device/+/status" -C 1 -W 5 > /tmp/mqtt_wild.txt 2>/dev/null &
                SUB_PID=$!
                sleep 1
                mosquitto_pub -h ${mqtt.host} -p ${mqtt.port} -t /sz/device/TEST001/status -m "online" 2>/dev/null
                wait $SUB_PID 2>/dev/null
                cat /tmp/mqtt_wild.txt
                rm -f /tmp/mqtt_wild.txt
            '
        `;
        const { stdout: wildOut } = await ssh.execCommand(wildcardCmd, { timeout: 15000 });
        if (!wildOut.includes('online')) {
            errors.push({ error: 'MQTT_WILDCARD_FAILED', detail: '通配符 topic 路由失败' });
            passed = false;
        } else {
            evidences.push(evidenceCollector.createEvidence(
                'MQTT 通配符 topic 路由通过',
                'packages/sz-rust-sz300/src/services/mqtt_listener.rs',
                '27-33'
            ));
        }

        await ssh.execCommand('pkill -f "mosquitto_sub.*sz/validation" 2>/dev/null; pkill -f "mosquitto_sub.*sz/device" 2>/dev/null', { timeout: 5000, expectNonZero: true });

    } catch (err) {
        errors.push({ error: err.name || 'MQTT_ERROR', detail: err.message });
        passed = false;
    }

    return {
        module: 'MQTT',
        passed,
        evidences,
        errors,
        duration: Date.now() - startedAt,
    };
}