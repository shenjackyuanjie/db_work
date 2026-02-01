mod models;
mod client;
mod utils;
mod server;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "glm-api")]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 启动HTTP服务器
    Server {
        /// 服务器监听地址
        #[arg(short, long, default_value = "127.0.0.1:3000")]
        addr: String,
    },
    /// 发送聊天消息
    Chat {
        /// 文本消息
        #[arg(short, long)]
        message: String,

        /// 图片文件路径（可选）
        #[arg(short, long)]
        image: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server { addr } => {
            let addr: std::net::SocketAddr = addr.parse()?;
            server::run_server(addr).await;
        }
        Commands::Chat { message, image } => {
            let api_key = std::env::var("GLM_API_KEY")
                .expect("请设置环境变量 GLM_API_KEY");

            let client = client::GlmClient::new(api_key);

            // 使用统一的 ChatApiRequest，走与 server /chat 相同的 JSON 输出结构（exec_chat_api）
            let chat_api_request = crate::models::ChatApiRequest {
                message,
                image,
                temperature: Some(0.7),
                top_p: Some(0.9),
                max_tokens: None,
                response_format: None,
            };

            match client.exec_chat_api(chat_api_request).await {
                Ok(json_val) => {
                    // CLI 输出可读格式：先打印性能与 token，再 pretty JSON（保持与 server 同构）
                    if let Some(m) = json_val.get("metrics") {
                        if let Some(d) = m.get("duration_secs").and_then(|v| v.as_f64()) {
                            println!("性能指标:");
                            println!("  请求耗时: {:.2} 秒", d);
                        }
                        if let Some(tps) = m.get("tokens_per_sec").and_then(|v| v.as_f64()) {
                            println!("  Token/s: {:.2}", tps);
                        }
                    }

                    if let Some(u) = json_val.get("usage") {
                        println!("\nToken使用情况:");
                        if let Some(v) = u.get("prompt_tokens") {
                            println!("  Prompt tokens: {}", v);
                        }
                        if let Some(v) = u.get("completion_tokens") {
                            println!("  Completion tokens: {}", v);
                        }
                        if let Some(v) = u.get("total_tokens") {
                            println!("  Total tokens: {}", v);
                        }
                    }

                    println!("\nJSON:");
                    println!("{}", serde_json::to_string_pretty(&json_val)?);
                }
                Err(e) => {
                    eprintln!("请求失败: {}", e);
                }
            }
        }
    }

    Ok(())
}
