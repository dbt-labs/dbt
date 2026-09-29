//! Build-time wiring for the SPA bundle.
//!
//! `web/dist/` is a committed build of the SPA whose source lives next to it in
//! `web/src/`. Cargo does **not** run the JS build — rebuild with
//! `pnpm build` in `web/` and commit the result alongside any source change.
//! Keeping the bundle in-tree is what lets `cargo build` work with no Node
//! toolchain and no access to the private `@dbt-labs` npm packages, which the
//! release and Copybara pipelines both depend on.

fn main() {
    // Only `web/dist` affects the compiled output. Watching `web/src` would imply
    // cargo rebuilds the bundle, which it does not. `rust-embed` finds the directory
    // itself from `CARGO_MANIFEST_DIR`; this exists purely to trigger the rebuild.
    println!("cargo:rerun-if-changed=web/dist");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var_os("CARGO_FEATURE_EMBED_UI").is_none() {
        println!(
            "cargo:warning=dbt-docs-server: building with no UI backend; `serve_assets` will return 501. \
             Enable `embed-ui` (default) for a working server."
        );
    }
}
