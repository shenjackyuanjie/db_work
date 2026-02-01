mod client;
mod models;
mod server;
mod utils;

use anyhow::Context;
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
    /// 分析柑橘图片
    Chat {
        /// 图片文件路径
        #[arg(short, long)]
        image: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server { addr } => {
            let addr: std::net::SocketAddr = addr.parse().context("解析 addr 失败")?;
            server::run_server(addr).await;
        }
        Commands::Chat { image } => {
            let api_key = std::env::var("OPENROUTER_API_KEY").expect("请设置环境变量 OPENROUTER_API_KEY");

            let client = client::GlmClient::new(api_key);

            // CLI Chat 走柑橘分析接口（analyze_citrus），并对结果做解析与格式化输出
            // 同时保留调试信息：metrics + usage
            match client.analyze_citrus(image).await {
                Ok(response) => {
                    let analysis = &response.data;

                    println!("柑橘分析结果:");
                    println!("  是否柑橘叶片: {}", analysis.is_citrus_leaf);
                    println!("  柑橘类型: {:?}", analysis.citrus_type);

                    println!("\n病害分析:");
                    println!("  是否健康: {}", analysis.disease_analysis.is_healthy);
                    println!("  病害名称: {}", analysis.disease_analysis.disease_name);
                    println!("  严重程度: {:?}", analysis.disease_analysis.severity);
                    println!("  置信度: {:.2}", analysis.disease_analysis.confidence);
                    println!(
                        "  治疗建议: {}",
                        analysis.disease_analysis.treatment_suggestion
                    );
                    println!(
                        "  预防措施: {}",
                        analysis.disease_analysis.preventive_measures
                    );

                    if !analysis.image_quality_warning.is_empty() {
                        println!("\n图片质量告警: {}", analysis.image_quality_warning);
                    }

                    println!("\n性能指标:");
                    println!("  请求耗时: {:.2} 秒", response.metrics.duration_secs);
                    println!("  Token/s: {:.2}", response.metrics.tokens_per_sec);

                    println!("\nToken使用情况:");
                    println!("  Prompt tokens: {}", response.usage.prompt_tokens);
                    println!("  Completion tokens: {}", response.usage.completion_tokens);
                    println!("  Total tokens: {}", response.usage.total_tokens);

                    println!("\nJSON:");
                    println!("{}", serde_json::to_string_pretty(&response)?);
                }
                Err(e) => {
                    eprintln!("请求失败: {}", e);
                }
            }
        }
    }

    Ok(())
}
