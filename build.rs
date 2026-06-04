// rtc-rs is a pure-Rust orchestration crate with no native linkage of its
// own; all platform bindings live in the (optional) sibling media crates.
// This build script is intentionally a no-op; it exists only so the crate
// matches the workspace layout convention.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
