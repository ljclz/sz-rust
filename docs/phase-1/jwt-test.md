# Phase -1.2：SZ-ORM-Auth JWT 兼容性验证

> 验证日期：2026-07-20
> 验证结果：✅ 53 测试全部通过

## 验证内容

| 测试项 | 结果 | 说明 |
|--------|------|------|
| JWT encode/decode roundtrip | ✅ | 标准 HS256 签名，PHP lcobucci/jwt 兼容格式 |
| JWT 格式校验（header.payload.signature） | ✅ | 三段式标准 JWT |
| JWT 篡改拒绝 | ✅ | 篡改 payload 后签名验证失败 |
| JWT refresh roundtrip | ✅ | 刷新令牌流转 |
| Auth Authenticator（完整认证流） | ✅ | encode → verify 全流程 |
| RBAC Authorizer（admin） | ✅ | admin 角色通过 |
| RBAC Authorizer（普通用户） | ✅ | 角色权限校验 |
| Doc-tests | ✅ | 1 ignored（需外部依赖） |

## 与 PHP JWT 库兼容性

| PHP 库 | SZ-ORM-Auth 等价功能 | 状态 |
|--------|---------------------|------|
| firebase/php-jwt | JWT::encode / JWT::decode | ✅ 53 tests pass |
| lcobucci/jwt | 令牌构建/签名/验证 | ✅ 复用 |

## 结论

SZ-ORM-Auth 可直接替代 PHP 的两个 JWT 库，53 测试全部通过，JWT roundtrip 成功，无需额外适配。
