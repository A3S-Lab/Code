fn main() {
    if std::env::var_os("CARGO_FEATURE_ZVEC_RUST_FTS").is_none() {
        return;
    }
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let runtime_marker = match target_os.as_str() {
        "macos" => "@loader_path/zvec",
        "linux" => "$ORIGIN/zvec",
        _ => return,
    };
    // Release workflows inject the same relocatable flag globally so that
    // standalone Core binaries can load the sidecar. Avoid emitting it a
    // second time for the SDK module itself; duplicate -rpath flags make the
    // linker noisy without changing runtime behavior.
    if std::env::var("RUSTFLAGS")
        .ok()
        .is_some_and(|flags| flags.contains(runtime_marker))
        || std::env::var("CARGO_ENCODED_RUSTFLAGS")
            .ok()
            .is_some_and(|flags| flags.contains(runtime_marker))
    {
        return;
    }
    match target_os.as_str() {
        "macos" => println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/zvec"),
        "linux" => println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/zvec"),
        _ => {}
    }
}
