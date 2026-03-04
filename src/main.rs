mod client;
mod config;
mod inference;
mod models;
mod server;
mod user_routes;
mod utils;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::AppConfig::load("config.toml")?;
    server::init_tracing(&config.server.log_level);
    server::run_server(config).await?;

    Ok(())
}
