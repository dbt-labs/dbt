//! Execute the shipped materialization against a recording warehouse boundary.
//! These checks validate the submitted SQL, not Snowflake execution or atomicity.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use dbt_adapter::relation::{RelationObject, factory::create_static_relation};
use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::JinjaEnvBuilder;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Kwargs, Object, from_args};
use minijinja::{Error, ErrorKind, State, Value};
use parking_lot::Mutex;

use super::clone_tests::{fixture, runtime_context};
use super::{TableLifecycle, initialize_candidate};

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
const ADAPTER_GRANT_MACROS: &str =
    include_str!("../../../dbt-loader/src/dbt_macro_assets/dbt-snowflake/macros/apply_grants.sql");
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
        Some("copy_grants") => "snowflake__copy_grants",
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
    candidate_relation: Value,
    candidate_exists: Mutex<bool>,
    transient: bool,
    fail_execution: bool,
    fail_creation: bool,
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
                Ok(
                    if identifier == "__DBT_WAP_TEST" && *self.candidate_exists.lock() {
                        self.candidate_relation.clone()
                    } else {
                        Value::from(())
                    },
                )
            }
            "build_catalog_relation" => Ok(Value::from_serialize(serde_json::json!({
                "catalog_type": "INFO_SCHEMA",
                "table_format": null,
                "is_transient": self.transient
            }))),
            "execute" => {
                let sql = args[0].as_str().unwrap();
                if sql.contains(" clone ") || sql.contains("__DBT_WAP_PLACEHOLDER boolean") {
                    if self.fail_creation {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "injected candidate creation failure",
                        ));
                    }
                    let mut exists = self.candidate_exists.lock();
                    if *exists {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "candidate already exists",
                        ));
                    }
                    *exists = true;
                } else if self.fail_execution {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "injected CTAS failure",
                    ));
                }
                Ok(Value::from(vec![Value::from("SUCCESS"), Value::from(())]))
            }
            unexpected => panic!("unexpected native table adapter call: {unexpected}"),
        }
    }
}

fn stage(
    transient: bool,
    source: Option<TableLifecycle>,
    transformation: &str,
    fail_execution: bool,
    fail_creation: bool,
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
        candidate_relation: RelationObject::new(wap.candidate_relation().unwrap().into())
            .into_value(),
        candidate_exists: Mutex::new(false),
        transient,
        fail_execution,
        fail_creation,
        calls: Mutex::new(Vec::new()),
    });
    context.insert(
        "adapter".to_owned(),
        Value::from_dyn_object(adapter.clone()),
    );
    let environment = JinjaEnvBuilder::new().build();
    let preparation = initialize_candidate(
        wap.public_relation().unwrap().as_ref(),
        wap.candidate_relation().unwrap().as_ref(),
        source,
        transient,
        |sql| {
            context.insert("__wap_sql".to_owned(), Value::from(sql));
            environment.render_str(
                "{{ adapter.execute(__wap_sql, auto_begin=false, fetch=false) }}",
                &context,
                &[],
            )?;
            Ok(())
        },
    );
    if let Err(error) = preparation {
        return (Err(error), adapter);
    }
    let template = [
        TABLE_MACRO,
        CREATE_MACROS,
        CREATE_DISPATCH_MACROS,
        STATEMENT_MACROS,
        HOOK_MACROS,
        GRANT_MACROS,
        ADAPTER_GRANT_MACROS,
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
fn wap_claims_or_clones_before_rebuilding_the_same_schema_candidate() {
    for transient in [false, true] {
        for source in [
            None,
            Some(TableLifecycle::Permanent),
            Some(TableLifecycle::Transient),
        ] {
            if source == Some(TableLifecycle::Transient) && !transient {
                continue;
            }
            for transformation in [
                "select 1 as id, 10 as amount",
                "select 2 as id, -10 as amount",
            ] {
                let (result, adapter) = stage(transient, source, transformation, false, false);
                result.unwrap();
                let lifecycle = if transient { "transient " } else { "" };
                let mut expected = Vec::new();
                if source.is_some() {
                    expected.push(format!(
                        "create {lifecycle}table \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" \
                         clone \"My\"\"Database\".\"My.Schema\".\"Orders\""
                    ));
                } else {
                    expected.push(format!(
                        "create {lifecycle}table \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" \
                         (__DBT_WAP_PLACEHOLDER boolean)"
                    ));
                }
                expected.push(format!(
                    "create or replace {lifecycle}table \
                         \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" \
                         as ({transformation} ) ;"
                ));
                assert_eq!(
                    submitted_sql(&adapter),
                    expected,
                    "source={source:?}, transient={transient}"
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
    let (result, adapter) = stage(
        true,
        Some(TableLifecycle::Transient),
        "select missing_column from upstream",
        true,
        false,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected CTAS failure")
    );
    assert_eq!(submitted_sql(&adapter).len(), 2);
    assert!(*adapter.candidate_exists.lock());
    let calls = adapter.calls.lock();
    let dispatched: Vec<_> = calls
        .iter()
        .filter(|call| call.method == "dispatch")
        .map(|call| call.args[0].as_str().unwrap())
        .collect();
    assert_eq!(dispatched, ["set_query_tag", "create_table_as"]);
}

#[test]
fn wap_candidate_creation_failure_prevents_transformation() {
    for source in [None, Some(TableLifecycle::Permanent)] {
        let (result, adapter) = stage(true, source, "select 1 as id", false, true);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("injected candidate creation failure"));
        assert!(error.contains("was not replaced"));
        assert!(error.contains("may have been created"));
        assert_eq!(submitted_sql(&adapter).len(), 1);
        assert!(!*adapter.candidate_exists.lock());
        assert!(
            !adapter
                .calls
                .lock()
                .iter()
                .any(|call| call.method == "dispatch")
        );
    }
}

#[test]
fn wap_concurrent_first_build_claims_have_one_winner() {
    use std::sync::Barrier;

    let (_directory, _io, wap) = fixture(Some(false));
    let target = wap.public_relation().unwrap();
    let candidate = wap.candidate_relation().unwrap();
    let barrier = Barrier::new(2);
    let claimed = Mutex::new(false);

    // Both builds have observed absent public and working tables. Their first
    // writes must claim the name rather than replace another build's candidate.
    let claim = || {
        initialize_candidate(target.as_ref(), candidate.as_ref(), None, true, |sql| {
            assert_eq!(
                sql,
                "create transient table \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" \
                 (__DBT_WAP_PLACEHOLDER boolean)"
            );
            barrier.wait();
            let mut claimed = claimed.lock();
            if *claimed {
                return Err(fs_err!(
                    ErrorCode::ExecutionError,
                    "candidate already exists"
                ));
            }
            *claimed = true;
            Ok(())
        })
    };
    std::thread::scope(|scope| {
        let first = scope.spawn(&claim);
        let second = scope.spawn(&claim);
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let error = results.into_iter().find_map(Result::err).unwrap();
        assert!(error.to_string().contains("candidate already exists"));
    });
    assert!(*claimed.lock());
}

#[test]
fn wap_rejects_transient_to_permanent_clone_before_writing() {
    let (result, adapter) = stage(
        false,
        Some(TableLifecycle::Transient),
        "select 1 as id",
        false,
        false,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("cannot clone transient public table")
    );
    assert!(adapter.calls.lock().is_empty());
}
