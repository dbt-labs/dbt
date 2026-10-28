# Config Field Inheritance Guide

This guide explains how hierarchical config inheritance works in dbt configurations,
including the `Omissible<T>` type, the `DefaultTo` trait, and the proc-macro derive.

## Overview

Config inheritance flows project-level → package-level → model-level. Two types of
fields participate:

- **`Option<T>`**: Field was not specified (`None`) vs specified (`Some`). Child inherits
  parent value when `None`.
- **`Omissible<T>`**: Extends `Option<T>` with a third state. Distinguishes between
  "field not present in YAML" (`Omitted`) and "field explicitly set to null"
  (`Present(None)`). Explicit null overrides the parent; absence does not.

## The `DefaultTo` Trait

Defined in `config_merge.rs`:

```rust
pub trait DefaultTo {
    fn inherit_from(&mut self, parent: &Self);
}
```

Each field type implements `inherit_from` with its own merge semantics:

| Type | Semantics |
|------|-----------|
| `Option<T>` where `T: ReplaceIfNone` | Replace child with parent when child is `None` |
| `Omissible<T>` | Replace child with parent when child is `Omitted` |
| `Tags` | Union-merge (dedup + sort) of parent and child tag lists |
| `Packages` | Append parent values before child values, no dedup |
| `Option<DbtQuoting>` | Deep-merge: each sub-field inherits from parent when unset |
| `Option<DocsConfig>` | Deep-merge: `node_color` falls back to parent |
| `Option<IndexMap<String, YmlValue>>` | Union-merge: child keys win, missing keys fall back to parent |
| `Option<BTreeMap<Spanned<String>, String>>` | Union-merge: parent keys fill missing child keys |
| `OmissibleGrantConfig` | DictKeyAppend: `+key` extends, plain key clobbers |
| `Verbatim<Option<Hooks>>` | Append: parent hooks prepended to child hooks |
| `Verbatim<T: DefaultTo>` | Delegates to inner type's `inherit_from` |
| `IndexesConfig` / `PrimaryKeyConfig` | Replace-if-none (opaque newtypes with `.is_none()`) |

### `ReplaceIfNone` Marker

```rust
pub trait ReplaceIfNone: Clone {}

impl<T: ReplaceIfNone> DefaultTo for Option<T> {
    fn inherit_from(&mut self, parent: &Self) {
        if self.is_none() {
            *self = parent.clone();
        }
    }
}
```

Implement `ReplaceIfNone` for any type whose `Option<T>` field should simply copy the
parent value when the child is `None`. Types with custom merge logic (quoting, docs,
meta, grants) have their own `impl DefaultTo for Option<T>` and must NOT implement
this marker.

## Tags and Packages Newtypes

`StringOrArrayOfStrings` has two different merge semantics depending on the field:

- **`Tags`** (wraps `Option<StringOrArrayOfStrings>`): parent-first append merge, order
  preserved, no dedup/sort. Used for the `tags` field.
- **`Classifiers`** (wraps `Option<StringOrArrayOfStrings>`): union-merge with dedup+sort.
  Used for the `classifiers` field.
- **`Packages`** (wraps `Option<StringOrArrayOfStrings>`): append parent before child,
  no dedup. Used for `packages` field.
- **Bare `Option<StringOrArrayOfStrings>`**: replace-if-none (via `ReplaceIfNone`
  blanket). Used for fields like `check_cols`, `unique_key`.

Both newtypes expose `inner()` and `into_inner()` accessors to get the wrapped
`Option<StringOrArrayOfStrings>`.

## Adding a New Config Field

### Regular field (replace-if-none)

If `T` already implements `ReplaceIfNone`, just add the field:

```rust
pub my_field: Option<T>,
```

No other changes needed — `#[derive(DefaultTo)]` picks it up automatically.

If `T` is a new type, add `impl ReplaceIfNone for T {}` to `config_merge.rs`.

### Field with custom merge semantics

Implement `DefaultTo` directly for the field type in `config_merge.rs`:

```rust
impl DefaultTo for Option<MyType> {
    fn inherit_from(&mut self, parent: &Self) {
        // custom merge logic
    }
}
```

Then add the field to the struct — `#[derive(DefaultTo)]` picks it up.

### Omissible field (explicit-null-wins)

Wrap the field in `Omissible<T>`. The blanket impl handles it:

```rust
pub schema: Omissible<Option<String>>,
```

`Omissible<T>` inherits from parent only when the child is `Omitted`. `Present(None)`
(explicit YAML null) blocks inheritance and propagates downstream.

## `#[derive(DefaultTo)]`

The proc-macro in `dbt-proc-macros` generates a `default_to_fields` method that calls
`DefaultTo::inherit_from` for every named field:

```rust
#[derive(DefaultTo)]
pub struct ModelConfig {
    pub schema: Omissible<Option<String>>,
    pub tags: Tags,
    pub enabled: Option<bool>,
    // ...
}
```

Generated code (simplified):

```rust
fn default_to_fields(&mut self, parent: &Self) {
    DefaultTo::inherit_from(&mut self.schema, &parent.schema);
    DefaultTo::inherit_from(&mut self.tags, &parent.tags);
    DefaultTo::inherit_from(&mut self.enabled, &parent.enabled);
    // ...
}
```

Skip a field with `#[default_to(skip)]` — use this for fields that should never
inherit from parent (e.g., internal metadata).

## `ResolvableConfig` / `DefaultConfig` Trait

Config structs implement `ResolvableConfig<ProjectConfig>`, which requires:

```rust
fn default_to(&mut self, parent: &ProjectConfig);
```

For most structs this is now just:

```rust
fn default_to(&mut self, parent: &ProjectConfig) {
    self.default_to_fields(parent);
}
```

The `default_to_fields` method is generated by `#[derive(DefaultTo)]`.

## Behavior Reference

### `Omissible<T>` states

| Child state | Parent state | Result |
|-------------|--------------|--------|
| `Omitted` | any | child takes parent's value |
| `Present(None)` | any | child keeps `Present(None)` (explicit null wins) |
| `Present(Some(v))` | any | child keeps `Present(Some(v))` |

### `Option<T: ReplaceIfNone>` states

| Child | Parent | Result |
|-------|--------|--------|
| `None` | any | child takes parent's value |
| `Some(v)` | any | child keeps `Some(v)` |

## Testing

See `config_merge_tests.rs` for tests covering `Omissible` inheritance patterns,
including explicit null overrides, chain inheritance, and serialization.

## Files

| File | Purpose |
|------|---------|
| `config_merge.rs` | `DefaultTo`, `ReplaceIfNone`, all per-type impls, `Tags`, `Packages` |
| `config_merge_tests.rs` | Tests for `Omissible` inheritance logic |
| `dbt-proc-macros/src/lib.rs` | `#[derive(DefaultTo)]` proc-macro |
