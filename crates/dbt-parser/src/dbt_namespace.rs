use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use dbt_adapter::Adapter;
use dbt_adapter::cast_util::downcast_value_to_dyn_base_relation;
use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::Object;
use minijinja::{
    Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State, Value as MinijinjaValue,
};

/// A namespace object that intercepts specific dbt macro calls to track them in the adapter
/// (in parse-phase mode) before delegating to the original templates.
#[derive(Debug, Clone)]
pub struct DbtNamespace {
    parse_adapter: Arc<Adapter>,
}

impl DbtNamespace {
    /// Creates a new DbtNamespace that tracks calls in the adapter
    pub fn new(parse_adapter: Arc<Adapter>) -> Self {
        debug_assert!(
            parse_adapter.is_parse(),
            "DbtNamespace should be created with a Adapter in parse mode",
        );
        Self { parse_adapter }
    }
}

impl fmt::Display for DbtNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DbtNamespace")
    }
}

impl Object for DbtNamespace {
    fn call_method(
        self: &Arc<Self>,
        state: &State,
        name: &str,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        // Intercept specific method calls to track them
        match name {
            "get_columns_in_relation" => {
                // Track the call in the parse adapter
                // Extract relation from args
                let relation = args.first().ok_or_else(|| {
                    MinijinjaError::new(
                        MinijinjaErrorKind::InvalidOperation,
                        "get_columns_in_relation requires one argument",
                    )
                })?;
                let relation = downcast_value_to_dyn_base_relation(relation)?;
                self.parse_adapter
                    .parse_adapter_state()
                    .expect("adapter should be configured for the parse phase")
                    .record_get_columns_in_relation_call(state, relation.as_ref())?;

                // Delegate to the original dbt.get_columns_in_relation template
                let template_name = "dbt.get_columns_in_relation";
                if let Ok(template) = state.env().get_template(template_name) {
                    let base_ctx = state.get_base_context();
                    let template_state = template.eval_to_state(base_ctx, listeners)?;
                    let func = template_state
                        .lookup("get_columns_in_relation", listeners)
                        .ok_or_else(|| {
                            MinijinjaError::new(
                                MinijinjaErrorKind::InvalidOperation,
                                "get_columns_in_relation macro not found",
                            )
                        })?;
                    func.call(&template_state, args, listeners)
                } else {
                    Err(MinijinjaError::new(
                        MinijinjaErrorKind::TemplateNotFound,
                        format!("Template {template_name} not found"),
                    ))
                }
            }
            "get_relation" => {
                // Track the call in the parse adapter if in execute mode
                let iter = ArgsIter::new(name, &["database", "schema", "identifier"], args);
                let database = iter.next_arg::<&str>()?;
                let schema = iter.next_arg::<&str>()?;
                let identifier = iter.next_arg::<&str>()?;
                // Consume optional needs_information (dbt-databricks v11.4+); we don't use it for
                // parse-phase tracking; the adapter ignores it when relations cache isn't implemented.
                let _ = iter.next_kwarg::<Option<bool>>("needs_information")?;
                iter.finish()?;
                // NOTE(felipecrv): this doens't have to be called directly when we move to Adapter
                self.parse_adapter
                    .parse_adapter_state()
                    .expect("adapter should be configured for the parse phase")
                    .record_get_relation_call(state, database, schema, identifier)?;

                // Delegate to the original dbt.get_relation template
                let template_name = "dbt.get_relation";
                if let Ok(template) = state.env().get_template(template_name) {
                    let base_ctx = state.get_base_context();
                    let template_state = template.eval_to_state(base_ctx, listeners)?;
                    let func = template_state
                        .lookup("get_relation", listeners)
                        .ok_or_else(|| {
                            MinijinjaError::new(
                                MinijinjaErrorKind::InvalidOperation,
                                "get_relation macro not found",
                            )
                        })?;
                    func.call(&template_state, args, listeners)
                } else {
                    Err(MinijinjaError::new(
                        MinijinjaErrorKind::TemplateNotFound,
                        format!("Template {template_name} not found"),
                    ))
                }
            }
            _ => {
                // For all other methods, delegate to the original dbt template
                let template_name = format!("dbt.{name}");
                if let Ok(template) = state.env().get_template(&template_name) {
                    let base_ctx = state.get_base_context();
                    let template_state = template.eval_to_state(base_ctx, listeners)?;
                    let func = template_state.lookup(name, listeners).ok_or_else(|| {
                        MinijinjaError::new(
                            MinijinjaErrorKind::InvalidOperation,
                            format!("{name} macro not found"),
                        )
                    })?;
                    func.call(&template_state, args, listeners)
                } else {
                    Err(MinijinjaError::new(
                        MinijinjaErrorKind::TemplateNotFound,
                        format!("Template {template_name} not found"),
                    ))
                }
            }
        }
    }
}
