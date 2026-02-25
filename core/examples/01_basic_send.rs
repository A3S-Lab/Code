use a3s_code_core::{Agent, SessionOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = Agent::new("/Users/roylin/Desktop/ai-lab/a3s/.a3s/config.hcl").await?;

    let opts = SessionOptions::new().with_permissive_policy();
    let session = agent.session("/tmp", Some(opts))?;

    let result = session.send("List the files in the current directory.", None).await?;
    println!("{}", result.text);
    println!("Tokens: {}", result.usage.total_tokens);
    Ok(())
}
