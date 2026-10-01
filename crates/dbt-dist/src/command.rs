//! Handler for the `dbt internal get-distribution-info` plumbing command.

use std::path::Path;

use dbt_common::tracing::dbt_emit::println;
use dbt_common::{ErrorCode, FsResult, fs_err};

use crate::DistInfoDiscovery;

/// Prints distribution info as JSON, per the `path`/`all` flags:
/// - `all`: info for every dbt found on `PATH`, as a JSON array.
/// - `path`: info for the dbt at that location, as a single JSON object.
/// - neither: info for the currently running process, as a single JSON object.
///
/// `command_name` is the CLI-brand name of the currently running binary
/// (e.g. `"dbt-core"` for OSS), used to resolve the current process's own
/// distribution when it's the target of the discovery.
pub fn execute_get_distribution_info(
    path: Option<&Path>,
    all: bool,
    command_name: &str,
) -> FsResult<()> {
    if all {
        let infos = DistInfoDiscovery::AllInPath.discover(command_name)?;
        println(serde_json::to_string_pretty(&infos)?);
        return Ok(());
    }

    let discovery = match path {
        Some(path) => DistInfoDiscovery::AtLocation(path),
        None => DistInfoDiscovery::Current,
    };
    let mut infos = discovery.discover(command_name)?;
    let info = infos.pop().ok_or_else(|| {
        fs_err!(
            ErrorCode::Unexpected,
            "distribution discovery returned no results"
        )
    })?;
    println(serde_json::to_string_pretty(&info)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_reports_current_process() {
        let result = execute_get_distribution_info(None, false, "dbt-core");
        assert!(result.is_ok());
    }

    #[test]
    fn missing_target_path_is_an_error() {
        let result = execute_get_distribution_info(
            Some(Path::new("/nonexistent/path/dbt")),
            false,
            "dbt-core",
        );
        assert!(result.is_err());
    }

    #[test]
    fn all_flag_reports_every_dbt_on_path() {
        let result = execute_get_distribution_info(None, true, "dbt-core");
        assert!(result.is_ok());
    }
}
