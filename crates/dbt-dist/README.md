# Optional runtime libraries

`optional-packages.toml` declares the version and compressed CDN checksum of each
driver library shipped with a dbt release. `cargo ci packages` verifies and stages
the libraries, creates Homebrew resource archives, and emits per-target
`fs-packages-v<VERSION>-<TARGET>.json` manifests. The staging directory is a
generated output tree and is replaced on each run. Libraries are included in the
CLI archives and the `dbt` Python wheel; the runner archive is independent.

## Layout

Every channel performs a prefixed install, and nothing is ever written outside the prefix: `dbt` at `<prefix>/bin/dbt` and the
libraries at `<prefix>/lib/dbt/`. There is no per-channel directory policy and no
distribution detection at load time. `dbt-adbc` climbs from the executable to the
nearest `lib/` directory and loads from its `dbt/` subdirectory.

| Channel | `<prefix>` |
| --- | --- |
| Standalone / Unclaimed | the install script's `--to` / `-To` prefix (`~/.local` by default); updating an install from before prefixes uses the parent of its `bin/`, or migrates a non-`bin` directory into `<dir>/bin` and leaves a link behind |
| Homebrew | the keg, `Cellar/dbt/<version>` |
| winget | the extracted package folder (`dbt.exe` beside `lib\dbt\`) |
| PyPI | the environment prefix; the wheel's data scheme installs to `<prefix>/lib/dbt/` |

Inside the Python extension the executable is the interpreter, so
`dbt_python_core::register` locates `lib/dbt` from the installed `dbt` package and
hands it to `dbt_adbc::driver::set_libs_directory`.

The managed (bridge) connection loads `adbc_driver_bridge` from this directory.
Other drivers are loaded from it before any system or CDN lookup, and the previous
strategy runs unchanged when the library is absent, including on-demand CDN
downloads and DuckDB's system lookup.

## Adapter-wheel contract

A future `dbt-<adapter>` wheel depends on `dbt` and owns only its library file in
`<prefix>/lib/dbt/`, through the wheel data scheme (`<name>-<ver>.data/data/lib/dbt/`).
The filename and entrypoint follow that backend's existing ADBC contract:

- `dbt-snowflake`: `lib/dbt/libadbc_driver_snowflake.so`
- `dbt-postgres`: `lib/dbt/libadbc_driver_postgresql.so`
- Windows uses `.dll` without `lib`; macOS uses the `lib` prefix and `.dylib`.

Adapter wheels must not replace `dbt/__init__.py`, its extension module, or any other
package's files, and must list every file they own in `RECORD` so pip can upgrade or
uninstall each adapter independently. Installers and the native updater replace only
the files their archive carries, so adapter libraries stay in place. Adapter-wheel
production is a separate rollout.

Bundled libraries update in lockstep with dbt. No independent version stamps or update
mechanism are provided. The download-at-install sdist returns a prebuilt wheel
containing its libraries and verifies their per-file checksums in addition to the
complete wheel checksum.
