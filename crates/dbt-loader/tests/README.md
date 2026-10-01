# dbt-loader integration tests

These tests verify correctness of the Jinja macros we ship to users via
`dbt_macro_assets/`. All tests share a single test binary (`main.rs`) and
the `macro_test_harness` module.

## Structure

```
tests/
  main.rs                   # test binary root
  macro_test_harness/       # shared harness for building Jinja environments
  macros/                   # isolated macro tests
  materializations/         # materialization tests (with mocked adapter)
```

### `macros/`

Tests for individual macros or small macro groups. Each file maps to a
functional area in `dbt_macro_assets/` (e.g. `relations.rs` tests macros
from the `relations/` directory, `persist_docs.rs` tests macros from
`adapters/persist_docs.sql`).

Tests here inject only the specific macros under test via `with_macro` or
`with_macro_at_path`, keeping the environment minimal. Use inner `mod`
blocks to group by adapter (e.g. `mod databricks { ... }`).

### `materializations/`

Tests for full materialization macros (view, table, incremental, etc.).
These use `load_all_macros()` to load the complete macro set for an adapter,
then mock adapter API calls (`get_relation`, `execute`, `drop_relation`, etc.)
to simulate different scenarios (no existing relation, existing view, existing
table with full refresh, etc.).

Each file covers one materialization type. Adapter variants are inner `mod`
blocks within the file.

## Macro test harness

`macro_test_harness/mod.rs` provides `MacroTestHarness` and its builder.
Key APIs:

- `MacroTestHarness::for_adapter(AdapterType)` starts a builder.
- `.with_macro(package, name, sql)` injects an inline macro definition.
- `.with_macro_at_path(package, name, sql, path)` injects a macro with an
  explicit file path (use with `include_str!` for real assets).
- `.load_all_macros()` loads every macro the adapter ships with.
- `.with_stub_functions()` registers stubs for `write`, `log`,
  `store_result` (needed by materializations).
- `.build()` produces a `MacroTestHarness`.
- `harness.render(template, ctx)` renders a Jinja string.
- `harness.mock()` accesses the mock adapter object for setting up
  `.on(method, closure)` handlers and asserting calls after render.

## Adding new tests

1. Identify the macro's source path under `dbt_macro_assets/`.
2. Find or create the corresponding file under `macros/` or
   `materializations/`.
3. Add an adapter-specific inner `mod` if needed.
4. For isolated macro tests: inject only what you need with `with_macro` /
   `with_macro_at_path`.
5. For materialization tests: use `load_all_macros()` and mock adapter calls.
