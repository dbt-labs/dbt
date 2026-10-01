//! The `dbt-core` distribution's Python extension module: the CLI surface and
//! runtime configuration over the shared glue in `dbt-python-core`.

use dbt_clap_core::{Cli, CliParser, CliParserFactory as _};
use dbt_common::io_args::SystemArgs;
use dbt_features::cli::DefaultCliParserFactory;
use dbt_features::feature_stack::{FeatureStack, FeatureStackConfig};
use dbt_features::feature_stack_builder::FeatureStackBuilder;
use dbt_features::tracing::TracingFeature;
use dbt_python_core::{Distribution, register};
use pyo3::prelude::*;
use std::sync::Arc;

/// This distribution's CLI surface.
fn dbt_core_cli_parser() -> CliParser {
    DefaultCliParserFactory.create("dbt-oss", env!("CARGO_PKG_VERSION"))
}

/// This distribution's runtime configuration. Takes the invocation's tracing
/// setup, which is installed per invocation rather than once per process.
fn dbt_core_feature_stack(
    tracing: TracingFeature,
    _cli: &Cli,
    arg: &SystemArgs,
) -> Arc<FeatureStack> {
    let feature_stack = FeatureStackBuilder::new(tracing).build();
    let config = FeatureStackConfig {
        send_anonymous_usage_stats: arg.io.send_anonymous_usage_stats,
    };
    feature_stack.configure(&config).into()
}

#[pymodule]
#[pyo3(name = "_core")]
fn dbt_core_pyo3(m: &Bound<'_, PyModule>) -> PyResult<()> {
    register(
        m,
        Distribution {
            cli_parser: dbt_core_cli_parser,
            feature_stack: dbt_core_feature_stack,
            // No command here needs its own sink stack.
            cli_tracing: None,
        },
    )
}
