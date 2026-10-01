//! This module contains the resources that are encountered while rendering sql and macros.

use std::fmt::Debug;

use dbt_schemas::schemas::project::ResolvableConfig;
use dbt_schemas::schemas::serde::NodeVersion;

use dbt_frontend_common::error::CodeLocation;
use minijinja::{ArgSpec, machinery::Span};

/// Resources that are encountered while rendering sql and macros
#[derive(Debug, Clone, PartialEq)]
pub enum SqlResource<T: ResolvableConfig<T>> {
    /// A source call (e.g. `{{ source('a', 'b') }}`)
    Source((String, String, CodeLocation)),
    /// A source discovered by static AST analysis of a dead Jinja branch.
    ///
    /// Semantically the same as `Source` but must NOT become a runtime
    /// `depends_on` entry — the branch never executed, so the source is
    /// not an actual dependency of this model.  Consumers that only care
    /// about schema fetching or lineage should treat it the same as `Source`.
    StaticSource((String, String, CodeLocation)),
    /// A ref call (e.g. `{{ ref('a', 'b') }}`)
    Ref((String, Option<String>, Option<NodeVersion>, CodeLocation)),
    /// A this call (e.g. `{{ this }}`)
    This,
    /// A function call (e.g. `{{ function('a', 'b') }}`)
    Function((String, Option<String>, CodeLocation)),
    /// A metric call (e.g. `{{ metric('a', 'b') }}`)
    Metric((String, Option<String>)),
    /// An explicit `{{ config(...) }}` call encountered in the SQL.
    ConfigCall(Box<T>),
    /// A test definition (e.g. `{% test foo() %}`)
    Test(String, Span, Vec<ArgSpec>, Span), // name, span, args, macro_name_span
    /// A macro definition (e.g. `{% macro my_macro(a, b) %}`)
    Macro(String, Span, Option<String>, Vec<ArgSpec>, Span), // name, span, funcsign, args, macro_name_span
    /// A docs definition (e.g. `{% docs my_docs %}`)
    Doc(String, Span),
    /// A snapshot definition (e.g. `{% snapshot my_snapshot %}`)
    Snapshot(String, Span, Span), // name, span, macro_name_span
    /// A materialization macro definition (e.g. `{% materialization my_materialization, adapter='snowflake' %}`)
    Materialization(String, String, Option<Vec<String>>, Span, Span), // name, adapter, supported_languages, span, macro_name_span
}

impl<T: ResolvableConfig<T>> std::fmt::Display for SqlResource<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SqlResource::Source((a, b, location)) => {
                write!(f, "Source({a}, {b}, {location:?})")
            }
            SqlResource::StaticSource((a, b, location)) => {
                write!(f, "StaticSource({a}, {b}, {location:?})")
            }
            SqlResource::Ref((a, b, c, location)) => {
                write!(f, "Ref({a}, {b:?}, {c:?}, {location:?})")
            }
            SqlResource::This => write!(f, "This"),
            SqlResource::Function((a, b, location)) => {
                write!(f, "Function({a}, {b:?}, {location:?})")
            }
            SqlResource::Metric((a, b)) => {
                write!(f, "Metric({a}, {b:?})")
            }
            SqlResource::ConfigCall(config) => write!(f, "ConfigCall({config:?})"),
            SqlResource::Test(name, span, _, _) => write!(f, "Test({name} {span:#?})"),
            SqlResource::Macro(name, span, _, _, _) => write!(f, "Macro({name} {span:#?})"),
            SqlResource::Doc(name, span) => write!(f, "Docs({name} {span:#?})"),
            SqlResource::Materialization(name, adapter, _, span, _) => {
                write!(f, "Materialization({name} {adapter} {span:#?})")
            }
            SqlResource::Snapshot(name, span, _) => {
                write!(f, "Snapshot({name} {span:#?})")
            }
        }
    }
}
