//! Public classifier data shapes shared across crates.
//!
//! These are the lean, compiler-free shapes used by the OSS surface
//! (`dbt-jinja-utils`, `dbt-index-core`) and by the proprietary propagation
//! engine (`dbt-classification`). Anything that requires the SQL/LogicalPlan
//! compiler or warehouse-specific logic lives in the `dbt-classification`
//! engine crate.
//!
//! # New-architecture note
//!
//! In the old (pre-OSS/proprietary-split) layout this crate also hosted a
//! runtime callback "gate slot" (`set_classification_gate` /
//! `classification_possible`) so that the then-proprietary `dbt-main` could
//! consult the gate without depending on the engine crate. That inversion is
//! **obsolete** in the new architecture: the engine is linked only into the
//! proprietary binary (via the proprietary task crate + the `FeatureStack`),
//! so separation is structural and no gate slot is needed here. The gate, if
//! still desired as a runtime activation flag, lives entirely inside the
//! proprietary `dbt-classification` crate.

pub mod macros;
pub mod taxonomy;
pub mod types;

pub use macros::{PACKAGE_NAME as PROPAGATION_MACRO_PACKAGE, propagation_macro_templates};
pub use taxonomy::{ClassifierDef, LabelDef, builtin_classifiers};
pub use types::{Label, LabelSet};
