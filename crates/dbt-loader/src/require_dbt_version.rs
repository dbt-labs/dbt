use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::{emit_debug_log_message, emit_warn_log_message};
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use fs_deps::require_dbt_version::{RequireDbtVersionCheck, check_require_dbt_version};

pub(crate) fn warn_require_dbt_version(spec: Option<&StringOrArrayOfStrings>, version_check: bool) {
    if !version_check {
        return;
    }
    let Some(spec) = spec else {
        return;
    };

    match check_require_dbt_version(&spec.to_strings(), running_dbt_version()) {
        RequireDbtVersionCheck::Compatible => {}
        RequireDbtVersionCheck::Incompatible { required, running } => {
            emit_warn_log_message(
                ErrorCode::RequireDbtVersionMismatch,
                format!(
                    "dbt_project.yml require-dbt-version {required} does not include the running dbt version {running}. dbt v2 will error on this in a future release. Update require-dbt-version, or pass --no-version-check to bypass this check."
                ),
            );
        }
        RequireDbtVersionCheck::Unparseable { text } => {
            emit_warn_log_message(
                ErrorCode::RequireDbtVersionUnparseable,
                format!(
                    "dbt_project.yml require-dbt-version could not be checked because `{text}` is not a supported version specifier. Use a comparator and a full version, such as `>=1.6.0`."
                ),
            );
        }
        RequireDbtVersionCheck::RunningUnparseable { running } => {
            emit_debug_log_message(format!(
                "running dbt version {running} could not be compared to require-dbt-version"
            ));
        }
    }
}

fn running_dbt_version() -> &'static str {
    option_env!("DBT_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
