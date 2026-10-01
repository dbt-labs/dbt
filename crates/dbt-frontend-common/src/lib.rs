pub mod column_resolution;
pub mod constraint;
pub mod dialect;
pub mod error;
pub mod expr;
pub mod ident;
pub mod span;
pub mod types;
pub mod utils;

// Don't re-export symbols for new sub-modules. See note below.
pub mod named_reference;
pub mod sources_extractor;

// TODO we should decide whether inner mods are pub, or we re-export individual items. Doing both creates unnecessary chaos where same names are imported from different paths.
pub use dialect::Dialect;
pub use ident::ColumnRef;
pub use ident::FullyQualifiedName;
pub use ident::IdentJoin;
pub use ident::Qualified;
pub use ident::QualifiedName;
