use std::collections::BTreeMap;

use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::project::QueryComment;

pub fn resolve_query_comment(
    query_comment: &QueryComment,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, minijinja::Value>,
) -> FsResult<()> {
    match query_comment {
        QueryComment::String(comment) => {
            validate_comment_string(comment)?;
            jinja_env.render_str(comment, base_ctx, &[])?;
        }
        QueryComment::Object(value) => {
            let config = value.as_mapping().ok_or_else(|| {
                fs_err!(ErrorCode::InvalidConfig, "Query comment must be a string")
            })?;

            if let Some(value) = config.get("comment") {
                let comment = value.as_str().ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Unrecognized value for query comment. Expected a string or a mapping.",
                    )
                })?;
                validate_comment_string(&comment.to_string())?;
                jinja_env.render_str(comment, base_ctx, &[])?;
            }

            if let Some(value) = config.get("append") {
                let _ = value.as_bool().ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Invalid query comment: append must be a bool"
                    )
                })?;
            }
            if let Some(value) = config.get("job-label") {
                let _ = value.as_bool().ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Invalid query comment: job-label must be a bool"
                    )
                })?;
            }
        }
    };
    Ok(())
}

fn validate_comment_string(comment: &String) -> FsResult<()> {
    // Deviation from Core: Disallow '/*' and '*/' to be able
    // to strip comments
    if comment.contains("*/") || comment.contains("/*") {
        return err!(
            ErrorCode::InvalidConfig,
            "Query comment cannot contain illegal values \"/*\" or \"*/\": {}",
            comment
        );
    }
    Ok(())
}
