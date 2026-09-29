//! Loads and validates `<project_root>/catalogs.yml` into a global holder.

use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::warn_error_options::project_flags_get_value;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::{
    dbt_catalogs::DbtCatalogs, dbt_catalogs_v2::validate_catalogs_v2, validate_catalogs,
};
use dbt_yaml as yml;
use std::path::Path;
use std::sync::{Arc, RwLock};

const CATALOGS_V2_DISCUSSION_URL: &str = "https://github.com/dbt-labs/dbt-core/discussions/12723";

static CATALOGS: RwLock<Option<Arc<DbtCatalogs>>> = RwLock::new(None);
static USE_CATALOGS_V2: RwLock<bool> = RwLock::new(false);

pub fn fetch_catalogs() -> Option<Arc<DbtCatalogs>> {
    match CATALOGS.read() {
        Ok(g) => g.as_ref().cloned(),
        Err(p) => p.into_inner().as_ref().cloned(),
    }
}

pub fn fetch_use_catalogs_v2() -> bool {
    match USE_CATALOGS_V2.read() {
        Ok(g) => *g,
        Err(p) => *p.into_inner(),
    }
}

/// Record whether the `use_catalogs_v2` behavior flag is set. Must run even when
/// there is no catalogs.yml, so callers can distinguish "flag set but no catalogs
/// defined" from "flag not set" (otherwise both look the same and yield a
/// misleading "set the flag" error).
pub fn set_use_catalogs_v2_from_flags(project_flags: Option<&yml::Value>) {
    let enabled = project_flags
        .and_then(|f| project_flags_get_value(f, "use_catalogs_v2"))
        .and_then(yml::Value::as_bool)
        .unwrap_or(true);
    *match USE_CATALOGS_V2.write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    } = enabled;
}

pub fn load_catalogs(
    text_yml: yml::Value,
    path: &Path,
    project_flags: Option<&yml::Value>,
) -> FsResult<()> {
    let validated = do_load_catalogs(text_yml, path, project_flags)?;
    let mut write_guard = match CATALOGS.write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *write_guard = Some(Arc::new(validated));
    Ok(())
}

pub fn do_load_catalogs(
    text_yml: yml::Value,
    path: &Path,
    project_flags: Option<&yml::Value>,
) -> FsResult<DbtCatalogs> {
    let _guard = yml::with_filename(Some(path.to_path_buf()));

    let (repr, span) = match text_yml {
        yml::Value::Mapping(mapping, span) => (mapping, span),
        _ => {
            return Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                loc  => path.to_path_buf(),
                "Top-level of '{}' must be a YAML mapping", path.display()
            ));
        }
    };

    let catalogs = DbtCatalogs::new(repr, span);
    set_use_catalogs_v2_from_flags(project_flags);
    if fetch_use_catalogs_v2() {
        let view = catalogs.view_v2()?;
        validate_catalogs_v2(&view, path)?;
    } else {
        emit_warn_log_message(
            ErrorCode::DeprecatedOption,
            format!(
                "catalogs.yml is being validated against the deprecated v1 schema. To migrate, \
                 remove the `use_catalogs_v2: false` override (or set it to `true`) under \
                 `flags:` in dbt_project.yml. See {CATALOGS_V2_DISCUSSION_URL}"
            ),
        );
        let view = catalogs.view()?;
        validate_catalogs(&view, path)?;
    }
    Ok(catalogs)
}
