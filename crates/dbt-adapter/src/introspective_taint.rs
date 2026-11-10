//! Re-export of minijinja's generic taint-propagation wrapper.
//!
//! `IntrospectiveValue` used to live here, but it has zero dependency on
//! anything `dbt-adapter`-specific (see its doc comment in minijinja), and
//! minijinja's own VM (`Macro::call`) now needs to manufacture tainted
//! values too (see the "any taint touched during a macro call taints its
//! return value" rule), so the single implementation now lives in
//! minijinja itself. This re-export keeps existing call sites in this crate
//! (`crate::introspective_taint::IntrospectiveValue::wrap(...)`) unchanged.
pub use minijinja::value::introspective::IntrospectiveValue;
