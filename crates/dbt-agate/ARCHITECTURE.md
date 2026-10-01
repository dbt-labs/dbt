# dbt-agate

This crate provides a Rust reimplementation of the core surface area of Python [agate](https://agate.readthedocs.io/en/latest/) behaviors for Fusion's MiniJinja layer. Agate provides a typed, immutable row-oriented data abstraction with transformation operations. Within dbt models or operations, `AgateTable` instances are first-class values in the Jinja execution context and may be constructed and referenced inside template statements (`{{ }}` and `{% %}`). Within the Fusion adapter layer, `AgateTable` instances provide key functionality to various adapter methods. Faithful but efficient semantics are critical to cross-platform conformance.

Fusion is Arrow-native and operates on columnar `RecordBatch` data. This crate provides the agate-compatible surface required by dbt (e.g. dynamic runtime result sets from `run_query`, table transformations such as `group_by()`) while preserving Arrow's columnar `RecordBatch` representation. Fields are converted into Jinja values only when requested.


## 1. Constraints

Our agate API is "columnar-first, row-when necessary".
- Arrow columnar representations are the native execution format and must not incur translation when received from storage engines
- conversion into per-row Jinja values must be lazy (deferred until the Jinja boundary)
- The system should be zero-copy wherever possible.
- Only the dbt Core-required agate surface is implemented; additional capabilities are added incrementally as required.
- Make Jinja template operations feel native and stable without introducing runtime costs that would undermine Arrow.


## 2. The Shape of Data

1. **Arrow `RecordBatch` input** arrives from upstream data sources
2. **`FlatRecordBatch`** flattens nested Arrow columns (structs, lists, dictionary) into a flat schema and translates Arrow data types into Agate data types (but doesn't convert the data from the nested fields themselves)
3. **`TableRepr`** stores the flat batch and an optional lazy `VecOfRows` representation
4. **`AgateTable`** stores the table representation and implements `minijinja::Object`
    1. **Lazy row materialization** happens only when necessary (e.g. Jinja iteration, row-level operations)
    2. **`minijinja::Value`** objects are produced per-cell via `ArrayConverter`s to drive templating and Python-like behaviors

This pipeline intentionally separates structural normalization from value conversion. Because Arrow data can have nested data types, we replicate Agate semantics: flatten the schema (i.e. nested into flat columns) into Agate types.

Scalar value conversion is a runtime operation: Arrow cells are transformed into Jinja values only at the point of use. Structural normalization occurs eagerly at schema level, but per-cell conversion is deferred. This allows Arrow semantics to power filtering, grouping, and projection.

The dataflow is "dual-format" by design. Arrow remains the authoritative representation, and the row-wise Jinja representation is a derived, lazily materialized view. This enables:
- using rows directly for a feature when Arrow dev hours are too expensive to frontload
- preserving efficiency in read-heavy scenarios
- overall memory use low and performance high for most dbt workloads
- supporting compatibility behaviors expected from Agate in Jinja

## 3. Internal Representation Mapping
- `AgateTable` is the public façade and wraps `TableRepr`
- `TableRepr` owns:
  - `FlatRecordBatch` owns:
    - The original `RecordBatch` via `self.original()`
    - The flattened (but not type-converted) record batch via `self.inner()`
  - `OnceLock<Result<Arc<VecOfRows>, Arc<ArrowError>>>` for lazy whole-table materialization, rarely needed, functions lazily convert individual values based on demand
  - Optional row name storage
- `Column` and `Row` are lightweight indexed views over `TableRepr`
- `Columns` and `Rows` implement `MappedSequence`, which provides Python-sequence-like lazy access
- `TableSet` aggregates tables sharing a schema and tracks grouping keys.

All public objects are views over the same underlying data. `TableRepr` is the authoritative representation of table structure: schema, row count, and access semantics. `FlatRecordBatch` owns the normalized Arrow arrays and is the authoritative source of data values.

This separation isolates concerns: data correctness is a property of `FlatRecordBatch`, while API and behavioral correctness is largely a property of `TableRepr` and the `MappedSequence` protocol.

`TableRepr` does not automatically materialize rows. It only builds them when a consumer requests row-wise iteration or tuple-style access. That boundary is also the line where Arrow types become Jinja values via converters. The architecture treats this as a one-way on-demand bridge: Arrow -> Jinja.

When Arrow can satisfy an operation, it is used directly. Only when a feature requires row-wise objects (e.g. Jinja iteration or Python-like tuple behavior) do we force conversion to `VecOfRows`.


## 4. Jinja/Python Compatibility Layer
The Jinja-facing surface is built around three cooperating abstractions:

- `MappedSequence`: defines common behavior for row-, column-, and table-like objects like the class of the same name in the Python library. It provides Python-sequence semantics (indexing, iteration, length) while remaining lazily backed by Arrow.
- `Tuple` / `TupleRepr`: implements virtualized tuple-like behavior (`count`, `index`, iteration) to enable lazy materialization of row data.
- `minijinja::Object`: integrates tables, rows, and columns into the Jinja runtime, enabling attribute access, method dispatch, and template-level function semantics.

This layer does not own data. It adapts Arrow-backed representations to behave like Python agate objects within the Jinja execution model. Protocol emulation is the aim.

## 5. Converters and Arrow Semantics
Converters are a critical part of this architecture. Anywhere flattened record batches are required, you can think of converters as the "how" for rendering Arrow's precisely optimized columnar types into the scalar Jinja values that users actually see. If this layer is even slightly wrong, downstream behavior becomes inconsistent: flattening results no longer match row values, template logic diverges from dbt Core, and cross-platform conformance breaks. In other words, this is where we decompose Arrow types into flattened records used across Fusion.

We are deliberately strict about Arrow semantics. The Arrow spec includes nullability, numeric width, decimal precision/scale, time units, and time zones. We honor these. We also honor nested types (e.g. lists, maps) by recursively converting child values while preserving offsets and nulls. This strictness is essential due to the fact that `FlatRecordBatch` normalizes the schema into a stable, Agate-like shape.

Key responsibilities the converters must uphold:
- Zero-copy or minimal-copy behavior by reusing Arrow buffers (e.g. cloning array views and buffers rather than materializing new data)
- Null correctness by respecting the Arrow null buffer independently of the value buffer (nulls must become `None` in Jinja, not default values
- Numeric fidelity by using the Arrow physical representation directly and careful widening or truncation operations when required
- Decimal correctness by honoring precision/scale, and by choosing integer representations only when Arrow semantics allow it (scale == 0, precision fits)
- Converting via Arrow-safe ranges and preserving timezone information when present
- Recursively converting list and map children while honoring offsets and per-row nulls
- Defined fallback behavior for unsupported Arrow types

This diligence is what makes Arrow remain the authoritative source of truth. When converters are correct, we can defer row materialization without fear of semantic drift. When they are incorrect, everything on the Jinja side is suspect. This layer must therefore be exact, stable, and spec-aligned to keep data coherent.
