mod models;
mod client;
mod utils;
mod server;

use clap::{Parser, Subcommand};
use std::time::Instant;

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

            let content = utils::build_message_content(&message, image.as_ref());

            let request = models::ChatRequest {
                model: "glm-4.6v-flash".to_string(),
                messages: vec![
                    models::ChatMessage {
                        role: "user".to_string(),
                        content,
                    },
                ],
                temperature: Some(0.7),
                top_p: Some(0.9),
                max_tokens: None,
            };

            let start_time = Instant::now();
            match client.chat_completions(&request).await {
                Ok(response) => {
                    let duration = start_time.elapsed();
                    let tps = response.usage.total_tokens as f64 / duration.as_secs_f64();

                    println!("Response ID: {}", response.id);
                    println!("Model: {}", response.model);
                    println!("\nAssistant回复:");

                    for choice in &response.choices {
                        let content_str = match &choice.message.content {
                            serde_json::Value::String(s) => s.clone(),
                            _ => choice.message.content.to_string(),
                        };
                        println!("[{}]: {}", choice.message.role, content_str);
                    }

                    println!("\n性能指标:");
                    println!("  请求耗时: {:.2} 秒", duration.as_secs_f64());
                    println!("  Token/s: {:.2}", tps);
                    println!("\nToken使用情况:");
                    println!("  Prompt tokens: {}", response.usage.prompt_tokens);
                    println!("  Completion tokens: {}", response.usage.completion_tokens);
                    println!("  Total tokens: {}", response.usage.total_tokens);
                }
                Err(e) => {
                    eprintln!("请求失败: {}", e);
                }
            }
        }
    }

    Ok(())
}
