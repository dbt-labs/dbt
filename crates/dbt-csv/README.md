# dbt-csv

A CSV reader designed to be **100% conformant with Core's CSV parsing behavior**.

## Background

Core's CSV parsing leverages Python's `agate` library via `dbt-common/dbt_common/clients/agate_helper.py`. The parsing pipeline consists of three steps:

1. **Parse CSV** — Use Python's built-in `csv` module to read data as string rows
2. **Infer types** — Use agate's type testers to determine column types from string values
3. **Cast values** — Convert string rows to Python-typed values based on inferred types

This produces a CSV table of Python-typed values (integers, decimals, dates, booleans, etc.).

## How This Crate Works

This crate replicates Core's behavior in Rust, producing Arrow record batches instead of Python objects:

1. **Parse CSV** — Use Rust's `csv` crate to read data as string rows
2. **Infer types** — Use `type_tester.rs` to infer agate-compatible types (`AgateType`)
3. **Cast values** — Convert string rows to Arrow arrays based on inferred `AgateType`

The output is a stream of Arrow `RecordBatch` objects with types that match what Core would produce.

## Type Inference Rules

Following agate's type priority (highest to lowest):

| Priority | AgateType | Arrow Type | Example Values |
|----------|-----------|------------|----------------|
| 1 | Integer | Int64 | `42`, ` 123 ` (whitespace trimmed) |
| 2 | Number | Float64 | `3.14`, `50%`, `1.5e10` |
| 3 | Date | Date32 | `2024-01-15` |
| 4 | DateTime | Timestamp(ns) | `2024-01-15 10:30:00` |
| 5 | ISODateTime | Timestamp(ns) | `2024-01-15T10:30:00Z`, `20240115T103000Z` |
| 6 | Boolean | Boolean | `true`, `false` (case-insensitive) |
| 7 | Text | Utf8 | Everything else |

**Null values:** Empty strings (`""`) and `"null"` (case-insensitive) are treated as NULL.

## Force Text Columns

Agate allows users to override type inference for specific columns. Core uses this feature to force user-specified seed columns to be treated as strings, deferring type casting to the warehouse.

This crate provides a constrained API for the same purpose via `infer_agate_schema_with_text_columns`:

```rust
let text_columns = vec!["DATE_CREATED".to_string(), "ORIGINAL_NETWORK_NAME".to_string()];
let (schema, records_read, missing) = format
    .infer_agate_schema_with_text_columns(&mut reader, max_records, &text_columns)
    .unwrap();
```

**Matching behavior:**
- Column names are matched **case-insensitively** using `dbt-ident`
- This differs from Core, which does case-sensitive matching because it preserves the user's original casing from the YAML file
- In Fusion, `DbtSeed` normalizes `column_types` keys based on warehouse semantics during the resolve phase (e.g., Snowflake uppercases unquoted identifiers), so case-insensitive matching is needed to match normalized keys against original CSV headers
- Columns not found in CSV headers are returned in `missing` for warning/logging

## Usage

Enable with the environment variable:

```bash
fs seed
```

## Crate Structure

```
src/
├── lib.rs              # Public API exports
├── type_tester.rs      # Agate-compatible type inference (AgateType, AgateSchema)
└── reader/
    ├── mod.rs          # CSV reader, decoder, and string-to-Arrow parsing
    └── records.rs      # Low-level record decoder (forked from arrow-csv)
```

### Key Components

- **`AgateType`** — Enum representing agate's type system
- **`AgateSchema`** — Schema using `AgateType` instead of Arrow `DataType`
- **`Format`** — Configures CSV parsing options and performs type inference
- **`ReaderBuilder`** / **`Reader`** — Builds and iterates over `RecordBatch` results
- **`RecordDecoder`** — Low-level CSV field parsing (forked from arrow-csv because the original is not public)

## Origin

Forked from [arrow-csv](https://github.com/apache/arrow-rs/tree/main/arrow-csv) with modifications for agate conformance.
