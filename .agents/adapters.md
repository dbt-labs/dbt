Paths in this file are relative to the directory containing `AGENTS.md`.

# Adapters architecture

Differently from dbt Core v1, adapters in Core v2 are "verticalized", which means we have "verticals" of functionality like auth, incremental materializations, relation configs etc. And each platform might have its own implementation of that vertical, ideally sharing as much of the code as possible.

## Crate overview

The code is stacked as follows, bottom up:
- `dbt-adbc`: Responsible for low-level database connectivity via Arrow ADBC. It contains logic for installing and loading drivers, as well as the primitives needed for managing a database connection like `Statement`, `Connection` and `Database`. We strongly prefer not to add platform-specific code here, and most platform-specific behavior should be tuned via ADBC database/connection/statement options by the application layer. In concrete terms, DO NOT EVER add any new methods, traits or functionality to `dbt-adbc`. The only exception is ADBC option key constants, such as in `bigquery.rs` or `databricks.rs`.
- `dbt-adapter-core`: A very lightweight crate that does not implement much behavior. It's only purpose is to break dependencies and expose widely used types like `AdapterType` to the entire codebase without requiring a hard dependency on `dbt-adapter` from other crates. Avoid adding new things to this crate as well.
- `dbt-adapter-sql`: Contains basic functionality for statement splitting, quoting etc. Everything that is related to parsing SQL should go here in a generic method that takes `adapter_type` as a parameter in this crate.
- `dbt-sql-kewords`: Similar to `dbt-adapter-sql` in the regard that it deals with SQL dialects, but this one is for identifying keywords. Any new keyword detection logic goes here.
- `dbt-auth`: Responsible for validating `profiles.yml` and translating into ADBC connection options. It is the bridge between the dbt auth world to the ADBC auth world. This crate MUST NOT contain anything that does not relate to this specific thing.
- `dbt-adapter`: Where the real "meat" of the adapter logic lives. Next section talks about it.

## How `dbt-adapter` is structured

`dbt-adapter` has 3 layers: 

### `Adapter`

Exposes the `adapter` jinja interface, e.g `adapter.execute()`, and any adapter-owned jinja objects like `RelationConfig`, `Column` and `Relation`. The `Adapter` struct is responsible for "translating" untyped Jinja function calls into strongly-typed Rust calls.

### `AdapterImpl`

This is called by `Adapter`. Implements the strongly typed methods of the dbt adapter API. Due to historical reasons, the adapters API grew organically, and so sometimes there are different ways to do the same thing. Ideally `AdapterImpl` is the bridge between our messy historical API and shared implementations between platforms. However, sometimes it is just not feasible to share the same code across all warehouses, and so we need to `match` on `adapter_type` to figure out what to do depending on the target platform.

The code in `AdapterImpl` is what effectively makes all the macro calls and that tries to mirror v1 dbt adapters.

### `AdapterEngine`

This is a higher-level abstraction over `dbt-adbc`, and it makes database calls with specific options set depending on user configuration. It is at this layer where we can inject things like query comments, tags and other connection/statement-specific properties.


# Hard rules

## Unsupported adapter gate

experimental adapters (postgres, trino, etc.) are blocked at profile-load time.
Set `DBT_ALLOW_EXPERIMENTAL_ADAPTERS=yes` to bypass when running or updating goldies locally.
CI sets this automatically.

## No new deps

NEVER EVER add a new dependency to any crate without permission, even if it is a workspace dependency or a sibling crate.

## No platform-specific types

Bad: `BigqueryFoo`, `SnowflakeBar`
Good: `GenericFoo` with `match adapter_type { ... }`

- Similar logic already exists elsewhere? Add `AdapterType` check, rename to `GenericXYZ`.
- Net-new? Add `AdapterType`, match on it. OK to add `unimplemented!()` for platforms not worked on.
- See `src/column/types.rs` and `src/relation/relation_impl.rs` for shared-type example.

## Jinja to Rust type conversion: two-layer rule

- `adapter.rs`, `relation_object.rs`: convert `minijinja::Value` into Rust types
- `adapter_impl.rs`, `relation_impl.rs`: uses typed Rust only, no `Value`/`ValueMap`

## Jinja objects need tests

If you `impl Object` or `DynJinjaObject`, add `jinja_assert()` unit tests. See `src/relation/config_v2.rs` for examples.

## Annotate v1 conformance references in `adapter_impl.rs`

To make the transition from v1 to v2 smoother, we want to make v2 adapters behave like v1 to the extent possible. To document behavior, we annotate the code in `adapter_impl.rs` with links to the v1 implementation reference. When annotating `adapter_impl.rs` with upstream dbt-adapters Python reference links, use `/adapters-annotate-references` skill

## Changing macros

DO NOT touch any macros in `crates/dbt-loader/src/dbt_macro_assets` unless explicitly allowed by the user.

If you do change a macro, add tests to any changes you make using the existing test suite. See `crates/dbt-loader/tests/` for examples.

## Review your changes

Use the `/adapters-critic` skill to review your changes before asking for user review, and iterate until the skill says your changes are good enough.


