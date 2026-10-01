fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // pyo3's `extension-module` feature deliberately does not link libpython:
    // the CPython symbols are resolved at import time by the interpreter that
    // loads this .so. Apple's linker rejects undefined symbols by default, so
    // a plain `cargo build` of this cdylib fails with "symbol(s) not found"
    // for every `Py*` symbol. maturin passes these flags itself; emitting them
    // here keeps `cargo build`/`cargo test` over the workspace working too.
    //
    // `rustc-cdylib-link-arg` only affects this package's cdylib, so it does
    // not weaken undefined-symbol checking for anything else in the workspace.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}
