use std::sync::Arc;

use dbt_clap_core::CliParserFactory as _;
use dbt_features::cli::DefaultCliParserFactory;
use dbt_features::feature_stack::FeatureStackConfig;
use dbt_features::feature_stack_builder::FeatureStackBuilder;
use dbt_features::tracing::TracingFeature;
use dbt_test_utils::task::utils::exec_fs;
use dbt_test_utils::task::{
    CommandFn, ExecuteAndCompare, G_DBT_TEST_UTILS_FEATURE_STACK, TaskSeq, fs_cmd_vec,
};

fn make_fs_command_fn() -> Arc<CommandFn> {
    let feature_stack = G_DBT_TEST_UTILS_FEATURE_STACK.get_or_init(|| {
        Arc::new(|tracing_config| {
            let tracing = TracingFeature::default().with_config_provider(tracing_config);
            let feature_stack = FeatureStackBuilder::new(tracing).build();
            let config = FeatureStackConfig {
                send_anonymous_usage_stats: false,
            };
            feature_stack.configure(&config).into()
        })
    });

    let version = env!("CARGO_PKG_VERSION");
    let parser = DefaultCliParserFactory.create("dbt-core", version);
    Arc::new(
        move |cmd_vec, project_dir, target_dir, stdout, stderr, tracing_handle| {
            exec_fs(
                Arc::clone(feature_stack),
                &parser,
                cmd_vec,
                project_dir,
                target_dir,
                stdout,
                stderr,
                dbt_main::dbt_lib::execute_fs,
                dbt_main::from_lib,
                tracing_handle,
            )
        },
    )
}

fn make_fs_cmd_vec(command: impl AsRef<str>) -> Vec<String> {
    let mut command = fs_cmd_vec(command.as_ref());
    command.push("--no-send-anonymous-usage-stats".to_string());
    command.push("--no-version-check".to_string());
    command
}

pub trait TaskSeqExt {
    fn fs_sa(&mut self, command: impl AsRef<str>) -> &mut Self;
}

impl TaskSeqExt for TaskSeq {
    fn fs_sa(&mut self, command: impl AsRef<str>) -> &mut Self {
        self.task(Box::new(ExecuteAndCompare::new(
            self.name().to_owned(),
            make_fs_cmd_vec(command),
            make_fs_command_fn(),
            false,
        )))
    }
}
