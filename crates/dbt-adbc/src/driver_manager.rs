// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Load and use ADBC drivers.
//!
//! The driver manager provides an implementation of the ADBC interface which
//! uses FFI to wrap an object file implementation of
//! [`adbc.h`](https://github.com/apache/arrow-adbc/blob/main/c/include/arrow-adbc/adbc.h).
//!
//! There are two ways that drivers can be used:
//! 1. By linking (either statically or dynamically) the driver implementation
//!    at link-time and then using [ManagedDriver::load_static].
//! 2. By loading the driver implementation at run-time (with `dlopen/LoadLibrary`)
//!    using [ManagedDriver::load_dynamic_from_name] or
//!    [ManagedDriver::load_dynamic_from_filename].
//!
//! Drivers are initialized using a function provided by the driver as a main
//! entrypoint, canonically called `AdbcDriverInit`. Although many will use a
//! different name to support statically linking multiple drivers within the
//! same program.
//!
//! ## Using across threads
//!
//! [ManagedDriver], [ManagedDatabase], [ManagedConnection] and [ManagedStatement]
//! can be used across threads though all of their operations are serialized
//! under the hood. They hold their inner implementations within [std::sync::Arc],
//! so they are cheaply clonable.
//!
//! ## Example
//!
//! ```rust
//! # use std::sync::Arc;
//! # use arrow_array::{Array, StringArray, Int64Array, Float64Array};
//! # use arrow_array::{RecordBatch, RecordBatchReader};
//! # use arrow_schema::{Field, Schema, DataType};
//! # use arrow_select::concat::concat_batches;
//! # use adbc_core::{
//! #     options::{AdbcVersion, OptionDatabase, OptionStatement},
//! #     Connection, Database, Driver, Statement, Optionable
//! # };
//! # use adbc_driver_manager::ManagedDriver;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let opts = [(OptionDatabase::Uri, ":memory:".into())];
//! let mut driver = ManagedDriver::load_dynamic_from_name("adbc_driver_sqlite", None, AdbcVersion::V100)?;
//! let database = driver.new_database_with_opts(opts)?;
//! let mut connection = database.new_connection()?;
//! let mut statement = connection.new_statement()?;
//!
//! // Define some data.
//! let columns: Vec<Arc<dyn Array>> = vec![
//!     Arc::new(Int64Array::from(vec![1, 2, 3, 4])),
//!     Arc::new(Float64Array::from(vec![1.0, 2.0, 3.0, 4.0])),
//!     Arc::new(StringArray::from(vec!["a", "b", "c", "d"])),
//! ];
//! let schema = Schema::new(vec![
//!     Field::new("a", DataType::Int64, true),
//!     Field::new("b", DataType::Float64, true),
//!     Field::new("c", DataType::Utf8, true),
//! ]);
//! let input: RecordBatch = RecordBatch::try_new(Arc::new(schema), columns)?;
//!
//! // Ingest data.
//! statement.set_option(OptionStatement::TargetTable, "my_table".into())?;
//! statement.bind(input.clone())?;
//! statement.execute_update()?;
//!
//! // Extract data.
//! statement.set_sql_query("select * from my_table")?;
//! let output = statement.execute()?;
//! let schema = output.schema();
//! let output: Result<Vec<RecordBatch>, _> = output.collect();
//! let output = concat_batches(&schema, &output?)?;
//! assert_eq!(input, output);
//!
//! # Ok(())
//! # }
//! ```

// According to the ADBC specification, objects allow serialized access from
// multiple threads: one thread may make a call, and once finished, another
// thread may make a call. They do not allow concurrent access from multiple
// threads.
//
// In order to implement these semantics, all mutable FFI objects are wrapped
// in `Mutex`es. `FFI_Driver` is not wrapped in a `Mutex` because it is
// an immutable struct of function pointers. Wrapping the driver in a `Mutex`
// would prevent any parallelism between driver calls, which is not desirable.

use std::collections::HashSet;
use std::ffi::{CString, OsStr};
use std::ops::{Deref, DerefMut};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::pin::Pin;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use adbc_ffi::options::{
    check_status, get_option_bytes, get_option_string, set_option_connection, set_option_database,
    set_option_statement,
};
use arrow_array::ffi::{FFI_ArrowSchema, to_ffi};
use arrow_array::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
use arrow_array::{Array, RecordBatch, RecordBatchReader, StructArray};

use adbc_core::{
    Connection, Database, Driver, LoadFlags, Optionable, PartitionedResult, Statement,
    error::{Error, Result, Status},
    options::{self, AdbcVersion, InfoCode, OptionDatabase, OptionValue},
};
use adbc_ffi::driver_method;
use adbc_ffi::signal::SignalStackGuard;

use adbc_driver_manager::search::{DriverLibrary, parse_driver_uri};

use tracy_client::span;

use crate::Backend;

const ERR_CANCEL_UNSUPPORTED: &str =
    "Canceling connection or statement is not supported with ADBC 1.0.0";
const ERR_STATISTICS_UNSUPPORTED: &str = "Statistics are not supported with ADBC 1.0.0";

