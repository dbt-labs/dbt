# DuckDB connect-time configuration

Use `config_options` in a DuckDB profile for options that must take effect when
DuckDB opens the database, before installing or loading extensions:

```yaml
default:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.duckdb
      config_options:
        allow_unsigned_extensions: true
        memory_limit: 1GB
        threads: 2
      settings:
        threads: 1
```

For example, a locally built unsigned extension requires
`allow_unsigned_extensions: true` at database open. Setting it under `settings`
is too late. Keep signature verification enabled for signed extensions.
Extension loading also requires an extension-capable DuckDB driver.

`config_options` accepts strings, booleans, numbers, and flat lists of those
values. Scalars are passed as driver option strings; lists use DuckDB's list
representation, preserving commas, quotes, backslashes, and whitespace in string
elements. For example:

```yaml
config_options:
  extension_directories: ['/opt/duckdb-ext', '/tmp/extra-extensions']
```

Individual nulls, mappings, nested lists, non-finite numbers, and strings
containing NUL characters are rejected. Omitting `config_options`, setting the
whole field to `null`, or using `{}` leaves the default connection behavior.
DuckDB determines which option names and values are supported by the installed
driver version. Option names are case-insensitive; supplying two spellings of the
same name (such as `threads` and `THREADS`) is rejected. Database initialization
errors retain DuckDB's error and include `config_options` context when this field
contains options used for the connection.

Connection routing and MotherDuck authentication remain controlled by the
adapter. `path`, `uri`, `username`, `password`, `motherduck_token`, and
`motherduck_saas_mode` cannot be supplied in `config_options` (case-insensitive).
Use the profile's `path` and existing MotherDuck configuration instead.

`settings` continues to execute SQL `SET` statements after opening the database
and processing extensions. If an option appears in both fields, its
`config_options` value applies first and its `settings` value applies later,
subject to DuckDB's restrictions on runtime changes. In the first example,
DuckDB opens with two threads and the later `SET` changes that to one.

Options under `config_options` are excluded from the connection summary printed
by `dbt debug`.

This field does not register extension repositories or replace
`CREATE EXTENSION REPOSITORY`.

## Validation

Unit tests cover profile round trips, driver options, list encoding, invalid
inputs, and existing MotherDuck behavior:

```sh
cargo test -p dbt-auth duckdb::
cargo test -p dbt-schemas duckdb
cargo test -p dbt-adapter database_error_context
```

A separate smoke test opens an in-memory database through the actual DuckDB ADBC
driver, verifies startup settings and list conversion, and applies a later SQL
setting. It uses a system-installed driver when available and otherwise downloads
the driver into the local cache. Run with `--nocapture` to see its version:

```sh
cargo test -p dbt-auth connect_time_options_work_with_duckdb -- --ignored --nocapture
```

To check the error context with invalid option names and values against the real
DuckDB driver:

```sh
cargo test -p dbt-adapter duckdb_config_options_driver_errors -- --ignored --nocapture
```
