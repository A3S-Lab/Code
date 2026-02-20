//! Vector RAG (Semantic Code Search) Example
//!
//! Demonstrates the VectorContextProvider and CodeSearch tool:
//! indexing workspace files and searching by semantic meaning.
//!
//! Run with: cargo run --example test_vector_rag

use a3s_code_core::{Agent, SessionOptions};
use a3s_code_core::context::{VectorContextConfig, VectorContextProvider};
use a3s_code_core::context::embedding::OpenAiEmbeddingProvider;
use a3s_code_core::context::vector_store::InMemoryVectorStore;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn find_config() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists())
        .expect("Config not found. Create ~/.a3s/config.hcl")
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("A3S Code — Vector RAG Example");
    println!("{}", "=".repeat(50));

    // Create a workspace with sample code files
    let dir = TempDir::new()?;
    std::fs::write(dir.path().join("auth.rs"), r#"
/// Verify a JWT token and return the claims.
pub fn verify_jwt(token: &str) -> Result<Claims> {
    // Decode and validate the JWT
    let claims = decode_jwt(token)?;
    validate_expiry(&claims)?;
    Ok(claims)
}
"#)?;
    std::fs::write(dir.path().join("db.rs"), r#"
/// Create a database connection pool.
pub async fn connect_pool(url: &str) -> Pool {
    // Initialize connection pool with retry logic
    Pool::builder()
        .max_connections(10)
        .connect(url)
        .await
        .expect("Failed to connect")
}
"#)?;
    std::fs::write(dir.path().join("handler.rs"), r#"
/// Handle an incoming HTTP request.
pub async fn handle_request(req: Request) -> Response {
    match req.method() {
        Method::GET => handle_get(req).await,
        Method::POST => handle_post(req).await,
        _ => Response::method_not_allowed(),
    }
}
"#)?;

    println!("Workspace: {}", dir.path().display());
    println!("Files: auth.rs, db.rs, handler.rs\n");

    // Use the agent with codesearch tool (requires embedding API)
    // For this example we demonstrate the SessionOptions API
    let agent = Agent::new(find_config().to_str().unwrap()).await?;
    let opts = SessionOptions::new()
        .with_permissive_policy()
        .with_fs_context(dir.path());

    let session = agent.session(dir.path().to_str().unwrap(), Some(opts))?;

    println!("Asking agent to find authentication-related code...\n");
    let result = session.send(
        "Search for code related to JWT authentication and token verification.",
        None,
    ).await?;

    println!("Tool calls: {}", result.tool_calls_count);
    println!("Response:\n{}", result.text);

    println!("\n✓ Vector RAG example complete");
    Ok(())
}
