//! ADBC Driver
//!
//!

use crate::Database;
use crate::database::AdbcDatabase;
use crate::driver_manager::ManagedDriver as ManagedAdbcDriver;
use crate::install::{self, DriverTriplet, build_http_agent};
use adbc_core::{
    Driver as _, LOAD_FLAG_ALLOW_RELATIVE_PATHS, LOAD_FLAG_DEFAULT, LOAD_FLAG_SEARCH_ENV,
    LOAD_FLAG_SEARCH_SYSTEM, LOAD_FLAG_SEARCH_USER,
    error::{Error, Result, Status},
    options::{AdbcVersion, OptionDatabase, OptionValue},
};
use parking_lot::RwLockUpgradableReadGuard;
use std::hash;
use std::{
    collections::HashMap, env, ffi::c_int, fmt, mem, path::Path, path::PathBuf, sync::LazyLock,
};

#[cfg(debug_assertions)]
use {crate::env_var::env_var_bool, std::io::ErrorKind, std::process::Command};

mod builder;
pub use builder::*;

/// Strategy for loading an ADBC driver.
#[derive(Clone, Debug)]
pub enum LoadStrategy {
    /// Download from the dbt Labs CDN (with local cache).
    CdnCache,
    /// Load from standard ADBC paths and additional system paths.
    ///
    /// The provided library name (e.g. `adbc_driver_snowflake`) is searched in
    /// standard ADBC paths and additional paths. Depending on the OS, the
    /// the full filename will be on of:
    /// - Linux: `libadbc_driver_snowflake.so`
    /// - Windows: `adbc_driver_snowflake.dll`
    /// - macOS: `libadbc_driver_snowflake.dylib`
    System(Option<String>),
    /// Try loading from system paths first; if not found, fall back to CDN cache.
    SystemThenCdnCache,
    /// Load the driver from the sibling lib/ folder.
    Bundled,
    /// Load the `flock` driver that proxies all ADBC calls to a service multiplexing
    /// different ADBC drivers.
    ///
    /// In this strategy, we load the "adbc_driver_flock" driver and configure it
    /// to make calls to the server that loads the actual drivers.
    Remote,
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum Backend {
    /// Snowflake driver implementation (ADBC).
    Snowflake,
    /// BigQuery driver implementation (ADBC).
    BigQuery,
    /// PostgreSQL driver implementation (ADBC).
    Postgres,
    /// Databricks driver implementation (ADBC).
    Databricks,
    /// Redshift driver implementation (ADBC).
    Redshift,
    /// Salesforce driver implementation (ADBC).
    Salesforce,
    /// Spark driver implementation (ADBC).
    Spark,
    /// Official DuckDB ADBC driver from `duckdb/duckdb` releases.
    /// Lives at `fs/adbc/duckdb/` on the CDN. Supports community extensions.
    /// Used by the DuckDB adapter.
    DuckDB,
    /// Bespoke dbt-built DuckDB driver with internal extensions.
    /// Lives at `fs/adbc/duckdb_extended/` on the CDN.
    DuckDBExtended,
    /// Lake compute driver implementation. Behaves like DuckDB.
    LakeCompute,
    /// Microsoft SQL Server implementation (ADBC).
    SQLServer,
    /// Athena driver implementation (ADBC).
    Athena,
    /// ClickHouse driver implementation (ADBC).
    ClickHouse,
    /// Exasol driver implementation (ADBC).
    Exasol,
    /// Generic ADBC driver implementation.
    ///
    /// This variant is fully dynamic and experimental. Features might not work reliably and fail
    /// at runtime.
    Generic {
        /// The name of the dynamic library without prefix or suffix.
        ///
        /// Example: `adbc_driver_sqlite`.
        library_name: &'static str,
        /// The entry point of the dynamic library.
        ///
        /// Example: `Some(b"SqliteDriverInit")`.
        entrypoint: Option<&'static [u8]>,
    },
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Snowflake => write!(f, "Snowflake"),
            Backend::BigQuery => write!(f, "BigQuery"),
            Backend::Postgres => write!(f, "PostgreSQL"),
            Backend::Databricks => write!(f, "Databricks"),
            Backend::Redshift => write!(f, "Redshift"),
            Backend::DuckDB | Backend::DuckDBExtended => write!(f, "DuckDB"),
            Backend::LakeCompute => write!(f, "LakeCompute"),
            Backend::Salesforce => write!(f, "Salesforce"),
            Backend::Spark => write!(f, "Spark"),
            Backend::SQLServer => write!(f, "SQL Server"),
            Backend::Athena => write!(f, "Athena"),
            Backend::ClickHouse => write!(f, "ClickHouse"),
            Backend::Exasol => write!(f, "Exasol"),
            Backend::Generic { library_name, .. } => write!(f, "Generic({library_name})"),
        }
    }
}

