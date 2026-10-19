//! The bridge driver is loaded from `<prefix>/lib/dbt/` by the executable in `<prefix>/bin/`.

use dbt_adbc::{Backend, LoadStrategy, driver};
use std::{env, fs, process::Command};

/// Stub `adbc_driver_bridge`: initializes successfully and implements nothing else.
const STUB_DRIVER: &str = r#"
#[unsafe(no_mangle)]
pub extern "C" fn AdbcDriverInit(_: i32, _: *mut u8, _: *mut u8) -> u8 {
    0 // ADBC_STATUS_OK
}
"#;

#[test]
#[ignore = "spawned by bridge_driver_is_loaded_from_lib_dbt_dir_next_to_bin_dir"]
fn child() {
    driver::Builder::new(Backend::Snowflake, LoadStrategy::Bridge)
        .try_load()
        .unwrap();
}

#[test]
fn bridge_driver_is_loaded_from_lib_dbt_dir_next_to_bin_dir() {
    let prefix = env::temp_dir().join(format!("dbt-adbc-bridge-{}", std::process::id()));
    let (bin_dir, lib_dir) = (prefix.join("bin"), prefix.join("lib").join("dbt"));
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&lib_dir).unwrap();
    let stub_source = prefix.join("stub_driver.rs");
    fs::write(&stub_source, STUB_DRIVER).unwrap();

    // Run from the crate so rustup picks the repository's toolchain.
    let compiled = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["--crate-type=cdylib", "--edition=2024", "-o"])
        .arg(lib_dir.join(libloading::library_filename("adbc_driver_bridge")))
        .arg(stub_source)
        .status()
        .unwrap();
    assert!(compiled.success(), "failed to compile the stub driver");

    // Re-run the child test from a copy of this binary in bin/.
    let exe = env::current_exe().unwrap();
    let copied_exe = bin_dir.join(exe.file_name().unwrap());
    fs::copy(&exe, &copied_exe).unwrap();
    let output = Command::new(copied_exe)
        .args(["child", "--exact", "--include-ignored"])
        .env_remove("ADBC_REPOSITORY")
        .env_remove("ADBC_DRIVER_PATH")
        .env_remove("DBT_LOAD_STANDARD_SNOWFLAKE_DRIVER")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}"
    );
    fs::remove_dir_all(prefix).unwrap();
}
