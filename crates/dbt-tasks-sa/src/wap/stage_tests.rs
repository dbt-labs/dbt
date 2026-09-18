//! Execute the shipped materialization against a recording warehouse boundary.
//! These checks validate the submitted SQL, not Snowflake execution or atomicity.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use dbt_adapter::relation::{RelationObject, factory::create_static_relation};
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_jinja_utils::JinjaEnvBuilder;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Kwargs, Object, from_args};
use minijinja::{Error, ErrorKind, State, Value};
use parking_lot::Mutex;

use super::clone_tests::{fixture, runtime_context};

const TABLE_MACRO: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-snowflake/macros/materializations/table.sql"
);
const CREATE_MACROS: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-snowflake/macros/relations/table/create.sql"
);
const CREATE_DISPATCH_MACROS: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/relations/table/create.sql"
);
const STATEMENT_MACROS: &str =
    include_str!("../../../dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/etc/statement.sql");
const HOOK_MACROS: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/materializations/hooks.sql"
);
const GRANT_MACROS: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/adapters/apply_grants.sql"
);
const DOC_MACROS: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/adapters/persist_docs.sql"
);
const ADAPTER_MACROS: &str =
    include_str!("../../../dbt-loader/src/dbt_macro_assets/dbt-snowflake/macros/adapters.sql");

/// Resolve the limited dispatch surface used by this native table fixture to
/// the shipped macro definitions. Other adapter calls fail unless registered.
fn dispatch(args: &[Value]) -> Result<Value, Error> {
    let name = match args.first().and_then(Value::as_str) {
        Some("create_table_as") => "snowflake__create_table_as",
        Some("set_query_tag") => "snowflake__set_query_tag",
        Some("unset_query_tag") => "snowflake__unset_query_tag",
        Some("apply_grants") => "default__apply_grants",
        Some("persist_docs") => "default__persist_docs",
        unexpected => panic!("unexpected native table dispatch: {unexpected:?}"),
    };
    Ok(Value::from_function(
        move |state: &State, args: &[Value]| {
            state.lookup(name, &[]).unwrap().call(state, args, &[])
        },
    ))
}

#[derive(Debug)]
struct RecordedCall {
    method: String,
    args: Vec<Value>,
}

#[derive(Debug)]
struct RecordingAdapter {
    public_relation: Option<Value>,
    transient: bool,
    fail_execution: bool,
    calls: Mutex<Vec<RecordedCall>>,
}

impl Object for RecordingAdapter {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        self.calls.lock().push(RecordedCall {
            method: name.to_owned(),
            args: args.to_vec(),
        });
        match name {
            "dispatch" => dispatch(args),
            "get_relation" => {
                let (_, kwargs) = from_args::<(&[Value], Kwargs)>(args)?;
                let identifier: String = kwargs.get("identifier")?;
                Ok(if identifier == "Orders" {
                    self.public_relation
                        .clone()
                        .unwrap_or_else(|| Value::from(()))
                } else {
                    Value::from(())
                })
            }
            "build_catalog_relation" => Ok(Value::from_serialize(serde_json::json!({
                "catalog_type": "INFO_SCHEMA",
                "table_format": null,
                "is_transient": self.transient
            }))),
            "execute" => {
                if self.fail_execution {
                    Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "injected CTAS failure",
                    ))
                } else {
                    Ok(Value::from(vec![Value::from("SUCCESS"), Value::from(())]))
                }
            }
            unexpected => panic!("unexpected native table adapter call: {unexpected}"),
        }
    }
}

fn stage(
    transient: bool,
    public_exists: bool,
    transformation: &str,
    fail_execution: bool,
) -> (FsResult<String>, Arc<RecordingAdapter>) {
    let (_directory, io, mut wap) = fixture(Some(true));
    Arc::make_mut(&mut wap.model)
        .deprecated_config
        .__warehouse_specific_config__
        .transient = Some(transient);
    let execution_model = wap.execution_model().unwrap();
    let mut context = runtime_context(&execution_model, &io);
    context.insert("execute".to_owned(), Value::from(true));
    context.insert("compiled_code".to_owned(), Value::from(transformation));
    context.insert(
        "api".to_owned(),
        Value::from_serialize(BTreeMap::from([(
            "Relation",
            create_static_relation(AdapterType::Snowflake, wap.model.__base_attr__.quoting)
                .unwrap(),
        )])),
    );
    let adapter = Arc::new(RecordingAdapter {
        public_relation: public_exists
            .then(|| RelationObject::new(wap.public_relation().unwrap().into()).into_value()),
        transient,
        fail_execution,
        calls: Mutex::new(Vec::new()),
    });
    context.insert(
        "adapter".to_owned(),
        Value::from_dyn_object(adapter.clone()),
    );
    let environment = JinjaEnvBuilder::new().build();
    let template = [
        TABLE_MACRO,
        CREATE_MACROS,
        CREATE_DISPATCH_MACROS,
        STATEMENT_MACROS,
        HOOK_MACROS,
        GRANT_MACROS,
        DOC_MACROS,
        ADAPTER_MACROS,
        "{{ materialization_table_snowflake() }}",
    ]
    .join("\n");
    let result = environment.render_str(&template, &context, &[]);
    assert_eq!(wap.model.__base_attr__.alias, "Orders");
    (result, adapter)
}

fn submitted_sql(adapter: &RecordingAdapter) -> Vec<String> {
    adapter
        .calls
        .lock()
        .iter()
        .filter(|call| call.method == "execute")
        .map(|call| {
            call.args[0]
                .as_str()
                .unwrap()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn wap_actual_table_materialization_only_rebuilds_the_same_schema_candidate() {
    for transient in [false, true] {
        for public_exists in [false, true] {
            for transformation in [
                "select 1 as id, 10 as amount",
                "select 2 as id, -10 as amount",
            ] {
                let (result, adapter) = stage(transient, public_exists, transformation, false);
                result.unwrap();
                let lifecycle = if transient { "transient " } else { "" };
                assert_eq!(
                    submitted_sql(&adapter),
                    [format!(
                        "create or replace {lifecycle}table \
                         \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" \
                         as ({transformation} ) ;"
                    )],
                    "only the candidate CTAS may be submitted before auditing; \
                     public_exists={public_exists}, transient={transient}"
                );
                let calls = adapter.calls.lock();
                let lookups: Vec<_> = calls
                    .iter()
                    .filter(|call| call.method == "get_relation")
                    .collect();
                assert_eq!(lookups.len(), 1);
                let (_, kwargs) = from_args::<(&[Value], Kwargs)>(&lookups[0].args).unwrap();
                assert_eq!(
                    kwargs.get::<String>("identifier").unwrap(),
                    "__DBT_WAP_TEST"
                );
                assert_eq!(kwargs.get::<String>("database").unwrap(), "My\"Database");
                assert_eq!(kwargs.get::<String>("schema").unwrap(), "My.Schema");
            }
        }
    }
}

#[test]
fn wap_actual_table_materialization_propagates_transformation_failure() {
    let (result, adapter) = stage(true, true, "select missing_column from upstream", true);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected CTAS failure")
    );
    assert_eq!(submitted_sql(&adapter).len(), 1);
    let calls = adapter.calls.lock();
    let dispatched: Vec<_> = calls
        .iter()
        .filter(|call| call.method == "dispatch")
        .map(|call| call.args[0].as_str().unwrap())
        .collect();
    assert_eq!(dispatched, ["set_query_tag", "create_table_as"]);
}
