//! Execute the shipped materialization against a recording warehouse boundary.
//! These checks validate the submitted SQL, not Snowflake execution or atomicity.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use dbt_adapter::relation::{RelationObject, factory::create_static_relation};
use dbt_adapter::stmt_splitter::{DefaultStmtSplitter, StmtSplitter};
use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::JinjaEnvBuilder;
use dbt_jinja_utils::phases::run::build_run_node_context;
use dbt_schemas::schemas::common::Hooks;
use dbt_telemetry::ExecutionPhase;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Kwargs, Object, from_args};
use minijinja::{Error, ErrorKind, State, Value};
use parking_lot::Mutex;

use super::clone_tests::fixture;
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
pub(super) struct RecordingAdapter {
    candidate_relation: Value,
    candidate_exists: Mutex<bool>,
    transient: bool,
    fail_execution: bool,
    fail_creation: bool,
    fail_on: Option<&'static str>,
    calls: Mutex<Vec<RecordedCall>>,
    statements: Mutex<Vec<String>>,
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
                // Mirror the Snowflake adapter's statement splitting so a
                // header failure can stop before the CTAS in the same call.
                for statement in DefaultStmtSplitter.split(sql, AdapterType::Snowflake) {
                    if DefaultStmtSplitter.is_empty(statement, AdapterType::Snowflake) {
                        continue;
                    }
                    self.statements.lock().push(normalize_sql(statement));
                    if let Some(failure) = self.fail_on
                        && statement.contains(failure)
                    {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            format!("injected failure: {failure}"),
                        ));
                    }
                }
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

#[derive(Default)]
pub(super) struct StageConfig {
    pub pre_hooks: Vec<String>,
    pub post_hooks: Vec<String>,
    pub sql_header: Option<String>,
    pub runtime_sql_header: Option<Value>,
    pub fail_execution: bool,
    pub fail_creation: bool,
    pub fail_on: Option<&'static str>,
}

fn stage(
    transient: bool,
    source: Option<TableLifecycle>,
    transformation: &str,
    fail_execution: bool,
    fail_creation: bool,
) -> (FsResult<String>, Arc<RecordingAdapter>) {
    stage_with_config(
        transient,
        source,
        transformation,
        StageConfig {
            fail_execution,
            fail_creation,
            ..Default::default()
        },
    )
}

pub(super) fn stage_with_config(
    transient: bool,
    source: Option<TableLifecycle>,
    transformation: &str,
    config: StageConfig,
) -> (FsResult<String>, Arc<RecordingAdapter>) {
    let (_directory, io, mut wap) = fixture(Some(true));
    let model_config = &mut Arc::make_mut(&mut wap.model).deprecated_config;
    model_config.__warehouse_specific_config__.transient = Some(transient);
    model_config.pre_hook = Some(Hooks::ArrayOfStrings(config.pre_hooks)).into();
    model_config.post_hook = Some(Hooks::ArrayOfStrings(config.post_hooks)).into();
    model_config.sql_header = config.sql_header;
    let execution_model = wap.execution_model().unwrap();
    let (mut context, _) = build_run_node_context(
        &execution_model,
        &execution_model.deprecated_config,
        AdapterType::Snowflake,
        None,
        &BTreeMap::new(),
        &io,
        ExecutionPhase::Run,
        config.runtime_sql_header,
        BTreeSet::new(),
    );
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
        fail_execution: config.fail_execution,
        fail_creation: config.fail_creation,
        fail_on: config.fail_on,
        calls: Mutex::new(Vec::new()),
        statements: Mutex::new(Vec::new()),
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

fn normalize_sql(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn submitted_sql(adapter: &RecordingAdapter) -> Vec<String> {
    adapter
        .calls
        .lock()
        .iter()
        .filter(|call| call.method == "execute")
        .map(|call| normalize_sql(call.args[0].as_str().unwrap()))
        .collect()
}

fn hooks_and_header() -> StageConfig {
    StageConfig {
        pre_hooks: vec![
            "delete from {{ this }} where id < 0".to_owned(),
            "update {{ this }} set id = id + 1".to_owned(),
        ],
        post_hooks: vec![
            "update {{ this }} set id = id + 2".to_owned(),
            "delete from {{ this }} where id = 0".to_owned(),
        ],
        sql_header: Some("set wap_test_header = 7;".to_owned()),
        ..Default::default()
    }
}

#[test]
fn wap_hooks_render_candidate_this_once_around_header_and_transformation() {
    let (result, adapter) = stage_with_config(
        true,
        Some(TableLifecycle::Permanent),
        "select 1 as id",
        hooks_and_header(),
    );
    result.unwrap();
    let candidate = "\"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\"";
    assert_eq!(
        submitted_sql(&adapter),
        [
            format!(
                "create transient table {candidate} clone \"My\"\"Database\".\"My.Schema\".\"Orders\""
            ),
            format!("delete from {candidate} where id < 0"),
            format!("update {candidate} set id = id + 1"),
            format!(
                "set wap_test_header = 7; create or replace transient table {candidate} as (select 1 as id ) ;"
            ),
            format!("update {candidate} set id = id + 2"),
            format!("delete from {candidate} where id = 0"),
        ]
    );
}

#[test]
fn wap_does_not_rewrite_hook_sql_already_resolved_to_the_public_table() {
    let sql = "update \"My\"\"Database\".\"My.Schema\".\"Orders\" set id = 1";
    let (result, adapter) = stage_with_config(
        true,
        Some(TableLifecycle::Permanent),
        "select 1 as id",
        StageConfig {
            post_hooks: vec![sql.to_owned()],
            ..Default::default()
        },
    );
    result.unwrap();
    assert_eq!(submitted_sql(&adapter).last().unwrap(), sql);
}

#[test]
fn wap_hook_header_and_transformation_failures_stop_later_statements() {
    for (failure, expected_statements) in [
        ("where id < 0", 2),
        ("set wap_test_header", 4),
        ("create or replace", 5),
        ("id = id + 2", 6),
    ] {
        let (result, adapter) = stage_with_config(
            true,
            Some(TableLifecycle::Permanent),
            "select 1 as id",
            StageConfig {
                fail_on: Some(failure),
                ..hooks_and_header()
            },
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains(&format!("injected failure: {failure}"))
        );
        let statements = adapter.statements.lock();
        assert_eq!(statements.len(), expected_statements, "{failure}");
        assert!(statements.last().unwrap().contains(failure));
        assert!(*adapter.candidate_exists.lock());
    }
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
