use a3s_code::Agent;
use std::time::Duration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create agent from config
    let agent = Agent::new("~/.a3s/config.acl").await?;
    let session = agent.session(".")?;

    println!("Starting long-running operation...");

    // Spawn a task to cancel after 3 seconds
    let session_clone = session.clone();
    tokio::spawn(async move {
        sleep(Duration::from_secs(3)).await;
        println!("\n🛑 Cancelling operation...");
        let cancelled = session_clone.cancel().await;
        println!("Cancelled: {}", cancelled);
    });

    // Start a long operation
    let result = session
        .send("Write a very long story about a robot learning to code. Make it at least 5000 words.", None)
        .await;

    match result {
        Ok(r) => {
            println!("\n✅ Operation completed (possibly partial)");
            println!("Response length: {} chars", r.text.len());
            println!("First 200 chars: {}", &r.text.chars().take(200).collect::<String>());
        }
        Err(e) => {
            println!("\n❌ Operation failed: {}", e);
        }
    }

    Ok(())
}
