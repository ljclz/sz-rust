const { Client } = require('ssh2');
const fs = require('fs');
const conn = new Client();
const privateKey = fs.readFileSync('scripts/ssh_key_temp', 'utf8');
conn.on('ready', () => {
    conn.exec('docker run --rm -d --name test-mysql -e MYSQL_ROOT_PASSWORD=test123 -p 13306:3306 mysql:9.6 2>&1 && sleep 10 && docker logs test-mysql 2>&1 | tail -5 && docker stop test-mysql 2>&1', (err, stream) => {
        if (err) { conn.end(); return; }
        let out = '';
        stream.on('data', (d) => out += d);
        stream.stderr.on('data', (d) => out += d);
        stream.on('close', () => { console.log(out); conn.end(); });
    });
}).on('error', (err) => console.error(err)).connect({
    host: '122.51.216.76', port: 22, username: 'root', privateKey,
});