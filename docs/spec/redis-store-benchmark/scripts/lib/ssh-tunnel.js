import net from 'net';

/**
 * 通过 SSH 连接建立本地端口转发隧道（localPort → remoteHost:remotePort）
 *
 * @param {object} opts
 * @param {import('ssh2').Client} opts.sshClient - 已连接的 ssh2 Client 实例
 * @param {number} opts.localPort - 本地监听端口
 * @param {string} opts.remoteHost - 远程目标主机（从服务器视角）
 * @param {number} opts.remotePort - 远程目标端口
 * @returns {Promise<{server: net.Server, close: () => Promise<void>}>}
 */
export function openTunnel({ sshClient, localPort, remoteHost, remotePort }) {
  return new Promise((resolve, reject) => {
    const sockets = new Set();
    let settled = false;

    const server = net.createServer((socket) => {
      sockets.add(socket);
      socket.on('error', () => { });
      socket.on('close', () => sockets.delete(socket));

      sshClient.forwardOut(socket.remoteAddress, socket.remotePort, remoteHost, remotePort, (err, stream) => {
        if (err) {
          socket.destroy();
          return;
        }
        stream.on('error', () => socket.destroy());
        stream.on('close', () => socket.destroy());
        socket.pipe(stream);
        stream.pipe(socket);
      });
    });

    server.on('error', (err) => {
      if (!settled) {
        settled = true;
        reject(err);
      }
    });

    server.listen(localPort, '127.0.0.1', () => {
      settled = true;
      resolve({
        server,
        async close() {
          for (const s of sockets) {
            try { s.destroy(); } catch { }
          }
          sockets.clear();
          return new Promise((res) => {
            server.close(() => res());
          });
        },
      });
    });
  });
}