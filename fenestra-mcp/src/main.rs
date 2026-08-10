//! The fenestra MCP server binary: serves the render/inspect/interact/verify
//! UI tools over stdio, so an MCP client (an AI assistant) can build, render,
//! query, and assert native UIs described as JSON.

use fenestra_mcp::server::FenestraServer;
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Refuses to start on a misconfigured baseline root rather than falling
    // back to a wider one than the operator asked for.
    let service = FenestraServer::from_env()?.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
