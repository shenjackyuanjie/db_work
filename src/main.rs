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
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server { addr } => {
            let addr: std::net::SocketAddr = addr.parse().context("解析 addr 失败")?;
            server::run_server(addr).await;
        }
        Commands::Chat { message, image } => {
            let api_key = std::env::var("GLM_API_KEY").expect("请设置环境变量 GLM_API_KEY");

            let client = client::GlmClient::new(api_key);

            // CLI Chat 走柑橘分析接口（exec_analyze_citrus），并对结果做解析与格式化输出
            // 同时保留调试信息：metrics + usage（由 exec_analyze_citrus 返回）
            match client.exec_analyze_citrus(message, image).await {
                Ok(json_val) => {
                    let success = json_val
                        .get("success")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    if !success {
                        println!("分析失败（success=false）");

                        if let Some(m) = json_val.get("metrics") {
                            if let Some(d) = m.get("duration_secs").and_then(|v| v.as_f64()) {
                                println!("\n性能指标:");
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
                        return Ok(());
                    }

                    let data = json_val
                        .get("data")
                        .context("返回缺少 data 字段")?
                        .clone();

                    let analysis: crate::models::CitrusAnalysisResult = serde_json::from_value(data)?;

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

                    if let Some(m) = json_val.get("metrics") {
                        if let Some(d) = m.get("duration_secs").and_then(|v| v.as_f64()) {
                            println!("\n性能指标:");
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