impl Backend {
    pub fn adbc_library_name(&self) -> Option<&'static str> {
        match self {
            Backend::Snowflake => Some("adbc_driver_snowflake"),
            Backend::BigQuery => Some("adbc_driver_bigquery"),
            Backend::Postgres => Some("adbc_driver_postgresql"),
            Backend::Databricks => Some("adbc_driver_databricks"),
            Backend::Salesforce => Some("adbc_driver_salesforce"),
            Backend::Spark => Some("adbc_driver_spark"),
            Backend::Redshift => Some("adbc_driver_redshift"),
            Backend::DuckDB | Backend::DuckDBExtended => Some("duckdb"),
            Backend::LakeCompute => Some("adbc_driver_dbt"),
            Backend::SQLServer => Some("adbc_driver_mssql"),
            Backend::Athena => Some("adbc_driver_athena"),
            Backend::ClickHouse => Some("adbc_clickhouse"),
            Backend::Exasol => Some("adbc_driver_exasol"),
            Backend::Generic { library_name, .. } => Some(library_name),
        }
    }

    pub fn adbc_driver_entrypoint(&self) -> Option<&'static [u8]> {
        match self {
            Backend::Snowflake => Some(b"SnowflakeDriverInit"),
            Backend::DuckDB | Backend::DuckDBExtended => Some(b"duckdb_adbc_init"),
            Backend::LakeCompute => Some(b"AdbcDriverDbtInit"),
            Backend::Generic {
                library_name: _,
                entrypoint,
            } => *entrypoint,
            _ => None,
        }
    }
}

/// ADBC Driver.
///
/// A [`Driver`] is a wrapper around a loaded ADBC driver. With a driver, you can create
/// new [`Database`] instances that, in turn, can create new [`Connection`] instances.
pub trait Driver {
    fn new_database(&mut self) -> Result<Box<dyn Database>>;

    fn new_database_with_opts(
        &mut self,
        opts: Vec<(OptionDatabase, OptionValue)>,
    ) -> Result<Box<dyn Database>>;
}

/// A key used to cache loaded ADBC drivers.
#[derive(PartialEq, Eq)]
struct AdbcDriverKey {
    backend: Backend,
    adbc_version: AdbcVersion,
    load_strategy: mem::Discriminant<LoadStrategy>,
}

impl hash::Hash for AdbcDriverKey {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.backend.hash(state);
        c_int::from(self.adbc_version).hash(state);
        self.load_strategy.hash(state);
    }
}

pub struct DriverFilenameDisplay<'a> {
    pub name: &'a str,
    /// OS, arch, and version (all optional).
    pub triplet: DriverTriplet<'a>,
}

impl<'a> fmt::Display for DriverFilenameDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = self.triplet.dll_prefix();
        let suffix = self.triplet.dll_suffix();
        match self.triplet.version {
            "" => write!(f, "{}adbc_driver_{}{}", prefix, self.name, suffix),
            version => write!(
                f,
                "{}adbc_driver_{}-{}{}",
                prefix, self.name, version, suffix
            ),
        }
    }
}

