//! Build script for a3s-code
//!
//! Compiles the gRPC proto definitions from proto/.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Compile proto file from proto/ directory
    tonic_build::configure()
        .build_server(true) // We're the server
        .build_client(false) // No client needed
        .compile(&["proto/code_agent.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/code_agent.proto");
    println!("cargo:rerun-if-changed=proto");

    Ok(())
}
