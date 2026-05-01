use anyhow::Result;
use memora_mcp::tools::MemoraMcpServer;
use rmcp::{transport::stdio, ServiceExt};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = MemoraMcpServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