#[derive(Debug)]
struct ManagedDriverInner {
    backend: Backend,
    driver: adbc_ffi::FFI_AdbcDriver,
    version: AdbcVersion, // Driver version
    // The dynamic library must be kept loaded for the entire lifetime of the driver.
    // To avoid complex lifetimes we prefer to store it as part of this struct.
    // Besides, the `library` field must always appear after `driver` because of drop order:
    // `driver` has an implicit dependency on `library` and so it must be dropped
    // before `library` because otherwise `driver` would be full of dangling
    // function pointers.
    // See: https://doc.rust-lang.org/std/ops/trait.Drop.html#drop-order
    _library: Option<libloading::Library>,
}

/// Implementation of [Driver].
#[derive(Clone, Debug)]
pub struct ManagedDriver {
    inner: Pin<Arc<ManagedDriverInner>>,
}

impl ManagedDriver {
    /// Returns the [`AdbcVersion`] of this driver.
    pub fn version(&self) -> AdbcVersion {
        self.inner.version
    }

    /// Returns the [`Backend`](crate::Backend) of this driver.
    pub fn backend(&self) -> Backend {
        self.inner.backend
    }

    /// Load a driver from an initialization function.
    pub fn load_static(
        backend: Backend,
        init: &adbc_ffi::FFI_AdbcDriverInitFunc,
        version: AdbcVersion,
    ) -> Result<Self> {
        let driver = DriverLibrary::from_static_init(init).init_driver(version)?;
        let inner = Arc::pin(ManagedDriverInner {
            backend,
            driver,
            version,
            _library: None,
        });
        Ok(ManagedDriver { inner })
    }

    /// Load a driver either by name, filename, path, or via locating a toml manifest file.
    /// The `load_flags` control what directories are searched to locate a manifest.
    /// The `entrypoint` allows customizing the name of the driver initialization function
    /// if it is not the default `AdbcDriverInit` and isn't described in the loaded manifest.
    /// If not provided, an entrypoint will be searched for based on the driver's name.
    /// The `version` defines the ADBC revision to attempt to initialize.
    ///
    /// The full logic used here is as follows:
    ///  - if `name` has an extension: it is treated as a filename. If the load_flags does not
    ///    contain `LOAD_FLAG_ALLOW_RELATIVE_PATHS`, then relative paths will be rejected.
    ///    - if the extension is `toml` then we attempt to load the Driver Manifest, otherwise
    ///      we defer to the previous logic in [`Self::load_dynamic_from_filename`] which will
    ///      attempt to load the library
    ///  - if `name` does not have an extension but is an absolute path: we first check to see
    ///    if there is an existing file with the same name that *does* have a "toml" extension,
    ///    attempting to load that if it exists. Otherwise we just pass it to load_dynamic_from_filename.
    ///  - Finally, if there's no extension and it is not an absolute path, we will search through
    ///    the relevant directories (based on the set load flags) for a manifest file with this name,
    ///    and if one is not found we see if the name refers to a library on the LD_LIBRARY_PATH etc.
    pub fn load_from_name(
        backend: Backend,
        name: impl AsRef<OsStr>,
        entrypoint: Option<&[u8]>,
        version: AdbcVersion,
        load_flags: LoadFlags,
        additional_search_paths: Option<Vec<PathBuf>>,
    ) -> Result<Self> {
        let search_hit = DriverLibrary::search(name, load_flags, additional_search_paths)?;

        let entrypoint = search_hit.resolve_entrypoint(entrypoint).to_vec();
        Self::load_from_library(backend, search_hit.library, entrypoint.as_ref(), version)
    }

    /// Load a driver from a dynamic library filename.
    ///
    /// Will attempt to load the dynamic library located at `filename`, find the
    /// symbol with name `entrypoint` (defaults to `AdbcDriverInit` if `None`),
    /// and then create the driver using the resolved function.
    ///
    /// The `filename` argument may be either:
    /// - A library filename;
    /// - The absolute path to the library;
    /// - A relative (to the current working directory) path to the library.
    pub fn load_dynamic_from_filename(
        backend: Backend,
        filename: impl AsRef<OsStr>,
        entrypoint: Option<&[u8]>,
        version: AdbcVersion,
    ) -> Result<Self> {
        let entrypoint = DriverLibrary::derive_entrypoint(entrypoint, filename.as_ref());
        let library = DriverLibrary::load_library(filename)?;
        Self::load_from_library(backend, library, entrypoint.as_ref(), version)
    }

    fn load_from_library(
        backend: Backend,
        library: libloading::Library,
        entrypoint: &[u8],
        version: AdbcVersion,
    ) -> Result<ManagedDriver> {
        let driver =
            DriverLibrary::try_from_dynamic_library(&library, entrypoint)?.init_driver(version)?;
        let inner = Arc::pin(ManagedDriverInner {
            backend,
            driver,
            version,
            _library: Some(library),
        });
        Ok(ManagedDriver { inner })
    }

