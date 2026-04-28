use anyhow::Result;
use clap::Args;
use memora_mcp::tools::MemoraMcpServer;
use rmcp::{transport::stdio, ServiceExt};

#[derive(Debug, Args)]
pub struct ServeArgs {}

pub async fn run(_args: ServeArgs) -> Result<()> {
    let service = MemoraMcpServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
