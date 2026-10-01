//! Helpers for accessing fully-qualified protobuf type name and type URL as `&'static str`.
//!
//! Prost's generated `prost::Name` trait exposes `full_name()` / `type_url()` as owned
//! `String` values. In some performance-sensitive or FFI integration points we need
//! zero-allocation `&'static str` constants instead. We generate impls of
//! [`StaticName`] for selected message types during the proto code generation
//! (see `xtask protogen`), extracting the literals that prost already embeds.
//!
//! The generated code looks like:
//!
//! ```ignore
//! impl crate::StaticName for MyMessage {
//!     const FULL_NAME: &'static str = "package.path.MyMessage";
//!     const TYPE_URL: &'static str = "/package.path.MyMessage";
//! }
//! impl ::prost::Name for MyMessage { /* ... */ }
//! ```
//!
//! This trait is intentionally minimal: just two associated constants. It should
//! only ever be implemented by generated code to ensure the values are guaranteed
//! correct and synchronized with the protobuf schema.

/// Provides compile-time access to a protobuf message's fully-qualified name and
/// type URL (`/fully.qualified.Name`) as `&'static str` constants.
///
/// Implementations are generated automatically; do not implement manually.
pub trait StaticName {
    /// Fully-qualified protobuf message name, e.g. `foo.bar.MyMessage`.
    const FULL_NAME: &'static str;

    /// Type URL form, i.e. `/{FULL_NAME}` used with `google.protobuf.Any` and
    /// other APIs that expect a leading `/`.
    const TYPE_URL: &'static str;
}
