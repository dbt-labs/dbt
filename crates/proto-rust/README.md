# Protocol Buffers Definitions

## Synchronizing Protocol Buffers Definitions

Clone the [dbt-labs/proto](https://github.com/dbt-labs/proto) repository as
sibling of this repository and run the `./scripts/sync_protos.sh` script to
synchronize the Protocol Buffers definitions with the latest version.

## Generating Rust Code

Check `protogen.toml` to make sure the `.proto` files you want to generate
are included via glob patterns (using the `glob` crate). Patterns are matched
against the path relative to the include roots (`fs/sa/crates/proto-rust/include`
for public and `crates/proto-rust-private/include` for private). Then run:

```shell
cargo xtask protogen
```

By default, this uses `protogen.toml` at the repo root. To try a non-standard config file (useful while testing changes), pass a full path:

```shell
cargo xtask protogen --config /absolute/path/to/protogen.toml
```

The explicit glob lists in `protogen.toml` are used to ensure that we only generate
code for the `.proto` files that we actually use in this repository. This
prevents unnecessary code generation and very slow Rust builds.

### Adding Type Attributes

You can add extra Rust attributes and derives to generated types via `protogen.toml`. Note that prost already adds some derives by default (e.g., `Clone`, `PartialEq`, `Message`). See the [prost documentation](https://docs.rs/prost/) for details on what is added by default.

No exrta attributes are applied by default; configure what you need explicitly:

```
[type_attributes]
".v1.public.events.fusion" = [
  "#[serde(rename_all = \"snake_case\")]",
]
".v1.events.vortex" = [
  "#[::serde_with::skip_serializing_none]",
  "#[derive(::serde::Serialize, ::serde::Deserialize)]",
]
```

- Key is a prost type path (e.g., a package like `.v1.public.events.fusion` or a fully-qualified message path).
- Values are attribute strings that are appended to that type (in order from outer to inner).
- Make sure to use absolute paths for attributes (e.g., `::serde::Serialize` instead of just `Serialize`) to avoid issues with imports.
- Make sure cargo dependencies are set up correctly for any attributes you use (e.g., `serde` and `serde_with` in the example above).

Include the files with `git`. We check-in the generated code to avoid having to
regenerate it on every build. `.gitattributes` declares these files are
generated so Github will collapse them in the UI.

## Manually tweaking lib.rs

We don't rely on auto-generated `_includes.rs` or `mod.rs` files. Instead we
manually maintain `lib.rs` files to have more control over the module structure
and visibility of the generated code. After running the code generation, you may
need to manually tweak the `lib.rs` files in the `proto-rust` and
`proto-rust-private` crates.

This let's us, for example, re-export certain types from the public crate so
they can be used by the private crate.