    /// Load a driver from a dynamic library name.
    ///
    /// Will attempt to load the dynamic library with the given `name`, find the
    /// symbol with name `entrypoint` (defaults to `AdbcDriverInit` if `None`),
    /// and then create the driver using the resolved function.
    ///
    /// The `name` should not include any platform-specific prefixes or suffixes.
    /// For example, use `adbc_driver_sqlite` rather than `libadbc_driver_sqlite.so`.
    pub fn load_dynamic_from_name(
        backend: Backend,
        name: impl AsRef<str>,
        entrypoint: Option<&[u8]>,
        version: AdbcVersion,
    ) -> Result<Self> {
        let entrypoint = DriverLibrary::derive_entrypoint_from_name(entrypoint, name.as_ref());
        let library = DriverLibrary::load_library_from_name(name.as_ref())?;
        Self::load_from_library(backend, library, entrypoint.as_ref(), version)
    }

    fn inner_ffi_driver(&self) -> &adbc_ffi::FFI_AdbcDriver {
        &self.inner.driver
    }

    /// Returns a new database using the loaded driver.
    fn database_new(&self) -> Result<adbc_ffi::FFI_AdbcDatabase> {
        let driver = self.inner_ffi_driver();
        let mut database = adbc_ffi::FFI_AdbcDatabase::default();

        // DatabaseNew
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(*driver, DatabaseNew);
        let status = unsafe { method(&mut database, &mut error) };
        check_status(status, error)?;

        Ok(database)
    }

    /// Initialize the given database using the loaded driver.
    fn database_init(
        &self,
        mut database: adbc_ffi::FFI_AdbcDatabase,
    ) -> Result<adbc_ffi::FFI_AdbcDatabase> {
        let driver = self.inner_ffi_driver();

        // DatabaseInit
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, DatabaseInit);
        let status = unsafe { method(&mut database, &mut error) };
        check_status(status, error)?;

        Ok(database)
    }
}

impl Driver for ManagedDriver {
    type DatabaseType = ManagedDatabase;

    fn new_database(&mut self) -> Result<Self::DatabaseType> {
        // Construct a new database.
        let database = self.database_new()?;
        // Initialize the database.
        let database = self.database_init(database)?;
        let inner = Arc::new(ManagedDatabaseInner {
            database: parking_lot::RwLock::new(database),
            driver: self.inner.clone(),
        });
        Ok(Self::DatabaseType { inner })
    }

    fn new_database_with_opts(
        &mut self,
        opts: impl IntoIterator<Item = (<Self::DatabaseType as Optionable>::Option, OptionValue)>,
    ) -> Result<Self::DatabaseType> {
        // Construct a new database.
        let mut database = self.database_new()?;
        // Set the options.
        {
            let driver = self.inner_ffi_driver();
            for (key, value) in opts {
                set_option_database(driver, &mut database, self.inner.version, key, value)?;
            }
        }
        // Initialize the database.
        let database = self.database_init(database)?;
        let inner = Arc::new(ManagedDatabaseInner {
            database: parking_lot::RwLock::new(database),
            driver: self.inner.clone(),
        });
        Ok(Self::DatabaseType { inner })
    }
}

struct ManagedDatabaseInner {
    /// The inner database object.
    ///
    /// Unlike [std::sync::RwLock], this lock is fair to writers and won't block them
    /// indefinitely if there is a steady flow of readers acquiring the lock.
    database: parking_lot::RwLock<adbc_ffi::FFI_AdbcDatabase>,
    driver: Pin<Arc<ManagedDriverInner>>,
}

/// SAFETY: an [adbc_ffi::FFI_AdbcDatabase] holds a raw pointer which is assumed by Rust to not be
/// [Sync] because it can be aliased and potentially mutated by another thread in the program. But
/// we declare the wrapper [ManagedDatabaseInner] as [Sync] because we ensure that all access to
/// the inner database is APPROPRIATELY serialized by a [parking_lot::RwLock].
///
/// DATABASE_SYNC_SAFETY: Functions using a read lock are trusting that the driver for a given
/// [Backend] fulfils thread-safety expectations that are stronger than what the ADBC spec
/// requires. If we don't know anything about the driver, we require a write lock to fully
/// synchronize access to the database and prevent concurrent calls.
///
/// For example, during connection initialization with `ConnectionInit`, we assume that it's safe
/// to call this function concurrently on multiple threads as long as they are using different
/// `FFI_AdbcConnection` objects. We also assume that the driver does not mutate the
/// `FFI_AdbcDatabase` object during connection initialization (or performs synchronization
/// internally), which means that it's safe to share a pointer to it across threads for this call.
unsafe impl Sync for ManagedDatabaseInner {}

impl Drop for ManagedDatabaseInner {
    fn drop(&mut self) {
        let _ = SignalStackGuard::new();
        let driver = &self.driver.driver;
        let mut database = self.database.write();
        let method = driver_method!(driver, DatabaseRelease);
        // TODO(alexandreyc): how should we handle `DatabaseRelease` failing?
        // See: https://github.com/apache/arrow-adbc/pull/1742#discussion_r1574388409
        unsafe { method(database.deref_mut(), null_mut()) };
    }
}

