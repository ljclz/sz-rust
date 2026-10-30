const { Client } = require('ssh2');
const fs = require('fs');

const conn = new Client();
conn.on('ready', () => {
  console.log('SSH connected');
  const cmds = [
    'ls -la /root/.cargo/bin/ 2>/dev/null',
    'which rustup 2>/dev/null; ls -la /root/.cargo/bin/rustup 2>/dev/null',
    '/root/.cargo/bin/rustc --version 2>/dev/null',
    '/root/.cargo/bin/cargo --version 2>/dev/null',
    'rustup show 2>/dev/null',
    'whoami',
    'echo $PATH',
  ];
  let i = 0;
  function runNext() {
    if (i >= cmds.length) { conn.end(); return; }
    const cmd = cmds[i++];
    conn.exec(cmd, (err, stream) => {
      let out = '';
      stream.on('data', d => out += d);
      stream.on('close', () => {
        console.log(`\n=== ${cmd} ===`);
        console.log(out.trim() || '(无输出)');
        runNext();
      });
    });
  }
  runNext();
}).connect({
  host: '122.51.216.76',
  port: 22,
  username: 'root',
  privateKey: fs.readFileSync('E:/vue/test/鲜视达/rust/sz-rust/deploy_key', 'utf-8')
});
