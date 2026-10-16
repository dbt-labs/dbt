import pytest

from dbt.deprecations import (
    CustomKeyInConfigDeprecation,
    CustomKeyInObjectDeprecation,
    GenericJSONSchemaValidationDeprecation,
    active_deprecations,
    reset_deprecations,
)
from dbt.jsonschemas.jsonschemas import (
    jsonschema_validate,
    project_schema,
    resources_schema,
    validate_model_config,
)
from dbt.tests.util import safe_set_invocation_context
from dbt_common.context import get_invocation_context
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.events.event_manager_client import add_callback_to_manager


class TestValidateModelConfigNoError:
    def test_validate_model_config_no_error(self):
        safe_set_invocation_context()
        get_invocation_context().uses_adapter("snowflake")
        caught_events = []
        add_callback_to_manager(caught_events.append)

        config = {
            "enabled": True,
        }
        validate_model_config(config, "test.yml")
        assert len(caught_events) == 0

    def test_validate_model_config_error(self):
        safe_set_invocation_context()
        get_invocation_context().uses_adapter("snowflake")
        ckiod_catcher = EventCatcher(CustomKeyInObjectDeprecation)
        ckicd_catcher = EventCatcher(CustomKeyInConfigDeprecation)
        gjsvd_catcher = EventCatcher(GenericJSONSchemaValidationDeprecation)
        add_callback_to_manager(ckiod_catcher.catch)
        add_callback_to_manager(ckicd_catcher.catch)
        add_callback_to_manager(gjsvd_catcher.catch)

        config = {
            "non_existent_config": True,  # this config key doesn't exist
            "docs": {
                "show": True,  # this is a valid config key
                "color": "red",  # this is an invalid config key, as it should be `node_color`
            },
        }

        validate_model_config(config, "test.yml")

        assert len(ckiod_catcher.caught_events) == 1
        assert ckiod_catcher.caught_events[0].data.key == "color"
        assert len(ckicd_catcher.caught_events) == 1
        assert ckicd_catcher.caught_events[0].data.key == "non_existent_config"
        assert len(gjsvd_catcher.caught_events) == 0


class TestValidateJsonSchema:
    @pytest.fixture(scope="class")
    def model_bigquery_alias_config_contents(self):
        return {
            "models": [
                {
                    "name": "model_1",
                    "config": {
                        "dataset": "dataset_1",
                        "project": "project_1",
                    },
                }
            ],
        }

    def test_validate_json_schema_no_error_aliases(self, model_bigquery_alias_config_contents):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("bigquery")

        jsonschema_validate(resources_schema(), model_bigquery_alias_config_contents, "test.yml")
        assert active_deprecations == {}

    def test_validate_json_schema_has_error_aliases(self, model_bigquery_alias_config_contents):
        reset_deprecations()

        safe_set_invocation_context()
        # Set to adapter that doesn't support aliases specified
        get_invocation_context().uses_adapter("snowflake")

        jsonschema_validate(resources_schema(), model_bigquery_alias_config_contents, "test.yml")
        assert active_deprecations == {"custom-key-in-config-deprecation": 2}


class TestSourceBigQueryAliases:
    @pytest.fixture(scope="class")
    def source_with_dataset(self):
        return {
            "sources": [
                {
                    "name": "my_source",
                    "dataset": "my_dataset",
                    "tables": [{"name": "my_table"}],
                }
            ]
        }

    @pytest.fixture(scope="class")
    def source_with_project(self):
        return {
            "sources": [
                {
                    "name": "my_source",
                    "project": "my-gcp-project",
                    "tables": [{"name": "my_table"}],
                }
            ]
        }

    @pytest.fixture(scope="class")
    def source_with_both_aliases(self):
        return {
            "sources": [
                {
                    "name": "my_source",
                    "project": "my-gcp-project",
                    "dataset": "my_dataset",
                    "tables": [{"name": "my_table"}],
                }
            ]
        }

    def test_bigquery_source_dataset_no_warning(self, source_with_dataset):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("bigquery")

        jsonschema_validate(resources_schema(), source_with_dataset, "test.yml")
        assert active_deprecations == {}

    def test_bigquery_source_project_no_warning(self, source_with_project):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("bigquery")

        jsonschema_validate(resources_schema(), source_with_project, "test.yml")
        assert active_deprecations == {}

    def test_bigquery_source_both_aliases_no_warning(self, source_with_both_aliases):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("bigquery")

        jsonschema_validate(resources_schema(), source_with_both_aliases, "test.yml")
        assert active_deprecations == {}

    def test_snowflake_source_dataset_warns(self, source_with_dataset):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("snowflake")

        jsonschema_validate(resources_schema(), source_with_dataset, "test.yml")
        assert active_deprecations == {"custom-key-in-object-deprecation": 1}


class TestDatabricksConfigKeys:
    DATABRICKS_CONFIG_KEYS = {
        "query_tags": '{"team": "finance"}',
        "row_filter": "col_a > 0",
        "view_update_via_alter": True,
        "zorder": ["col_a", "col_b"],
        "incremental_apply_config_changes": True,
        "use_safer_relation_operations": True,
        "unique_tmp_table_suffix": True,
        "skip_optimize": True,
        "use_materialization_v2": True,
    }

    def test_model_config_no_warning(self):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("databricks")

        config = {
            "materialized": "incremental",
            "options": {"compression": "snappy"},
            **self.DATABRICKS_CONFIG_KEYS,
        }
        validate_model_config(config, "test.yml")
        assert active_deprecations == {}

    def test_project_config_no_warning(self):
        reset_deprecations()

        safe_set_invocation_context()
        get_invocation_context().uses_adapter("databricks")

        project_dict = {
            "name": "my_project",
            "version": "1.0.0",
            "profile": "my_project",
            "models": {"my_project": {f"+{k}": v for k, v in self.DATABRICKS_CONFIG_KEYS.items()}},
        }
        jsonschema_validate(project_schema(), project_dict, "dbt_project.yml")
        assert active_deprecations == {}