/// Implementation of [Database].
#[derive(Clone)]
pub struct ManagedDatabase {
    inner: Arc<ManagedDatabaseInner>,
}

impl ManagedDatabase {
    pub fn from_uri(
        backend: Backend,
        uri: &str,
        entrypoint: Option<&[u8]>,
        version: AdbcVersion,
        load_flags: LoadFlags,
        additional_search_paths: Option<Vec<PathBuf>>,
    ) -> Result<Self> {
        Self::from_uri_with_opts(
            backend,
            uri,
            entrypoint,
            version,
            load_flags,
            additional_search_paths,
            std::iter::empty(),
        )
    }

    pub fn from_uri_with_opts(
        backend: Backend,
        uri: &str,
        entrypoint: Option<&[u8]>,
        version: AdbcVersion,
        load_flags: LoadFlags,
        additional_search_paths: Option<Vec<PathBuf>>,
        opts: impl IntoIterator<Item = (<Self as Optionable>::Option, OptionValue)>,
    ) -> Result<Self> {
        let (driver, final_uri) = parse_driver_uri(uri)?;

        let mut drv = ManagedDriver::load_from_name(
            backend,
            driver,
            entrypoint,
            version,
            load_flags,
            additional_search_paths,
        )?;

        drv.new_database_with_opts(opts.into_iter().chain(std::iter::once((
            OptionDatabase::Uri,
            OptionValue::String(final_uri.to_string()),
        ))))
    }

    fn ffi_driver(&self) -> &adbc_ffi::FFI_AdbcDriver {
        &self.inner.driver.driver
    }

    fn driver_version(&self) -> AdbcVersion {
        self.inner.driver.version
    }

    fn backend(&self) -> Backend {
        self.inner.driver.backend
    }

    /// Returns a new connection using the loaded driver.
    fn connection_new(&self) -> Result<adbc_ffi::FFI_AdbcConnection> {
        let driver = self.ffi_driver();
        let mut connection = adbc_ffi::FFI_AdbcConnection::default();

        // ConnectionNew
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(*driver, ConnectionNew);
        let status = unsafe { method(&mut connection, &mut error) };
        check_status(status, error)?;

        Ok(connection)
    }

    /// Initialize the given connection using the loaded driver.
    fn connection_init(
        &self,
        mut connection: adbc_ffi::FFI_AdbcConnection,
    ) -> Result<adbc_ffi::FFI_AdbcConnection> {
        let driver = self.ffi_driver();

        // ConnectionInit
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let status = {
            let method = driver_method!(driver, ConnectionInit);
            match self.backend() {
                // We know the drivers we use for these backends have thread-safe connection
                // initialization, so we can allow multiple connections to be initialized in
                // parallel.
                // TODO(INC-5645): Add Snowflake back once we implement connection backpressure and pooling
                Backend::Snowflake | Backend::Spark | Backend::BigQuery => {
                    let database = {
                        let _span = span!("RwLock::read");
                        self.inner.database.read()
                    };
                    let _span = span!("ConnectionInit");
                    let database_ptr = database.deref() as *const adbc_ffi::FFI_AdbcDatabase
                        as *mut adbc_ffi::FFI_AdbcDatabase;
                    // SAFETY: see DATABASE_SYNC_SAFETY.
                    unsafe { method(&mut connection, database_ptr, &mut error) }
                }
                _ => {
                    let mut database = self.inner.database.write();
                    unsafe { method(&mut connection, &mut *database, &mut error) }
                }
            }
        };
        check_status(status, error)?;

        Ok(connection)
    }
}

impl Optionable for ManagedDatabase {
    type Option = OptionDatabase;

    fn get_option_bytes(&self, key: Self::Option) -> Result<Vec<u8>> {
        let driver = self.ffi_driver();
        let database = &mut self.inner.database.write();
        let method = driver_method!(driver, DatabaseGetOptionBytes);
        let populate = |key: *const c_char,
                        value: *mut u8,
                        length: *mut usize,
                        error: *mut adbc_ffi::FFI_AdbcError| unsafe {
            method(database.deref_mut(), key, value, length, error)
        };
        get_option_bytes(key, populate, driver)
    }

    fn get_option_double(&self, key: Self::Option) -> Result<f64> {
        let driver = self.ffi_driver();
        let mut database = self.inner.database.write();
        let key = CString::new(key.as_ref())?;
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let mut value: f64 = f64::default();
        let method = driver_method!(driver, DatabaseGetOptionDouble);
        let status = unsafe { method(database.deref_mut(), key.as_ptr(), &mut value, &mut error) };
        check_status(status, error)?;
        Ok(value)
    }

    fn get_option_int(&self, key: Self::Option) -> Result<i64> {
        let driver = self.ffi_driver();
        let mut database = self.inner.database.write();
        let key = CString::new(key.as_ref())?;
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let mut value: i64 = 0;
        let method = driver_method!(driver, DatabaseGetOptionInt);
        let status = unsafe { method(database.deref_mut(), key.as_ptr(), &mut value, &mut error) };
        check_status(status, error)?;
        Ok(value)
    }

