# 柑橘果园智能诊断与管理系统

一个同端口部署的 Rust 单体服务：Axum 托管原生多页 Web 前端，同时提供果园诊断、园区监测、账号与权限、现货商城和管理员运营接口。

```text
Browser (HTML/CSS/JS)
        │ Cookie / HTTP API
        ▼
Axum application
 ├─ accounts, admin, tasks, orchard, store
 ├─ PostgreSQL (SQLx)
 ├─ cached local ONNX gate/classifier
 └─ OpenRouter diagnosis and fertilization client
```

## 运行

要求：Rust 1.85+、PostgreSQL、两个 ONNX 模型；使用高级诊断和施肥建议时还需要 OpenRouter API Key。

创建本机 `config.toml`（该文件被 Git 忽略）：

```toml
[server]
addr = "127.0.0.1:11000"
log_level = "info"
database_max_connections = 10
# HTTPS 或可信反向代理环境设为 true；本地 HTTP 开发保持 false。
secure_session_cookie = false

[ai]
openrouter_api_key = "sk-or-v1-your-key"

[database]
postgres_url = "postgres://user:password@127.0.0.1:5432/citrus"

[bootstrap]
# 仅开发演示环境启用，生产环境不会自动写入树位、传感器或商品数据。
seed_demo_data = false

[inference]
mode = "remote" # remote 或 onnx
model_1_path = "onnx/model_1.onnx"
model_2_path = "onnx/model_2.onnx"
```

```powershell
cargo run --release
```

服务会安全地补齐表、会话过期列和索引；不会在默认配置下写入演示数据。

## Web 页面

| 路径 | 页面 | 权限 |
|---|---|---|
| `/` | 登录、注册 | 公开 |
| `/analyze` | 病害识别 | 已登录 |
| `/orchard-3d` | 三维果园沙盘 | 页面公开，数据需登录 |
| `/store` | 脐橙现货商城 | 商品公开；下单和订单需登录 |
| `/cart` | 购物车与结算 | 商品公开；下单需登录 |
| `/admin` | 管理后台 | 管理员 |
| `/commerce` | 历史开团入口 | 兼容跳转至 `/store` |

## 会话与权限

- 浏览器登录使用 `HttpOnly; SameSite=Lax` Cookie，前端不会读取或复制令牌。
- 旧 API 客户端仍可从登录响应读取 `token`，并通过 `X-Session-Token` 发送；此兼容方式建议逐步迁移到 Cookie 或专用服务凭据。
- 会话在服务端强制 30 天过期，旧的 BLAKE3 密码摘要会在账户下次成功登录时自动升级为 Argon2id。
- 用户级接口以当前登录用户为准。为兼容旧请求保留的 `username` 字段和查询参数只能与当前会话一致，不能再用于读取或修改其他用户的数据。
- 识别图片保存在 `storage/recognition_records/`；读取图片需要通过所属用户会话验证。旧 `/uploads/{file}` 路径也会走相同验证。

## API 兼容性

历史路由、HTTP 方法和字段继续保留，包括：

- 账号：`/api/register`、`/api/login`、`/api/logout`、`/api/validate` 与 `/user/*` 等价路由。
- 诊断、任务、温湿度、果园：`/api/citrus-disease*`、`/api/tasks*`、`/api/temperature-humidity`、`/api/recognition-records`、`/user/orchard/overview`。
- 现货商城：`/api/store/products`、`/user/store/orders`、`/user/admin/store/*`。商城管理支持商品编辑与封面上传：`PUT /user/admin/store/products/{id}` 与 `POST /user/admin/store/products/{id}/cover`（multipart `image` 字段），封面图通过公开路由 `/store-images/{file_name}` 访问。
- 历史批次团购兼容 API：`/api/commerce/*`、`/user/commerce/*`、`/user/admin/commerce/*`。这些接口与 `commerce_*` 表保留，新的 Web 界面不再主动调用它们。

跨域策略暂保持原有宽松行为以避免中断既有 API 调用；部署到公网时应在反向代理或后续配置中按调用方白名单收紧来源。

## 项目结构

```text
src/
├── main.rs                 # 启动入口
├── config.rs               # TOML 配置
├── server.rs               # Router、状态与服务生命周期
├── server/                 # 核心、AI、商城和数据库初始化
├── user_routes/            # 注册、会话、管理员与商城 API
├── inference/              # ONNX 缓存推理与远程推理运行时
├── client/                 # OpenRouter 和施肥建议客户端
└── system_settings.rs      # 可配置业务设置
static/                     # 原生多页前端
storage/recognition_records/# 运行时识别图片（Git 忽略）
onnx/                       # 本地模型
scripts/                    # 运维与测试脚本
```

## 验证

```powershell
cargo fmt --check
cargo check
cargo test
```

`WebServer性能测试报告.md` 是本机短时 HTTP 基线，不包含 ONNX 推理和远端 AI 请求；生产容量需要独立压测机、真实数据库配置和长时混合流量验证。
