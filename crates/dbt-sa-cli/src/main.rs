use dbt_clap_core::{CliParserFactory as _, from_main};
use dbt_common::tracing::FsTraceConfigBuilder;
use dbt_features::cli::DefaultCliParserFactory;
use dbt_features::feature_stack::{FeatureStack, FeatureStackConfig};
use dbt_features::tracing::TracingFeature;
use dbt_main::print_trimmed_error;

use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let version = env!("CARGO_PKG_VERSION");
    let cli_parser = DefaultCliParserFactory.create("dbt-oss", version);
    let cli = dbt_main::prepare_cli_or_exit(&cli_parser);

    let mut arg = from_main(&cli);

    let trace_config =
        FsTraceConfigBuilder::from_io_args("dbt", cli_parser.command_name(), &arg.io)
            .with_command(arg.command)
            .with_project_dir(cli.project_dir().as_ref())
            .with_target_path(cli.target_path().as_ref())
            .with_query_log_enabled(true) // Always enable query log for now
            .with_warn_error_options(cli.common_args().get_cli_warn_error_options())
            .with_skip_fusion_only_upgrades(cli.common_args().skip_fusion_only_upgrades())
            .with_quiet(cli.common_args().get_quiet())
            .build();
    let tracing_config_provider = trace_config.create_config_provider();
    let telemetry_handle = match trace_config.init(Arc::clone(&tracing_config_provider)) {
        Ok(handle) => handle,
        Err(e) => {
            let msg = e.to_string();
            print_trimmed_error(msg);
            std::process::exit(1);
        }
    };

    let tracing = TracingFeature::default()
        .with_config_provider(tracing_config_provider)
        .with_shutdown_handle(telemetry_handle);

    if let Some(resolved_file_log_path) = tracing.config_provider.get_file_log_path() {
        arg.io.log_path = Some(resolved_file_log_path);
    }

    let feature_stack: Arc<FeatureStack> = {
        let feature_stack =
            dbt_features::feature_stack_builder::FeatureStackBuilder::new(tracing).build();
        let config = FeatureStackConfig {
            send_anonymous_usage_stats: arg.io.send_anonymous_usage_stats,
        };
        feature_stack.configure(&config).into()
    };

    dbt_main::run_cli(cli, arg, feature_stack)
}