    fn get_option_string(&self, key: Self::Option) -> Result<String> {
        let driver = self.ffi_driver();
        let mut database = self.inner.database.write();
        let method = driver_method!(driver, DatabaseGetOption);
        let populate = |key: *const c_char,
                        value: *mut c_char,
                        length: *mut usize,
                        error: *mut adbc_ffi::FFI_AdbcError| unsafe {
            method(database.deref_mut(), key, value, length, error)
        };
        get_option_string(key, populate, driver)
    }

    fn set_option(&mut self, key: Self::Option, value: OptionValue) -> Result<()> {
        let driver = self.ffi_driver();
        let mut database = self.inner.database.write();
        set_option_database(
            driver,
            database.deref_mut(),
            self.driver_version(),
            key,
            value,
        )
    }
}

impl Database for ManagedDatabase {
    type ConnectionType = ManagedConnection;

    fn new_connection(&self) -> Result<Self::ConnectionType> {
        // Construct a new connection.
        let connection = self.connection_new()?;
        // Initialize the connection.
        let connection = self.connection_init(connection)?;
        let inner = ManagedConnectionInner {
            connection: Mutex::new(connection),
            database: self.inner.clone(),
        };
        Ok(Self::ConnectionType {
            inner: Arc::new(inner),
        })
    }

    fn new_connection_with_opts(
        &self,
        opts: impl IntoIterator<Item = (<Self::ConnectionType as Optionable>::Option, OptionValue)>,
    ) -> Result<Self::ConnectionType> {
        // Construct a new connection.
        let mut connection = self.connection_new()?;
        // Set the options.
        {
            let driver = self.ffi_driver();
            for (key, value) in opts {
                set_option_connection(driver, &mut connection, self.driver_version(), key, value)?;
            }
        }
        // Initialize the connection.
        let connection = self.connection_init(connection)?;
        let inner = ManagedConnectionInner {
            connection: Mutex::new(connection),
            database: self.inner.clone(),
        };
        Ok(Self::ConnectionType {
            inner: Arc::new(inner),
        })
    }
}

struct ManagedConnectionInner {
    connection: Mutex<adbc_ffi::FFI_AdbcConnection>,
    database: Arc<ManagedDatabaseInner>,
}

impl Drop for ManagedConnectionInner {
    fn drop(&mut self) {
        let _ = SignalStackGuard::new();
        let driver = &self.database.driver.driver;
        let mut connection = self.connection.lock().unwrap();
        let method = driver_method!(driver, ConnectionRelease);
        // TODO(alexandreyc): how should we handle `ConnectionRelease` failing?
        // See: https://github.com/apache/arrow-adbc/pull/1742#discussion_r1574388409
        unsafe { method(connection.deref_mut(), null_mut()) };
    }
}

/// Implementation of [Connection].
#[derive(Clone)]
pub struct ManagedConnection {
    inner: Arc<ManagedConnectionInner>,
}

impl ManagedConnection {
    fn ffi_driver(&self) -> &adbc_ffi::FFI_AdbcDriver {
        &self.inner.database.driver.driver
    }

    fn driver_version(&self) -> AdbcVersion {
        self.inner.database.driver.version
    }
}

impl Optionable for ManagedConnection {
    type Option = options::OptionConnection;

    fn get_option_bytes(&self, key: Self::Option) -> Result<Vec<u8>> {
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let method = driver_method!(driver, ConnectionGetOptionBytes);
        let populate = |key: *const c_char,
                        value: *mut u8,
                        length: *mut usize,
                        error: *mut adbc_ffi::FFI_AdbcError| unsafe {
            method(connection.deref_mut(), key, value, length, error)
        };
        get_option_bytes(key, populate, driver)
    }

    fn get_option_double(&self, key: Self::Option) -> Result<f64> {
        let key = CString::new(key.as_ref())?;
        let mut value: f64 = f64::default();
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetOptionDouble);
        let status =
            unsafe { method(connection.deref_mut(), key.as_ptr(), &mut value, &mut error) };
        check_status(status, error)?;
        Ok(value)
    }

    fn get_option_int(&self, key: Self::Option) -> Result<i64> {
        let key = CString::new(key.as_ref())?;
        let mut value: i64 = 0;
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetOptionInt);
        let status =
            unsafe { method(connection.deref_mut(), key.as_ptr(), &mut value, &mut error) };
        check_status(status, error)?;
        Ok(value)
    }

    fn get_option_string(&self, key: Self::Option) -> Result<String> {
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let method = driver_method!(driver, ConnectionGetOption);
        let populate = |key: *const c_char,
                        value: *mut c_char,
                        length: *mut usize,
                        error: *mut adbc_ffi::FFI_AdbcError| unsafe {
            method(connection.deref_mut(), key, value, length, error)
        };
        get_option_string(key, populate, driver)
    }

    fn set_option(&mut self, key: Self::Option, value: OptionValue) -> Result<()> {
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        set_option_connection(
            driver,
            connection.deref_mut(),
            self.driver_version(),
            key,
            value,
        )
    }
}

