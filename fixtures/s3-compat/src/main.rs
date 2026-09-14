//! Local S3-compatible endpoint for hermetic workspace qualification.
//!
//! Official MinIO container images are no longer published. This process is
//! the S3 API stand-in the release gate needs: path-style, static credentials,
//! and a pre-created bucket. It is not a product dependency.

use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use s3s_fs::FileSystem;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::TcpListener;

const DEFAULT_BIND: &str = "127.0.0.1:9000";
const DEFAULT_BUCKET: &str = "a3s-code-tests";
const DEFAULT_ACCESS_KEY: &str = "a3s-code-test";
const DEFAULT_SECRET_KEY: &str = "a3s-code-test-secret";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind = arg_value("--bind").unwrap_or_else(|| DEFAULT_BIND.to_string());
    let bucket = arg_value("--bucket").unwrap_or_else(|| DEFAULT_BUCKET.to_string());
    let access_key = arg_value("--access-key").unwrap_or_else(|| DEFAULT_ACCESS_KEY.to_string());
    let secret_key = arg_value("--secret-key").unwrap_or_else(|| DEFAULT_SECRET_KEY.to_string());
    let data_dir = arg_value("--data-dir").map(PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("a3s-code-s3-compat-{}", std::process::id()))
    });
    // s3s-fs stores each bucket as a directory under the filesystem root.
    std::fs::create_dir_all(data_dir.join(&bucket))?;

    let mut builder = S3ServiceBuilder::new(
        FileSystem::new(&data_dir).map_err(|error| format!("s3 filesystem: {error:?}"))?,
    );
    builder.set_auth(SimpleAuth::from_single(access_key, secret_key));
    let service = builder.build();

    let addr: SocketAddr = bind.parse()?;
    let listener = TcpListener::bind(addr).await?;
    println!("s3-compat ready {addr}");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let service = service.clone();
                tokio::spawn(async move {
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let _ = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(io, service)
                    .await;
                });
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    Ok(())
}

fn arg_value(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
        if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            return Some(value.to_string());
        }
    }
    None
}
