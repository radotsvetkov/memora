use anyhow::Result;
use memora_mcp::tools::MemoraMcpServer;
use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let service = MemoraMcpServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
