fn main() {
    if std::env::var_os("CARGO_FEATURE_ZVEC_RUST_FTS").is_none() {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/zvec"),
        Ok("linux") => println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/zvec"),
        _ => {}
    }
}
