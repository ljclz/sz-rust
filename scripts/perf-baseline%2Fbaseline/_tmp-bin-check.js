import { createSSHClient, execCommand, closeClient } from './_ssh.js';

const client = await createSSHClient();
const r = await execCommand(client, 'ls /www/rust/sz-rust-new/target/release/sz* 2>/dev/null; ls /www/rust/sz300/ 2>/dev/null', 10000);
console.log(r.stdout.trim());
await closeClient(client);