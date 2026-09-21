mod client;
mod compat;
mod config;
mod inference;
mod models;
mod server;
mod system_settings;
mod user_routes;
mod utils;
mod web;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = config::AppConfig::load("config.toml")?;
    server::init_tracing(&config.server.log_level);
    server::run_server(config).await?;

    Ok(())
}
