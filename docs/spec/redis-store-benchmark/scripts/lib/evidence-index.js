const REDIS_STORE_FILE = 'packages/sz-rust-auth-facade/src/redis_store.rs';

export const REDIS_STORE_EVIDENCE = {
  get_version: { file: REDIS_STORE_FILE, line: 142, endLine: 154, cmd: 'GET' },
  increment_version: { file: REDIS_STORE_FILE, line: 156, endLine: 168, cmd: 'INCR' },
  revoke: { file: REDIS_STORE_FILE, line: 199, endLine: 214, cmd: 'SETEX' },
  is_revoked: { file: REDIS_STORE_FILE, line: 216, endLine: 226, cmd: 'EXISTS' },
  register_session: { file: REDIS_STORE_FILE, line: 263, endLine: 292, cmd: 'HSET' },
  get_sessions: { file: REDIS_STORE_FILE, line: 294, endLine: 312, cmd: 'HGETALL' },
  get_session: { file: REDIS_STORE_FILE, line: 314, endLine: 338, cmd: 'HGET' },
  revoke_session: { file: REDIS_STORE_FILE, line: 340, endLine: 372, cmd: 'HGET+HDEL' },
  update_last_active: { file: REDIS_STORE_FILE, line: 374, endLine: 409, cmd: 'HGET+HSET' },
  update_session_jti: { file: REDIS_STORE_FILE, line: 411, endLine: 448, cmd: 'HGET+HSET' },
  cleanup_expired: { file: REDIS_STORE_FILE, line: 450, endLine: 497, cmd: 'HGETALL+HDEL' },
  clear_user_sessions: { file: REDIS_STORE_FILE, line: 499, endLine: 527, cmd: 'HGETALL+DEL' },
  create_redis_stores: { file: REDIS_STORE_FILE, line: 536, endLine: 548, cmd: 'factory' },
  create_redis_stores_with_devices: { file: REDIS_STORE_FILE, line: 554, endLine: 572, cmd: 'factory' },
  RedisConfig: { file: REDIS_STORE_FILE, line: 27, endLine: 40, cmd: 'config' },
  RedisRefreshTokenStore_new: { file: REDIS_STORE_FILE, line: 127, endLine: 137, cmd: 'connect' },
  RedisTokenBlacklist_new: { file: REDIS_STORE_FILE, line: 184, endLine: 194, cmd: 'connect' },
  RedisDeviceSessionStore_new: { file: REDIS_STORE_FILE, line: 243, endLine: 253, cmd: 'connect' },
  RedisDeviceSessionStore_from_conn: { file: REDIS_STORE_FILE, line: 256, endLine: 258, cmd: 'shared-conn' },
};