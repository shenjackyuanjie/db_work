//! Bearer 鉴权、角色守卫与密码校验。
//!
//! 契约要点（来自 `tests/fixtures/contract/REPORT.md` 的文案字典）：
//! - 无 `Authorization` 头 → 401 `身份认证信息未提供。`
//! - 头格式不对 → 401 `Authorization 请求头格式无效`
//! - token 查不到 → 401 `登录凭证无效`
//! - token 过期 → 401 `登录已过期，请重新登录`（并删除该 token）
//! - 角色不符 → 403 `该接口仅限果农使用` / `该接口仅限购买者使用`
//!
//! 密码三格式：`$argon2*`（新建用）、`pbkdf2_sha256$`（Django 遗留）、旧 `blake3`。
//!
//! TODO(W0-b): 待实现。