impl Connection for ManagedConnection {
    type StatementType = ManagedStatement;

    fn new_statement(&mut self) -> Result<Self::StatementType> {
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut statement = adbc_ffi::FFI_AdbcStatement::default();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementNew);
        let status = unsafe { method(connection.deref_mut(), &mut statement, &mut error) };
        check_status(status, error)?;

        let inner = Arc::new(ManagedStatementInner {
            statement: Mutex::new(statement),
            connection: self.inner.clone(),
        });

        Ok(Self::StatementType { inner })
    }

    fn cancel(&mut self) -> Result<()> {
        if let AdbcVersion::V100 = self.driver_version() {
            return Err(Error::with_message_and_status(
                ERR_CANCEL_UNSUPPORTED,
                Status::NotImplemented,
            ));
        }
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionCancel);
        let status = unsafe { method(connection.deref_mut(), &mut error) };
        check_status(status, error)
    }

    fn commit(&mut self) -> Result<()> {
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionCommit);
        let status = unsafe { method(connection.deref_mut(), &mut error) };
        check_status(status, error)
    }

    fn rollback(&mut self) -> Result<()> {
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionRollback);
        let status = unsafe { method(connection.deref_mut(), &mut error) };
        check_status(status, error)
    }

    fn get_info(&self, codes: Option<HashSet<InfoCode>>) -> Result<impl RecordBatchReader> {
        let mut stream = FFI_ArrowArrayStream::empty();
        let codes: Option<Vec<u32>> =
            codes.map(|codes| codes.iter().map(|code| code.into()).collect());
        let (codes_ptr, codes_len) = codes
            .as_ref()
            .map(|c| (c.as_ptr(), c.len()))
            .unwrap_or((null(), 0));
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetInfo);
        let status = unsafe {
            method(
                connection.deref_mut(),
                codes_ptr,
                codes_len,
                &mut stream,
                &mut error,
            )
        };
        check_status(status, error)?;
        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }

    fn get_objects(
        &self,
        depth: options::ObjectDepth,
        catalog: Option<&str>,
        db_schema: Option<&str>,
        table_name: Option<&str>,
        table_type: Option<Vec<&str>>,
        column_name: Option<&str>,
    ) -> Result<impl RecordBatchReader> {
        let catalog = catalog.map(CString::new).transpose()?;
        let db_schema = db_schema.map(CString::new).transpose()?;
        let table_name = table_name.map(CString::new).transpose()?;
        let column_name = column_name.map(CString::new).transpose()?;
        let table_type = table_type
            .map(|t| {
                t.iter()
                    .map(|x| CString::new(*x))
                    .collect::<std::result::Result<Vec<CString>, _>>()
            })
            .transpose()?;

        let catalog_ptr = catalog.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let db_schema_ptr = db_schema.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let table_name_ptr = table_name.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let column_name_ptr = column_name.as_ref().map(|c| c.as_ptr()).unwrap_or(null());

        let mut table_type_ptrs = table_type
            .as_ref()
            .map(|v| v.iter().map(|c| c.as_ptr()))
            .map(|c| c.collect::<Vec<_>>());
        let table_type_ptr = match table_type_ptrs.as_mut() {
            None => null(),
            Some(t) => {
                t.push(null());
                t.as_ptr()
            }
        };

        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetObjects);
        let mut stream = FFI_ArrowArrayStream::empty();

        let status = unsafe {
            method(
                connection.deref_mut(),
                depth.into(),
                catalog_ptr,
                db_schema_ptr,
                table_name_ptr,
                table_type_ptr,
                column_name_ptr,
                &mut stream,
                &mut error,
            )
        };
        check_status(status, error)?;

        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }

    fn get_statistics(
        &self,
        catalog: Option<&str>,
        db_schema: Option<&str>,
        table_name: Option<&str>,
        approximate: bool,
    ) -> Result<impl RecordBatchReader> {
        if let AdbcVersion::V100 = self.driver_version() {
            return Err(Error::with_message_and_status(
                ERR_STATISTICS_UNSUPPORTED,
                Status::NotImplemented,
            ));
        }

        let catalog = catalog.map(CString::new).transpose()?;
        let db_schema = db_schema.map(CString::new).transpose()?;
        let table_name = table_name.map(CString::new).transpose()?;

        let catalog_ptr = catalog.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let db_schema_ptr = db_schema.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let table_name_ptr = table_name.as_ref().map(|c| c.as_ptr()).unwrap_or(null());

        let mut stream = FFI_ArrowArrayStream::empty();
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetStatistics);
        let status = unsafe {
            method(
                connection.deref_mut(),
                catalog_ptr,
                db_schema_ptr,
                table_name_ptr,
                approximate as c_char,
                &mut stream,
                &mut error,
            )
        };
        check_status(status, error)?;
        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }

    fn get_statistic_names(&self) -> Result<impl RecordBatchReader> {
        if let AdbcVersion::V100 = self.driver_version() {
            return Err(Error::with_message_and_status(
                ERR_STATISTICS_UNSUPPORTED,
                Status::NotImplemented,
            ));
        }
        let mut stream = FFI_ArrowArrayStream::empty();
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetStatisticNames);
        let status = unsafe { method(connection.deref_mut(), &mut stream, &mut error) };
        check_status(status, error)?;
        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }

    fn get_table_schema(
        &self,
        catalog: Option<&str>,
        db_schema: Option<&str>,
        table_name: &str,
    ) -> Result<arrow_schema::Schema> {
        let catalog = catalog.map(CString::new).transpose()?;
        let db_schema = db_schema.map(CString::new).transpose()?;
        let table_name = CString::new(table_name)?;

        let catalog_ptr = catalog.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let db_schema_ptr = db_schema.as_ref().map(|c| c.as_ptr()).unwrap_or(null());
        let table_name_ptr = table_name.as_ptr();

        let mut schema = FFI_ArrowSchema::empty();
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetTableSchema);
        let status = unsafe {
            method(
                connection.deref_mut(),
                catalog_ptr,
                db_schema_ptr,
                table_name_ptr,
                &mut schema,
                &mut error,
            )
        };
        check_status(status, error)?;
        Ok((&schema).try_into()?)
    }

    fn get_table_types(&self) -> Result<impl RecordBatchReader> {
        let mut stream = FFI_ArrowArrayStream::empty();
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionGetTableTypes);
        let status = unsafe { method(connection.deref_mut(), &mut stream, &mut error) };
        check_status(status, error)?;
        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }

    fn read_partition(&self, partition: impl AsRef<[u8]>) -> Result<impl RecordBatchReader> {
        let mut stream = FFI_ArrowArrayStream::empty();
        let driver = self.ffi_driver();
        let mut connection = self.inner.connection.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, ConnectionReadPartition);
        let partition = partition.as_ref();
        let status = unsafe {
            method(
                connection.deref_mut(),
                partition.as_ptr(),
                partition.len(),
                &mut stream,
                &mut error,
            )
        };
        check_status(status, error)?;
        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }
}

