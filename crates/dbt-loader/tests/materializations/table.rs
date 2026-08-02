use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

mod sqlserver {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::SqlServer;

    fn build_harness() -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(ADAPTER)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("harness should build");

        let mock = harness.mock();
        mock.on("quote", |args| {
            Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
        });
        mock.on("rename_relation", |_| Ok(Value::UNDEFINED));
        mock.on("drop_relation", |_| Ok(Value::UNDEFINED));
        mock.on("commit", |_| Ok(Value::UNDEFINED));
        mock.on("render_raw_model_constraints", |_| {
            Ok(Value::from(Vec::<Value>::new()))
        });

        harness
    }

    /// A `config` mock that always answers `contract`/`indexes` with a
    /// no-op-safe default (empty), with room for a test to override specific
    /// keys. `config.get(key, default=...)` uses a keyword arg for `default`
    /// on some call sites (e.g. dbt-adapters' own `create_indexes`), which
    /// this harness's generic positional fallback doesn't unwrap correctly -
    /// so any key a macro iterates or branches on needs its own explicit arm
    /// here rather than relying on the passthrough default.
    fn config_mock(overrides: BTreeMap<&'static str, Value>) -> Arc<MockJinjaObject> {
        let mock = Arc::new(MockJinjaObject::new());
        mock.on("get", move |args| {
            let key = args.first().and_then(|v| v.as_str()).unwrap_or("");
            if let Some(value) = overrides.get(key) {
                return Ok(value.clone());
            }
            match key {
                "contract" => Ok(Value::from_serialize(BTreeMap::from([(
                    "enforced".to_string(),
                    Value::from(false),
                )]))),
                "indexes" => Ok(Value::from(Vec::<Value>::new())),
                // Positional `config.get(key, default)` calls (table_refresh_method,
                // full_refresh_build, ...) land their default in args[1]; keyword
                // `default=` calls (dbt-adapters' own create_indexes) don't, which is
                // why indexes/contract get their own arms above instead of relying
                // on this.
                _ => Ok(args.get(1).cloned().unwrap_or(Value::UNDEFINED)),
            }
        });
        mock.on("persist_column_docs", |_| Ok(Value::from(false)));
        mock.on("persist_relation_docs", |_| Ok(Value::from(false)));
        mock
    }

    fn render_table(
        harness: &MacroTestHarness,
        ctx: BTreeMap<String, Value>,
    ) -> dbt_common::FsResult<String> {
        harness.render("{{ materialization_table_sqlserver() }}", ctx)
    }

    #[test]
    fn no_existing_relation_renames_intermediate_into_target() {
        let harness = build_harness();
        harness.mock().on("get_relation", |_| Ok(Value::from(())));

        let ctx = harness
            .materialization_context("my_table", "SELECT id FROM source_table")
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(config_mock(BTreeMap::new())))
            .build();
        render_table(&harness, ctx)
            .unwrap_or_else(|e| panic!("table materialization failed: {e:?}"));

        // Intermediate -> target only; no existing/backup relation to swap out.
        assert_eq!(
            harness
                .mock()
                .observed_calls()
                .to("rename_relation")
                .count(),
            1,
            "expected only the intermediate->target rename"
        );
    }

    #[test]
    fn existing_table_renamed_to_backup_before_swap() {
        let harness = build_harness();
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_table",
            Some(RelationType::Table),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });

        let ctx = harness
            .materialization_context("my_table", "SELECT id FROM source_table")
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(config_mock(BTreeMap::new())))
            .build();
        render_table(&harness, ctx)
            .unwrap_or_else(|e| panic!("table materialization failed: {e:?}"));

        // Two renames: existing -> backup, then intermediate -> target.
        assert_eq!(
            harness
                .mock()
                .observed_calls()
                .to("rename_relation")
                .count(),
            2,
            "expected existing->backup and intermediate->target renames"
        );
    }

    #[test]
    fn full_refresh_build_prebuilt_raises_compiler_error() {
        let harness = build_harness();
        harness.mock().on("get_relation", |_| Ok(Value::from(())));

        let overrides = BTreeMap::from([("full_refresh_build", Value::from("prebuilt"))]);
        let ctx = harness
            .materialization_context("my_table", "SELECT id FROM source_table")
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(config_mock(overrides)))
            .build();

        let result = render_table(&harness, ctx);
        assert!(
            result.is_err(),
            "full_refresh_build='prebuilt' should raise a compiler error, got: {result:?}"
        );
    }

    #[test]
    fn indexes_config_raises_compiler_error() {
        let harness = build_harness();
        harness.mock().on("get_relation", |_| Ok(Value::from(())));

        let overrides = BTreeMap::from([(
            "indexes",
            Value::from_serialize(vec![BTreeMap::from([(
                "columns".to_string(),
                Value::from(vec![Value::from("id")]),
            )])]),
        )]);
        let ctx = harness
            .materialization_context("my_table", "SELECT id FROM source_table")
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(config_mock(overrides)))
            .build();

        let result = render_table(&harness, ctx);
        assert!(
            result.is_err(),
            "a configured `indexes:` should raise a compiler error rather than silently no-op, got: {result:?}"
        );
    }
}