/// Attempt to run `make` in the location of arrow-adbc.
///
/// Only runs when `DISABLE_CDN_DRIVER_CACHE` is set and `DISABLE_AUTO_DRIVER_REBUILD`
/// is unset.
#[cfg(debug_assertions)]
fn rebuild_drivers(dir: &PathBuf) -> Result<()> {
    let needs_rebuild = match Command::new("make").arg("-C").arg(dir).arg("-q").status() {
        Ok(s) => s.code() == Some(1),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            eprintln!("`make` not found, skipping rebuild");
            false
        }
        Err(e) => {
            return Err(Error::with_message_and_status(
                format!("failed to spawn `make -q`: {e}"),
                Status::Internal,
            ));
        }
    };

    if needs_rebuild {
        let status = Command::new("make")
            .arg("-C")
            .arg(dir)
            .status()
            .expect("failed to spawn `make`");

        if !status.success() {
            return Err(Error::with_message_and_status(
                format!("`make` failed in {}", dir.display()),
                Status::Internal,
            ));
        }
    }
    Ok(())
}

/// Searches for subpath starting at `start` and continuing upward through its parents.
///
/// Always checks start. `max_hops = 0` checks `start` only.
/// does not canonicalize
pub fn find_upward_dir(start: &Path, subpath: &Path, max_hops: usize) -> Option<PathBuf> {
    if subpath.is_absolute() {
        return None;
    }

    for dir in start.ancestors().take(max_hops + 1) {
        let candidate = dir.join(subpath);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Climb up the directory tree and returns the first lib/ directory found.
fn find_adbc_libs_directory() -> Option<PathBuf> {
    // No. of dirs to walk is chosen for `dbt` to operate when the invoked dbt project is:
    // * a subdir of the fusion root but not past the crate level
    // * a sibling directory to the invoked fs repo
    // Also supports invocations by relative paths to <fs repo root>/target/debug/dbt
    const LIB_HEIGHT_MAX: usize = 5;
    #[cfg(debug_assertions)]
    const ARROW_HEIGHT_MAX: usize = 10;

    let starting_dir = env::current_exe().ok()?.parent()?.to_path_buf();

    #[cfg(debug_assertions)]
    {
        let adbc_libs_path = env::var("ADBC_REPOSITORY")
            .ok()
            .map(|repo_str| repo_str.into())
            // fall back to `../arrow-adbc/` if ADBC repository is not provided
            .or_else(|| {
                let arrow_adbc_pkg_rel_path: PathBuf =
                    ["arrow-adbc", "go", "adbc", "pkg"].iter().collect();

                find_upward_dir(&starting_dir, &arrow_adbc_pkg_rel_path, ARROW_HEIGHT_MAX)
            })
            .inspect(|arrow_repo| {
                if !env_var_bool("DISABLE_AUTO_DRIVER_REBUILD")
                    .unwrap()
                    .unwrap_or(false)
                {
                    rebuild_drivers(arrow_repo).unwrap();
                }
            });

        if adbc_libs_path.is_some() {
            return adbc_libs_path;
        }
    }

    let lib_dir_rel_path = &PathBuf::from("lib");

    if let Some(sibling_lib) = find_upward_dir(&starting_dir, lib_dir_rel_path, LIB_HEIGHT_MAX) {
        return Some(sibling_lib);
    }

    None
}

/// Directory used by [`AdbcDriver::load_dynamic_from_name`].
static ADBC_LIBS_DIRECTORY: LazyLock<Option<PathBuf>> = LazyLock::new(find_adbc_libs_directory);
/// All loaded ADBC drivers are cached in `LOADED_ADBC_DRIVERS`, no matter the loading strategy used.
static LOADED_ADBC_DRIVERS: LazyLock<
    parking_lot::RwLock<HashMap<AdbcDriverKey, Result<ManagedAdbcDriver>>>,
> = LazyLock::new(|| parking_lot::RwLock::new(HashMap::new()));

pub(crate) struct AdbcDriver {
    backend: Backend,
    driver: ManagedAdbcDriver,
}

impl AdbcDriver {
    /// Returns an ADBC [`Driver`] for a given [`Backend`] and [`AdbcVersion`].
    pub fn try_load_dynamic(
        backend: Backend,
        adbc_version: AdbcVersion,
        mut load_strategy: LoadStrategy,
    ) -> Result<Self> {
        // Override for Snowflake dbt Projects integration
        //
        // This flag changes driver loading behavior to adhere to:
        // https://arrow.apache.org/adbc/main/format/driver_manifests.html
        let use_local_snowflake = env::var("DBT_LOAD_STANDARD_SNOWFLAKE_DRIVER").is_ok();
        if use_local_snowflake {
            load_strategy = LoadStrategy::System(None);
        }
        Self::try_load_driver(backend, adbc_version, load_strategy)
            .map(|driver| Self { backend, driver })
    }

    fn try_load_driver(
        backend: Backend,
        adbc_version: AdbcVersion,
        load_strategy: LoadStrategy,
    ) -> Result<ManagedAdbcDriver> {
        let key = AdbcDriverKey {
            backend,
            adbc_version,
            load_strategy: mem::discriminant(&load_strategy),
        };
        let cache = LOADED_ADBC_DRIVERS.upgradable_read();
        if let Some(driver) = cache.get(&key) {
            return driver.clone();
        }
        // Upgrade the lock for writes before the driver is loaded. This also prevents
        // multiple threads from calling non-thread-safe OS functions used to load the driver.
        let mut cache = RwLockUpgradableReadGuard::upgrade(cache);
        // check again after exclusive lock
        if let Some(driver) = cache.get(&key) {
            return driver.clone();
        }
        let driver = Self::try_load_driver_internal(backend, adbc_version, load_strategy);
        cache.insert(key, driver.clone());
        driver
    }

    fn try_load_driver_internal(
        backend: Backend,
        adbc_version: AdbcVersion,
        load_strategy: LoadStrategy,
    ) -> Result<ManagedAdbcDriver> {
        use Backend::*;
        use LoadStrategy::*;
        let final_strategy = match (load_strategy, backend) {
            // CDN strategy for drivers published to the dbt Labs CDN.
            (
                load_strategy @ (CdnCache | SystemThenCdnCache),
                Snowflake | BigQuery | Postgres | Databricks | Redshift | Spark | DuckDB
                | DuckDBExtended | LakeCompute | Salesforce | SQLServer | ClickHouse,
            ) => {
                #[cfg(debug_assertions)]
                {
                    // This option is only used during development of ADBC drivers to make sure
                    // the drivers are not downloaded from the CDN and are instead loaded from
                    // either the repo root lib/ directory or an arrow-adbc repo whose root is
                    // a sibling to this fs repo.
                    let disable_cdn_driver_cache =
                        env_var_bool("DISABLE_CDN_DRIVER_CACHE")?.unwrap_or(false);
                    if disable_cdn_driver_cache {
                        eprintln!(
                            "WARNING: {} ADBC driver is being loaded from {} in debug mode.",
                            backend,
                            ADBC_LIBS_DIRECTORY.as_ref().unwrap().display()
                        );
                        Bundled
                    } else {
                        load_strategy
                    }
                }
                #[cfg(not(debug_assertions))]
                {
                    load_strategy
                }
            }
            // CDN strategy for non-CDN drivers: just fall back to the system strategy.
            (CdnCache | SystemThenCdnCache | Remote, Athena | Exasol) => System(None),
            // Generic drivers can only be loaded from a file, so fallback to the System strategy.
            (CdnCache | SystemThenCdnCache | Remote, Generic { library_name, .. }) => {
                System(Some(library_name.to_string()))
            }
            // System strategy: load from a provided library name (e.g. "adbc_driver_snowflake").
            (load_strategy @ System(_), _) => load_strategy,
            // Bundled strategy doesn't change for any backend.
            (Bundled, _) => Bundled,
            // Remote drivers are used via the "adbc_driver_flock" library
            (
                load_strategy @ Remote,
                Snowflake | BigQuery | Postgres | Databricks | Redshift | Spark | DuckDB
                | DuckDBExtended | LakeCompute | Salesforce | SQLServer | ClickHouse,
            ) => load_strategy,
        };

        match final_strategy {
            CdnCache => Self::try_load_driver_through_cdn_cache(backend, adbc_version),
            System(library_name) => {
                let name = match library_name.as_ref() {
                    Some(name) => name,
                    // Safe to unwrap because it's an ADBC backend
                    None => backend.adbc_library_name().unwrap(),
                };
                Self::try_load_driver_from_name(backend, name, LOAD_FLAG_DEFAULT, adbc_version)
            }
            SystemThenCdnCache => {
                // Safe to unwrap because it's an ADBC backend and non-CDN backends were already
                // redirected to System(_) above.
                let name = backend.adbc_library_name().unwrap();
                Self::try_load_driver_from_name(backend, name, LOAD_FLAG_DEFAULT, adbc_version)
                    .or_else(|e1| {
                        Self::try_load_driver_through_cdn_cache(backend, adbc_version)
                            .map_err(|e2| {
                                // combine the errors into one
                                let message = format!("Failed to load `{name}` driver from name, then failed to load it from the CDN.\n
First error:\n\
{e1}\n\
Second error:\n\
{e2}");
                                Error::with_message_and_status(
                                    message,
                                    Status::Internal,
                                )
                            })
                    })
            }
            Remote => Self::prepare_for_remote_driver(backend, adbc_version),
            Bundled => {
                let name = backend.adbc_library_name().unwrap();
                // don't search system paths, only the provided
                // additional paths (e.g. sibling lib/ directory)
                let load_flags = 0;
                Self::try_load_driver_from_name(backend, name, load_flags, adbc_version)
            }
        }
    }

    /// Load the driver using the [LoadStrategy::System] or [LoadStrategy::Bundled] strategies.
    fn try_load_driver_from_name(
        backend: Backend,
        name: &str,
        load_flags: u32,
        adbc_version: AdbcVersion,
    ) -> Result<ManagedAdbcDriver> {
        let entrypoint = backend.adbc_driver_entrypoint();
        let mut additional_search_paths: Vec<PathBuf> = Vec::new();
        // Climb up the directory tree and choose the first lib/ directory we can
        // find. The result of that search is cached in LIBS_DIRECTORY. We use as
        // as an additional search path for the driver library.
        if let Some(libs_dir) = ADBC_LIBS_DIRECTORY.as_ref() {
            additional_search_paths.push(libs_dir.clone());
        }
        // Add Homebrew library path on macOS (Apple Silicon).
        // On Intel Macs, Homebrew installs to /usr/local/lib which is already
        // searched by the dynamic linker by default.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        additional_search_paths.push(PathBuf::from("/opt/homebrew/lib"));

        ManagedAdbcDriver::load_from_name(
            backend,
            name,
            entrypoint,
            adbc_version,
            load_flags,
            Some(additional_search_paths),
        )
    }

    /// Load the driver using the [LoadStrategy::CdnCache] strategy.
    fn try_load_driver_through_cdn_cache(
        backend: Backend,
        adbc_version: AdbcVersion,
    ) -> Result<ManagedAdbcDriver> {
        let http_agent = build_http_agent();
        let entrypoint = backend.adbc_driver_entrypoint();
        let (backend_name, triplet) = install::driver_parameters(backend);
        let full_driver_path =
            install::format_driver_path(backend_name, triplet).map_err(|e| e.to_adbc_error())?;
        ManagedAdbcDriver::load_dynamic_from_filename(
            backend,
            &full_driver_path,
            entrypoint,
            adbc_version,
        )
        .or_else(|_| {
            install::install_driver_internal(&http_agent, backend_name, triplet)
                .map_err(|e| Error::with_message_and_status(e.to_string(), Status::IO))?;

            let driver = ManagedAdbcDriver::load_dynamic_from_filename(
                backend,
                &full_driver_path,
                entrypoint,
                adbc_version,
            )?;
            Ok(driver)
        })
    }

    /// Load the driver virtually using the [LoadStrategy::Remote] strategy.
    fn prepare_for_remote_driver(
        backend: Backend,
        adbc_version: AdbcVersion,
    ) -> Result<ManagedAdbcDriver> {
        let mut additional_search_paths: Vec<PathBuf> = Vec::new();
        if let Some(libs_dir) = ADBC_LIBS_DIRECTORY.as_ref() {
            additional_search_paths.push(libs_dir.clone());
            let load_flags = LOAD_FLAG_SEARCH_ENV
                | LOAD_FLAG_SEARCH_USER
                | LOAD_FLAG_SEARCH_SYSTEM
                | LOAD_FLAG_ALLOW_RELATIVE_PATHS;
            return ManagedAdbcDriver::load_from_name(
                backend,
                "adbc_driver_flock",
                None, // entrypoint
                adbc_version,
                load_flags,
                Some(additional_search_paths),
            );
        }

        Err(Error::with_message_and_status(
            "Remote driver strategy requires the `adbc_driver_flock` driver to be \
located in a `lib/` directory next to the executable, but no such directory could \
be found."
                .to_string(),
            Status::Internal,
        ))
    }
}

impl Driver for AdbcDriver {
    fn new_database(&mut self) -> Result<Box<dyn Database>> {
        let managed_database = self.driver.new_database()?;
        let database = AdbcDatabase::new(self.backend, managed_database);
        Ok(Box::new(database))
    }

    fn new_database_with_opts(
        &mut self,
        opts: Vec<(OptionDatabase, OptionValue)>,
    ) -> Result<Box<dyn Database>> {
        let managed_database = self.driver.new_database_with_opts(opts)?;
        let database = AdbcDatabase::new(self.backend, managed_database);
        Ok(Box::new(database))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_load_with_builder(backend: Backend, adbc_version: AdbcVersion) -> Result<()> {
        Builder::new(backend, LoadStrategy::CdnCache)
            .with_adbc_version(adbc_version)
            .try_load()?;
        Ok(())
    }

    #[test]
    fn backend_library_and_protocol_metadata_are_stable() {
        let generic = Backend::Generic {
            library_name: "adbc_driver_sqlite",
            entrypoint: Some(b"SqliteDriverInit"),
        };

        assert_eq!(
            Backend::Postgres.adbc_library_name(),
            Some("adbc_driver_postgresql")
        );
        assert_eq!(Backend::DuckDBExtended.adbc_library_name(), Some("duckdb"));
        assert_eq!(
            Backend::ClickHouse.adbc_library_name(),
            Some("adbc_clickhouse")
        );
        assert_eq!(generic.adbc_library_name(), Some("adbc_driver_sqlite"));
    }

    #[test]
    fn only_custom_entrypoint_backends_report_entrypoints() {
        let generic = Backend::Generic {
            library_name: "adbc_driver_sqlite",
            entrypoint: Some(b"SqliteDriverInit"),
        };
        let generic_without_entrypoint = Backend::Generic {
            library_name: "adbc_driver_sqlite",
            entrypoint: None,
        };

        assert_eq!(
            Backend::Snowflake.adbc_driver_entrypoint(),
            Some(&b"SnowflakeDriverInit"[..])
        );
        assert_eq!(
            Backend::DuckDBExtended.adbc_driver_entrypoint(),
            Some(&b"duckdb_adbc_init"[..])
        );
        assert_eq!(Backend::Postgres.adbc_driver_entrypoint(), None);
        assert_eq!(
            generic.adbc_driver_entrypoint(),
            Some(&b"SqliteDriverInit"[..])
        );
        assert_eq!(generic_without_entrypoint.adbc_driver_entrypoint(), None);
    }

    #[test]
    fn driver_filename_display_honors_platform_prefix_suffix_and_version() {
        let windows = DriverTriplet {
            os: "pc-windows-msvc",
            arch: "x86_64",
            version: "1.2.3",
        };
        let linux_without_version = DriverTriplet {
            os: "manylinux_2_17-linux-gnu",
            arch: "aarch64",
            version: "",
        };

        assert_eq!(
            DriverFilenameDisplay {
                name: "snowflake",
                triplet: windows,
            }
            .to_string(),
            "adbc_driver_snowflake-1.2.3.dll"
        );
        assert_eq!(
            DriverFilenameDisplay {
                name: "duckdb",
                triplet: linux_without_version,
            }
            .to_string(),
            "libadbc_driver_duckdb.so"
        );
    }

    // XXX: remove the `test_with` attribute when the CI image downloads the Snowflake driver.

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn load_v1_0_0() -> Result<()> {
        try_load_with_builder(Backend::Snowflake, AdbcVersion::V100)?;
        try_load_with_builder(Backend::BigQuery, AdbcVersion::V100)?;
        try_load_with_builder(Backend::Postgres, AdbcVersion::V100)?;
        try_load_with_builder(Backend::Databricks, AdbcVersion::V100)?;
        try_load_with_builder(Backend::DuckDBExtended, AdbcVersion::V100)?;
        try_load_with_builder(Backend::Salesforce, AdbcVersion::V100)?;
        // try_load_with_builder(Backend::Spark, AdbcVersion::V100)?;
        // try_load_with_builder(Backend::SQLServer, AdbcVersion::V100)?;
        // ClickHouse fails when loaded with v1.0.0 requirements, so we skip it here.
        // try_load_with_builder(Backend::ClickHouse, AdbcVersion::V100)?;
        // try_load_with_builder(Backend::Exasol, AdbcVersion::V100)?;
        Ok(())
    }

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn load_v1_1_0() -> Result<()> {
        try_load_with_builder(Backend::Snowflake, AdbcVersion::V110)?;
        try_load_with_builder(Backend::BigQuery, AdbcVersion::V110)?;
        try_load_with_builder(Backend::Postgres, AdbcVersion::V110)?;
        try_load_with_builder(Backend::Databricks, AdbcVersion::V110)?;
        try_load_with_builder(Backend::DuckDBExtended, AdbcVersion::V110)?;
        try_load_with_builder(Backend::Salesforce, AdbcVersion::V110)?;
        // try_load_with_builder(Backend::Spark, AdbcVersion::V110)?;
        // try_load_with_builder(Backend::SQLServer, AdbcVersion::V110)?;
        try_load_with_builder(Backend::ClickHouse, AdbcVersion::V110)?;
        // try_load_with_builder(Backend::Exasol, AdbcVersion::V110)?;
        Ok(())
    }

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn dynamic() -> Result<()> {
        for backend in [
            Backend::Snowflake,
            Backend::BigQuery,
            Backend::Postgres,
            Backend::Databricks,
            Backend::DuckDBExtended,
            Backend::Salesforce,
            // Backend::Spark,
            // Backend::SQLServer,
            Backend::ClickHouse,
            // Backend::Exasol,
        ]
        .iter()
        .copied()
        {
            let _a = AdbcDriver::try_load_dynamic(
                backend,
                AdbcVersion::default(),
                LoadStrategy::CdnCache,
            )?;
            let _b = AdbcDriver::try_load_dynamic(
                backend,
                AdbcVersion::default(),
                LoadStrategy::CdnCache,
            )?;
        }
        Ok(())
    }

    #[test_with::env(FLOCK_DRIVER_TESTS)]
    #[test]
    fn load_flock_driver() -> Result<()> {
        for backend in [
            Backend::Snowflake,
            Backend::BigQuery,
            Backend::Postgres,
            Backend::Databricks,
            Backend::Redshift,
            Backend::Spark,
            Backend::DuckDBExtended,
            Backend::Salesforce,
            Backend::SQLServer,
        ] {
            AdbcDriver::try_load_dynamic(backend, AdbcVersion::default(), LoadStrategy::Remote)?;
        }
        Ok(())
    }
}