struct ManagedStatementInner {
    statement: Mutex<adbc_ffi::FFI_AdbcStatement>,
    connection: Arc<ManagedConnectionInner>,
}
/// Implementation of [Statement].
#[derive(Clone)]
pub struct ManagedStatement {
    inner: Arc<ManagedStatementInner>,
}

impl ManagedStatement {
    fn driver_version(&self) -> AdbcVersion {
        self.inner.connection.database.driver.version
    }

    fn ffi_driver(&self) -> &adbc_ffi::FFI_AdbcDriver {
        &self.inner.connection.database.driver.driver
    }
}

impl Statement for ManagedStatement {
    fn bind(&mut self, batch: RecordBatch) -> Result<()> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementBind);
        let batch: StructArray = batch.into();
        let (mut array, mut schema) = to_ffi(&batch.to_data())?;
        let status = unsafe { method(statement.deref_mut(), &mut array, &mut schema, &mut error) };
        check_status(status, error)?;
        Ok(())
    }

    fn bind_stream(&mut self, reader: Box<dyn RecordBatchReader + Send>) -> Result<()> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementBindStream);
        let mut stream = FFI_ArrowArrayStream::new(reader);
        let status = unsafe { method(statement.deref_mut(), &mut stream, &mut error) };
        check_status(status, error)?;
        Ok(())
    }

    fn cancel(&mut self) -> Result<()> {
        if let AdbcVersion::V100 = self.driver_version() {
            return Err(Error::with_message_and_status(
                ERR_CANCEL_UNSUPPORTED,
                Status::NotImplemented,
            ));
        }
        let driver = self.ffi_driver();
        // Workaround before [1] is fixed.
        //
        // Drivers are supposed to handle thread-safety internally.
        // It's an overkill for the Rust code to always take a lock
        // to make calls, but it's true that not all drivers handle
        // thread-safety well. Nevertheless, for cancellation of
        // long-running statements (a call to `execute` that holds a
        // lock while running), we have not choice but to make a call
        // while a lock is held. This happens as a response to user-
        // -triggered Ctrl+C and in those situation we are more
        // concerned about actually cancelling statements than shutting
        // down cleanly (without cancellation errors and even segfaults).
        // Join the discussion in [1] to improve this situation.
        //
        // [1] https://github.com/apache/arrow-adbc/issues/3454
        let mut statement = unsafe {
            let mutex_ptr = &self.inner.statement as *const Mutex<adbc_ffi::FFI_AdbcStatement>
                as *mut Mutex<adbc_ffi::FFI_AdbcStatement>;
            (*mutex_ptr).get_mut().unwrap_unchecked()
        };
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementCancel);
        let status = unsafe { method(statement.deref_mut(), &mut error) };
        check_status(status, error)
    }

    fn execute(&mut self) -> Result<impl RecordBatchReader> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementExecuteQuery);
        let mut stream = FFI_ArrowArrayStream::empty();
        let status = unsafe { method(statement.deref_mut(), &mut stream, null_mut(), &mut error) };
        check_status(status, error)?;
        let reader = ArrowArrayStreamReader::try_new(stream)?;
        Ok(reader)
    }

    fn execute_schema(&mut self) -> Result<arrow_schema::Schema> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementExecuteSchema);
        let mut schema = FFI_ArrowSchema::empty();
        let status = unsafe { method(statement.deref_mut(), &mut schema, &mut error) };
        check_status(status, error)?;
        Ok((&schema).try_into()?)
    }

    fn execute_update(&mut self) -> Result<Option<i64>> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementExecuteQuery);
        let mut rows_affected: i64 = -1;
        let status = unsafe {
            method(
                statement.deref_mut(),
                null_mut(),
                &mut rows_affected,
                &mut error,
            )
        };
        check_status(status, error)?;
        Ok((rows_affected != -1).then_some(rows_affected))
    }

    fn execute_partitions(&mut self) -> Result<PartitionedResult> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementExecutePartitions);
        let mut schema = FFI_ArrowSchema::empty();
        let mut partitions = adbc_ffi::FFI_AdbcPartitions::default();
        let mut rows_affected: i64 = -1;
        let status = unsafe {
            method(
                statement.deref_mut(),
                &mut schema,
                &mut partitions,
                &mut rows_affected,
                &mut error,
            )
        };
        check_status(status, error)?;

        let result = PartitionedResult {
            partitions: partitions.into(),
            schema: (&schema).try_into()?,
            rows_affected,
        };

        Ok(result)
    }

    fn get_parameter_schema(&self) -> Result<arrow_schema::Schema> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementGetParameterSchema);
        let mut schema = FFI_ArrowSchema::empty();
        let status = unsafe { method(statement.deref_mut(), &mut schema, &mut error) };
        check_status(status, error)?;
        Ok((&schema).try_into()?)
    }

    fn prepare(&mut self) -> Result<()> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementPrepare);
        let status = unsafe { method(statement.deref_mut(), &mut error) };
        check_status(status, error)?;
        Ok(())
    }

    fn set_sql_query(&mut self, query: impl AsRef<str>) -> Result<()> {
        let query = CString::new(query.as_ref())?;
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementSetSqlQuery);
        let status = unsafe { method(statement.deref_mut(), query.as_ptr(), &mut error) };
        check_status(status, error)?;
        Ok(())
    }

    fn set_substrait_plan(&mut self, plan: impl AsRef<[u8]>) -> Result<()> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementSetSubstraitPlan);
        let plan = plan.as_ref();
        let status =
            unsafe { method(statement.deref_mut(), plan.as_ptr(), plan.len(), &mut error) };
        check_status(status, error)?;
        Ok(())
    }
}

