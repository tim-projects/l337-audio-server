use std::env;

fn main() {
    let version = env::var("L337_VERSION").unwrap_or_else(|_| "0.0.0-dev".to_string());
    println!("cargo:rustc-env=L337_VERSION={}", version);
}
