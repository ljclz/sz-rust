const { Client } = require('ssh2');
const net = require('net');
const conn = new Client();
const key = `-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACAZ7aSyL1OtDzpB6g9DOlXEkbW/8m7PoABKI6F+HPYClAAAAJDU+jBX1Pow
VwAAAAtzc2gtZWQyNTUxOQAAACAZ7aSyL1OtDzpB6g9DOlXEkbW/8m7PoABKI6F+HPYClA
AAAECTym3MgA4KhTkqlXmGdgDivtIyVsDqegfqguAUJhi9mxntpLIvU60POkHqD0M6VcSR
tb/ybs+gAEojoX4c9gKUAAAADHJvb3RAUzI1My03NQE=
-----END OPENSSH PRIVATE KEY-----`;

const LOCAL_PORT = 5433;
const REMOTE_HOST = '127.0.0.1';
const REMOTE_PORT = 5432;

conn.on('ready', () => {
    console.log(`SSH connected, forwarding localhost:${LOCAL_PORT} -> ${REMOTE_HOST}:${REMOTE_PORT}`);

    net.createServer((socket) => {
        conn.forwardOut(socket.remoteAddress, socket.remotePort, REMOTE_HOST, REMOTE_PORT, (err, stream) => {
            if (err) { socket.destroy(); return; }
            socket.pipe(stream).pipe(socket);
        });
    }).listen(LOCAL_PORT, '127.0.0.1', () => {
        console.log(`Tunnel listening on 127.0.0.1:${LOCAL_PORT}`);
    });

    process.on('SIGINT', () => { conn.end(); process.exit(0); });
}).on('error', err => console.error('SSH:', err.message)).connect({
    host: '121.204.253.75', port: 32, username: 'root', privateKey: key,
});