![logo](https://raw.githubusercontent.com/apache/arrow/refs/heads/main/docs/source/_static/favicon.ico)

[![crates.io](https://img.shields.io/crates/v/adbc_snowflake.svg)](https://crates.io/crates/adbc_snowflake)
[![docs.rs](https://docs.rs/adbc_snowflake/badge.svg)](https://docs.rs/c)

# Rust wrapper for ADBC drivers

All [ADBC (Arrow Database Connectivity)](https://arrow.apache.org/adbc/) drivers
(shared libraries) are loaded dynamically.

Drivers are automatically downloaded from the dbt CDN when dbt needs to connect to a
data warehouse.

## Snowflake example

We use the
[ADBC Snowflake Go driver](https://github.com/apache/arrow-adbc/tree/main/go/adbc/driver/snowflake)
for [Snowflake](https://www.snowflake.com).

```rust,no_run
use adbc_core::options::AdbcVersion;
use adbc_core::{Connection, Statement};
use dbt_adbc::{connection, database, driver, Backend};
use arrow_array::{cast::AsArray, types::Decimal128Type};

# fn main() -> Result<(), Box<dyn std::error::Error>> {

// Load the driver
let mut driver = driver::Builder::new(Backend::Snowflake)
    .with_version(AdbcVersion::V110)
    .build()?;

// Construct a database using system configuration
let mut database = database::Builder::from_snowsql_config()?.build(&mut driver)?;
// ..or from a URI.
let mut builder = database::Builder::new(Backend::Snowflake);
builder.with_parse_uri("my_account/my_db/my_schema?role=R&warehouse=WH")?;
let mut database = builder.build(&mut driver)?;

// Create a connection to the database
let mut connection = connection::Builder::default().build(&mut database)?;

// Construct a statement to execute a query
let mut statement = connection.new_statement()?;

// Execute a query
statement.set_sql_query("SELECT 21 + 21")?;
let mut reader = statement.execute()?;

// Check the result
let batch = reader.next().expect("a record batch")?;
assert_eq!(
    batch.column(0).as_primitive::<Decimal128Type>().value(0),
    42
);

# Ok(()) }
```

## Bumping an ADBC driver version

1. Get the target version: caller-provided, or the `DBT_VERSION` output of the
   "Determine ADBC & dbt Version" job in the `Release ADBC` workflow run in `dbt-labs/fs`.
2. Add `<backend>-<version>` to `drivers.toml`.
3. Bump the matching `*_DRIVER_VERSION` const in `src/lib.rs`.
4. Run `cargo run --bin adbc-sync -p dbt-adbc` to regenerate `checksums.rs` for real.
   Only run this once the release artifacts exist on the CDN, or the new entries are
   silently skipped (404) instead of hashed.
5. Add a changelog entry with `changie new --kind Dependencies --body "..." --interactive=false`.
   Not every driver bump has a dbt-core/fusion issue ticket, so `issue`/`author` are
   often empty — see root `AGENTS.md` for the dummy-value-then-edit trick required to
   get an empty string past `changie`'s non-interactive validation. We no longer add
   changelogs to the `fs/sa/.changes` directory.

## Working on ADBC drivers

Most ADBC drivers we use are written in Go, using the official Go SDKs for each
data warehouse backend.

### Changing driver code

When you need to extend the funcionality of an ADBC diver, you can do so by
cloning our fork of `apache/arrow-adbc` and modifying the Go code of the driver
you want to work on.

```bash
git clone git@github.com:dbt-labs/arrow-adbc.git
cd arrow-adbc
cd go/adbc/driver/bigquery  # source directory
go build -v ./...           # build go
cd go/adbc/pkg              # build directory for all Go drivers
make clean || make libadbc_driver_bigquery.dylib
```

Use `make libadbc_driver_bigquery.so` to build the Linux driver, or
`make libadbc_driver_bigquery.dll` to build the Windows driver.

You can replace the `bigquery` driver with `snowflake` if you're building the
Snowflake driver instead.

### Loading the in-development driver

To get `fs` to skip loading the production drivers from the CDN, you need to set
an environment variable that is checked by, and only by debug builds of `fs` and
link to the local driver from the `lib/` folder at the root of the `fs` repo.

```bash
export DISABLE_CDN_DRIVER_CACHE=true
```
When the non-CDN driver is found,
you should see a message like this when you run `fs` or tests that trigger the
loading of ADBC drivers:

```
$ cargo test -p dbt-adbc -- --nocapture
...
WARNING: BigQuery ADBC driver is being loaded from /Users/felipe/code/fs/lib in debug mode.
...
```

The driver is being looked for as a file with the bare driver name with no `lib` prefix or extension (e.g. `adbc_driver_bigquery`, not `libadbc_driver_bigquery.dylib`).

The driver is being looked for in two locations (assuming you keep all the repos in the `~/code` folder).
* First, as `~/code/arrow-adbc/go/adbc/pkg/adbc_driver_bigquery`
* Second, *only if the above folder does not exist*, as `~/code/fs/lib/adbc_driver_bigquery`

Since the 2nd option is pre-empted by the first, it is only useful for people who do not do driver development
(do not have `~/code/arrow-adbc`) but need to try some pre-built non-CDN driver.

Also, when the 1st option is in force, we automatically attempt to rebuild the drivers.
To disable that, set
```bash
export DISABLE_AUTO_DRIVER_REBUILD=true
```

Change the path to the driver to match your local setup and the driver extension
that your system uses (.dylib, .so, .dll).

### Sample commands:
- `DISABLE_AUTO_DRIVER_REBUILD= DISABLE_CDN_DRIVER_CACHE=0 ../fs3/target/debug/dbt seed`  -- dbt project sibling to the fs dir, no auto makefile invocation, will look for local drivers
- `DISABLE_CDN_DRIVER_CACHE= ../fs3/target/debug/dbt seed` -- will still use CDN provided drivers
- `DISABLE_CDN_DRIVER_CACHE=0 cargo run --bin dbt seed` -- when dbt project is a subdir of the `fs` directory we can still look for the arrow-adbc dir, lib dir, and automake (or not)


### Publishing the changes

When you're done with your changes, open a PR against [](https://github.com/dbt-labs/arrow-adbc),
after review and merge, trigger an `ADBC Release` workflow in the `fs`
repository and bump the driver version in `fs`.


### DuckDB specifics

Working on the DuckDB driver is different from others, since it is bundled with DuckDB itself.

For reference, the build process in CI is governed by `~/code/fs/.github/workflows/build-duckdb-driver.yml`
and  `~/code/fs/scripts/build_cmake_duckdb_macos.sh` (and its siblings for Windows and Linux).

As a preliminary for building DuckDB locally, have these cloned in `~/code`:
* `arrow-adbc` from our fork at https://github.com/dbt-labs/arrow-adbc
* `duckdb` from their repo at https://github.com/duckdb/duckdb

Then, in `~/code`:
```bash
ln -s arrow-adbc adbc

./fs/scripts/build_cmake_duckdb_macos.sh --build-extension "dylib" --full-version  "0.18.0+dbt99.1.0" --build-target "x86_64-apple-darwin" --workspace "$PWD"
```
(replace `0.18.0+dbt99.1.0` as desired, this is the label of the driver being built)

When successful, this should build the driver as
`~/code/adbc_build/duckdb/adbc_driver_duckdb-0.18.0+dbt99.1.0-x86_64-apple-darwin.dylib`.

To load this driver when fs runs,
* place it at the 1st location option above:
  ```
  ln -s ~/code/adbc_build/duckdb/adbc_driver_duckdb-0.18.0+dbt99.1.0-x86_64-apple-darwin.dylib ~/code/arrow-adbc/go/adbc/pkg/duckdb
  ```
  (Reminder: the loader searches for a file named `duckdb` in that directory — not `libduckdb.dylib`)
* Make sure `DISABLE_CDN_DRIVER_CACHE=true` and `DISABLE_AUTO_DRIVER_REBUILD=true` are set


### Using the REPL

We expose a basic REPL that is tightly coupled with the drivers in order to execute queries against and enable a tighter feedback loop.

To invoke this REPL (for Databricks):

```bash
$ cargo adbc --profile databricks
```

You'll need a `adbc_config.toml` file somewhere with all your profiles. An example is provided in `adbc_conf.toml.example`.

Follow the prompts within the REPL for features such as executing queries, inspect schemas, and check `RecordBatch` objects extracted from the `RecordBatchReader`.
