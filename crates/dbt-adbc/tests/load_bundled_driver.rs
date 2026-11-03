//! Drivers are loaded from `<prefix>/lib/dbt/` by the executable in `<prefix>/bin/`.

use dbt_adbc::{Backend, LoadStrategy, driver};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Stub driver: initializes successfully and implements nothing else.
const STUB_DRIVER: &str = r#"
#[unsafe(no_mangle)]
pub extern "C" fn AdbcDriverInit(_: i32, _: *mut u8, _: *mut u8) -> u8 {
    0 // ADBC_STATUS_OK
}
"#;

const BRIDGE: &str = "adbc_driver_bridge";
const POSTGRES: &str = "adbc_driver_postgresql";

#[test]
#[ignore = "spawned by the bridge tests"]
fn bridge_child() {
    driver::Builder::new(Backend::Snowflake, LoadStrategy::Bridge)
        .try_load()
        .unwrap();
}

#[test]
#[ignore = "spawned by installed_driver_is_preferred_over_cdn_cache"]
fn installed_child() {
    driver::Builder::new(Backend::Postgres, LoadStrategy::CdnCache)
        .try_load()
        .unwrap();
}

#[test]
#[ignore = "spawned by broken_installed_driver_is_an_error"]
fn broken_installed_child() {
    let Err(error) = driver::Builder::new(Backend::Postgres, LoadStrategy::CdnCache).try_load()
    else {
        panic!("loaded a broken driver");
    };
    let trace = error
        .details
        .iter()
        .flatten()
        .find(|(key, _)| key == "adbc.drivermanager.driver_load_trace")
        .map(|(_, trace)| String::from_utf8_lossy(trace).into_owned())
        .unwrap_or_default();
    let filename = libloading::library_filename(POSTGRES);
    assert!(trace.contains(&*filename.to_string_lossy()), "{error}");
}

#[test]
fn bridge_driver_is_loaded_from_lib_dbt_dir_next_to_bin_dir() {
    let prefix = tempfile::tempdir().unwrap();
    let exe = install_stub(prefix.path(), BRIDGE);
    assert_child_passes(&exe, prefix.path(), "bridge_child");
}

#[cfg(unix)]
#[test]
fn bridge_driver_is_found_through_a_symlinked_executable() {
    let prefix = tempfile::tempdir().unwrap();
    let exe = install_stub(prefix.path(), BRIDGE);
    // Outside the prefix, so only canonicalization finds `lib/`.
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join(exe.file_name().unwrap());
    std::os::unix::fs::symlink(&exe, &link).unwrap();
    assert_child_passes(&link, prefix.path(), "bridge_child");
}

#[test]
fn installed_driver_is_preferred_over_cdn_cache() {
    let prefix = tempfile::tempdir().unwrap();
    let exe = install_stub(prefix.path(), POSTGRES);
    assert_child_passes(&exe, prefix.path(), "installed_child");
}

/// Without the rewrite to `Bundled`, the CDN driver would load instead.
#[cfg(unix)]
#[test]
fn broken_installed_driver_is_an_error() {
    let prefix = tempfile::tempdir().unwrap();
    let exe = install_stub(prefix.path(), POSTGRES);
    let library = prefix
        .path()
        .join("lib")
        .join("dbt")
        .join(libloading::library_filename(POSTGRES));
    fs::write(library, b"not a library").unwrap();
    assert_child_passes(&exe, prefix.path(), "broken_installed_child");
}

/// Compiles the stub as `<prefix>/lib/dbt/<library>` and returns this test binary
/// copied into `<prefix>/bin/`.
fn install_stub(prefix: &Path, library: &str) -> PathBuf {
    let (bin_dir, lib_dir) = (prefix.join("bin"), prefix.join("lib").join("dbt"));
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&lib_dir).unwrap();
    let stub_source = prefix.join("stub_driver.rs");
    fs::write(&stub_source, STUB_DRIVER).unwrap();

    // Run from the crate so rustup picks the repository's toolchain.
    let compiled = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["--crate-type=cdylib", "--edition=2024", "-o"])
        .arg(lib_dir.join(libloading::library_filename(library)))
        .arg(stub_source)
        .status()
        .unwrap();
    assert!(compiled.success(), "failed to compile the stub driver");

    let exe = env::current_exe().unwrap();
    let copied_exe = bin_dir.join(exe.file_name().unwrap());
    fs::copy(&exe, &copied_exe).unwrap();
    copied_exe
}

fn assert_child_passes(exe: &Path, prefix: &Path, child: &str) {
    let output = Command::new(exe)
        .args([child, "--exact", "--include-ignored"])
        .env_remove("ADBC_REPOSITORY")
        .env_remove("ADBC_DRIVER_PATH")
        .env_remove("DBT_LOAD_STANDARD_SNOWFLAKE_DRIVER")
        .env("HOME", prefix)
        .env("XDG_CACHE_HOME", prefix.join("cache"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