impl Optionable for ManagedStatement {
    type Option = options::OptionStatement;

    fn get_option_bytes(&self, key: Self::Option) -> Result<Vec<u8>> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let method = driver_method!(driver, StatementGetOptionBytes);
        let populate = |key: *const c_char,
                        value: *mut u8,
                        length: *mut usize,
                        error: *mut adbc_ffi::FFI_AdbcError| unsafe {
            method(statement.deref_mut(), key, value, length, error)
        };
        get_option_bytes(key, populate, driver)
    }

    fn get_option_double(&self, key: Self::Option) -> Result<f64> {
        let key = CString::new(key.as_ref())?;
        let mut value: f64 = f64::default();
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementGetOptionDouble);
        let status = unsafe { method(statement.deref_mut(), key.as_ptr(), &mut value, &mut error) };
        check_status(status, error)?;
        Ok(value)
    }

    fn get_option_int(&self, key: Self::Option) -> Result<i64> {
        let key = CString::new(key.as_ref())?;
        let mut value: i64 = 0;
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let mut error = adbc_ffi::FFI_AdbcError::with_driver(driver);
        let method = driver_method!(driver, StatementGetOptionInt);
        let status = unsafe { method(statement.deref_mut(), key.as_ptr(), &mut value, &mut error) };
        check_status(status, error)?;
        Ok(value)
    }

    fn get_option_string(&self, key: Self::Option) -> Result<String> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let method = driver_method!(driver, StatementGetOption);
        let populate = |key: *const c_char,
                        value: *mut c_char,
                        length: *mut usize,
                        error: *mut adbc_ffi::FFI_AdbcError| unsafe {
            method(statement.deref_mut(), key, value, length, error)
        };
        get_option_string(key, populate, driver)
    }

    fn set_option(&mut self, key: Self::Option, value: OptionValue) -> Result<()> {
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        set_option_statement(
            driver,
            statement.deref_mut(),
            self.driver_version(),
            key,
            value,
        )
    }
}

impl Drop for ManagedStatement {
    fn drop(&mut self) {
        let _ = SignalStackGuard::new();
        let driver = self.ffi_driver();
        let mut statement = self.inner.statement.lock().unwrap();
        let method = driver_method!(driver, StatementRelease);
        // TODO(alexandreyc): how should we handle `StatementRelease` failing?
        // See: https://github.com/apache/arrow-adbc/pull/1742#discussion_r1574388409
        unsafe { method(statement.deref_mut(), null_mut()) };
    }
}
