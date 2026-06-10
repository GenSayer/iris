// Bake the release version into APP_VERSION at compile time.
//
// In CI, RELEASE_VERSION is set to the date-stamped tag (e.g. "2025-06-09-02-00").
// Locally, it falls back to the Cargo.toml version with a "-dev" suffix so
// debug builds are distinguishable from releases.
//
// The rerun-if-env-changed directives are required: without them, Cargo's
// build-script caching would keep APP_VERSION frozen at whatever value was
// baked on the first compile, even after RELEASE_VERSION changes between
// the `cargo test` and `cargo build` steps in CI.
fn main() {
    println!("cargo:rerun-if-env-changed=RELEASE_VERSION");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    let version = std::env::var("RELEASE_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));

    let profile = std::env::var("PROFILE").unwrap_or_default();
    let full_version = if profile == "debug" && std::env::var("RELEASE_VERSION").is_err() {
        format!("{}-dev", version)
    } else {
        version
    };

    println!("cargo:rustc-env=APP_VERSION={}", full_version);
}
