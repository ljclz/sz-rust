const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    const cmd = 'for i in $(seq 1 60); do if ! ps -p 2572357 > /dev/null 2>&1; then echo "PROCESS_DONE after ${i}0 seconds"; break; fi; sleep 10; done; echo "=== LAST 80 LINES ==="; tail -80 /www/rust/cov-final.log';
    console.log('Waiting for process to complete (up to 10 minutes)...');
    conn.exec(cmd, (err, stream) => {
        if (err) { console.error(err); conn.end(); return; }
        let out = '';
        stream.on('data', (d) => { out += d; });
        stream.stderr.on('data', (d) => { out += d; });
        stream.on('close', () => { console.log(out); conn.end(); });
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
    readyTimeout: 30000,
});