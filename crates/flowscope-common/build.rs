// Record the compiler version for the version endpoint.
fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().nth(1).map(|v| v.to_string()))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=FLOWSCOPE_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=FLOWSCOPE_VERSION");
}
