use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::state::ResolverState;
use minijinja::Value as MinijinjaValue;

/// Helper function to render a single constraint ref field
fn render_constraint_ref_field(
    to_ref: &str,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<String> {
    if to_ref.contains("ref(") || to_ref.contains("source(") {
        let wrapped_ref = format!("{{{{{to_ref}}}}}");

        jinja_env
            .render_str(&wrapped_ref, base_context, &[])
            .map_err(|e| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Failed to render constraint 'to' field '{}': {}",
                    to_ref,
                    e
                )
            })
    } else {
        Ok(to_ref.to_string())
    }
}

/// Renders constraint refs in-place for all models in the ResolverState
/// This should be called after build_compiler_env but before compile/run phases
pub fn render_all_model_constraint_refs_in_place(
    resolver_state: &mut ResolverState,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<()> {
    for (_, model_arc) in resolver_state.nodes.models.iter_mut() {
        let model = Arc::make_mut(model_arc);

        // Render model-level constraint 'to' fields
        for constraint in model.__model_attr__.constraints.iter_mut() {
            if let Some(to_spanned) = constraint.to.take() {
                let rendered = render_constraint_ref_field(&to_spanned, jinja_env, base_context)?;
                constraint.to = Some(to_spanned.map(|_| rendered));
            }
        }

        // Render column-level constraint 'to' fields
        for column_arc in model.__base_attr__.columns.iter_mut() {
            let column = Arc::make_mut(column_arc);
            for constraint in column.constraints.iter_mut() {
                if let Some(to_spanned) = constraint.to.take() {
                    let rendered =
                        render_constraint_ref_field(&to_spanned, jinja_env, base_context)?;
                    constraint.to = Some(to_spanned.map(|_| rendered));
                }
            }
        }
    }
    Ok(())
}
