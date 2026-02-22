# AI 服务

这是一个基于 Rust 开发的智谱 AI AI-4V 模型 API 客户端，提供命令行工具（CLI）和 HTTP 服务器两种使用方式，支持文本和图片的多模态对话。

## 功能特性

- 🚀 支持 CLI 命令行直接调用 AI-4V 模型
- 🌐 内置 HTTP 服务器，提供 RESTful API 接口
- 🖼️ 支持图片输入（JPEG、PNG、GIF、WebP）
- ⚡ 异步请求处理，基于 Tokio 运行时
- 🔧 灵活的参数配置（temperature、top_p、max_tokens）
- 📊 详细的性能统计和 Token 使用情况展示
- 🛡️ 支持 CORS 跨域请求

## 技术栈

- **Rust** - 主要编程语言
- **Tokio** - 异步运行时
- **Axum** - HTTP 服务器框架
- **Reqwest** - HTTP 客户端
- **Serde** - JSON 序列化/反序列化
- **Clap** - 命令行参数解析

## 环境要求

- Rust 1.75 或更高版本
- 智谱 AI API Key

## 安装

```bash
# 克隆项目
git clone <repository-url>
cd db

# 编译项目
cargo build --release
```

## 环境配置

设置智谱 AI API Key：

```bash
# Linux/macOS
export AI_API_KEY=your_api_key_here

# Windows PowerShell
$env:AI_API_KEY="your_api_key_here"

# Windows CMD
set AI_API_KEY=your_api_key_here
```

API Key 可以从 [智谱 AI 开放平台](https://open.bigmodel.cn/) 获取。

## 使用方法

### 方式一：命令行工具（CLI）

#### 启动聊天

```bash
# 纯文本对话
cargo run -- chat --message "你好，请介绍一下你自己"

# 带图片的对话
cargo run -- chat --message "请描述这张图片的内容" --image ./path/to/image.jpg
```

#### 查看帮助

```bash
cargo run -- --help
```

#### CLI 输出示例

```
Response ID: chat-1234567890
Model: AI-4.6v-flash

Assistant回复:
[user]: 你好！我是智谱AI开发的AI-4.6V...

性能指标:
  请求耗时: 1.23 秒
  Token/s: 45.67

Token使用情况:
  Prompt tokens: 45
  Completion tokens: 12
  Total tokens: 57
```

### 方式二：HTTP 服务器

#### 启动服务器

```bash
# 使用默认地址 127.0.0.1:3000
cargo run -- server

# 自定义监听地址
cargo run -- server --addr 0.0.0.0:8080
```

#### API 接口

##### 1. 健康检查

```bash
GET /health
```

响应：
```json
{
  "status": "ok",
  "service": "ai-service-server"
}
```

##### 2. 页面与鉴权相关接口

服务器模式主要用于登录/注册与页面访问控制。
聊天能力请使用 CLI 模式。

## 支持的图片格式

- JPEG/JPG
- PNG
- GIF
- WebP

## 项目结构

```
db/
├── src/
│   ├── main.rs      # 主程序入口，CLI 参数解析
│   ├── client.rs    # AI 服务 客户端实现
│   ├── server.rs    # HTTP 服务器实现
│   ├── models.rs    # 数据结构定义
│   └── utils.rs     # 工具函数（图片处理等）
├── examples/        # 示例代码（目前为空）
├── Cargo.toml       # 项目配置和依赖
└── README.md        # 本文档
```

## 开发

```bash
# 运行测试
cargo test

# 格式化代码
cargo fmt

# 检查代码
cargo check
```

## 注意事项

- ⚠️ 请妥善保管 API Key，不要提交到版本控制系统
- ⚠️ 注意 API 调用的频率限制
- ⚠️ 图片文件越大，base64 编码后体积越大，请控制图片大小
- ⚠️ HTTP 服务器默认只监听本地地址，生产环境请配置防火墙和反向代理

## 常见问题

### Q: 如何获取智谱 AI API Key？
A: 访问 [智谱 AI 开放平台](https://open.bigmodel.cn/) 注册并获取 API Key。

### Q: 支持哪些模型？
A: 当前使用 `AI-4.6v-flash` 模型，支持文本和图片多模态输入。

### Q: 图片大小有限制吗？
A: 智谱 AI 对上传的图片大小有限制，建议使用小于 10MB 的图片。

### Q: 如何在 Docker 中运行？
A: 可以创建 Dockerfile 使用 Rust 镜像构建，或在编译后使用最小化的运行时镜像。

## 许可证

本项目代码仅供学习参考使用。

## 相关链接

- [智谱 AI 开放平台](https://open.bigmodel.cn/)
- [AI-4 模型文档](https://open.bigmodel.cn/dev/api)
- [Rust 官方文档](https://www.rust-lang.org/)
- [Axum 框架](https://github.com/tokio-rs/axum)

