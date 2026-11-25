from unittest import mock

import pytest
from metricflow_semantic_interfaces.type_enums.export_destination_type import (
    ExportDestinationType,
)

from dbt.contracts.graph.manifest import Manifest
from dbt.events.types import CustomKeyInConfigDeprecation
from dbt.tests.util import run_dbt, update_config_file
from dbt_common.events.event_catcher import EventCatcher
from tests.functional.assertions.test_runner import dbtTestRunner
from tests.functional.configs.fixtures import BaseConfigProject
from tests.functional.saved_queries.fixtures import (
    saved_queries_with_defaults_yml,
    saved_queries_yml,
    saved_query_description,
    saved_query_with_cache_configs_defined_yml,
    saved_query_with_export_configs_defined_at_saved_query_level_yml,
    saved_query_with_export_database_configs_yml,
    saved_query_with_extra_config_attributes_yml,
    saved_query_with_tags_defined_yml,
    saved_query_without_export_configs_defined_yml,
)
from tests.functional.semantic_models.fixtures import (
    fct_revenue_sql,
    metricflow_time_spine_sql,
    schema_yml,
)


class TestSavedQueryConfigs(BaseConfigProject):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "saved-queries": {
                "test": {
                    "test_saved_query": {
                        "+enabled": True,
                        "+export_as": ExportDestinationType.VIEW.value,
                        "+schema": "my_default_export_schema",
                        "+cache": {"enabled": True},
                    }
                },
            },
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_extra_config_attributes_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_basic_saved_query_config(
        self,
        project,
    ):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.config.export_as == ExportDestinationType.VIEW
        assert saved_query.config.schema == "my_default_export_schema"
        assert saved_query.config.cache.enabled is True

        # disable the saved_query via project config and rerun
        config_patch = {"saved-queries": {"test": {"test_saved_query": {"+enabled": False}}}}
        update_config_file(config_patch, project.project_root, "dbt_project.yml")
        result = runner.invoke(["parse"])
        assert result.success
        assert len(result.result.saved_queries) == 0


# Test that the cache will default to enabled = false if not set in the saved_query config
class TestSavedQueryDefaultCacheConfigs(BaseConfigProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_extra_config_attributes_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_basic_saved_query_config(
        self,
        project,
    ):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.config.cache.enabled is False


class TestExportConfigsWithAdditionalProperties(BaseConfigProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_queries_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_extra_config_properties_dont_break_parsing(self, project):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert len(saved_query.exports) == 1
        assert saved_query.exports[0].config.__dict__.get("my_random_config") is None


class TestExportConfigsWithDefaultProperties(BaseConfigProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_queries_with_defaults_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_default_properties(self, project):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert len(saved_query.exports) == 1
        export = saved_query.exports[0]
        assert export.config.alias == "my_export_alias"
        assert export.config.schema_name == project.test_schema
        assert export.config.database == project.database


class TestInheritingExportConfigFromSavedQueryConfig(BaseConfigProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_export_configs_defined_at_saved_query_level_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_export_config_inherits_from_saved_query(self, project):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert len(saved_query.exports) == 2

        # assert Export `my_export` has its configs defined from itself because they should take priority
        export1 = next(
            (export for export in saved_query.exports if export.name == "my_export"), None
        )
        assert export1 is not None
        assert export1.config.export_as == ExportDestinationType.VIEW
        assert export1.config.export_as != saved_query.config.export_as
        assert export1.config.schema_name == "my_custom_export_schema"
        assert export1.config.schema_name != saved_query.config.schema
        assert export1.config.database == project.database

        # assert Export `my_export` has its configs defined from the saved_query because they should take priority
        export2 = next(
            (export for export in saved_query.exports if export.name == "my_export2"), None
        )
        assert export2 is not None
        assert export2.config.export_as == ExportDestinationType.TABLE
        assert export2.config.export_as == saved_query.config.export_as
        assert export2.config.schema_name == "my_default_export_schema"
        assert export2.config.schema_name == saved_query.config.schema
        assert export2.config.database == project.database


class TestInheritingExportConfigsFromProject(BaseConfigProject):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "saved-queries": {
                "test": {
                    "test_saved_query": {
                        "+export_as": ExportDestinationType.VIEW.value,
                    }
                },
            },
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_without_export_configs_defined_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_export_config_inherits_from_project(
        self,
        project,
    ):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.config.export_as == ExportDestinationType.VIEW

        # change export's `export_as` to `TABLE` via project config
        config_patch = {
            "saved-queries": {
                "test": {"test_saved_query": {"+export_as": ExportDestinationType.TABLE.value}}
            }
        }
        update_config_file(config_patch, project.project_root, "dbt_project.yml")
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.config.export_as == ExportDestinationType.TABLE


class TestInheritingExportDatabaseFromSavedQueryConfig(BaseConfigProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_export_database_configs_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    @mock.patch("dbt.jsonschemas.jsonschemas._JSONSCHEMA_SUPPORTED_ADAPTERS", {"postgres"})
    def test_export_database_inherits_from_saved_query(self, project):
        event_catcher = EventCatcher(CustomKeyInConfigDeprecation)
        manifest = run_dbt(
            ["parse", "--no-partial-parse", "--show-all-deprecations"],
            callbacks=[event_catcher.catch],
        )
        # `database` is a supported config, not a custom key
        assert event_catcher.caught_events == []

        saved_query = manifest.saved_queries["saved_query.test.test_saved_query"]

        # the export's own `database` takes priority
        export1 = next(export for export in saved_query.exports if export.name == "my_export")
        assert export1.config.database == "my_custom_export_database"

        # an export without `database` inherits it from the saved query
        export2 = next(export for export in saved_query.exports if export.name == "my_export2")
        assert export2.config.database == "my_default_export_database"

        assert saved_query.config.database == "my_default_export_database"


class TestInheritingExportDatabaseFromProject(BaseConfigProject):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "saved-queries": {
                "test": {
                    "+export_as": ExportDestinationType.TABLE.value,
                    "+schema": "my_project_export_schema",
                    "+database": "my_project_export_database",
                },
            },
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_without_export_configs_defined_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    @mock.patch("dbt.jsonschemas.jsonschemas._JSONSCHEMA_SUPPORTED_ADAPTERS", {"postgres"})
    def test_export_database_inherits_from_project(self, project):
        event_catcher = EventCatcher(CustomKeyInConfigDeprecation)
        manifest = run_dbt(
            ["parse", "--no-partial-parse", "--show-all-deprecations"],
            callbacks=[event_catcher.catch],
        )
        # `+database` is a supported config, not a custom key
        assert event_catcher.caught_events == []

        saved_query = manifest.saved_queries["saved_query.test.test_saved_query"]

        export = saved_query.exports[0]
        assert export.config.schema_name == "my_project_export_schema"
        assert export.config.database == "my_project_export_database"

        assert saved_query.config.database == "my_project_export_database"


# cache can be specified just in a SavedQuery config
class TestSavedQueryLevelCacheConfigs(BaseConfigProject):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_cache_configs_defined_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_basic_saved_query_config(
        self,
        project,
    ):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.config.cache.enabled is True


# the cache defined in yaml for the SavedQuery overrides settings from the dbt_project.toml
class TestSavedQueryCacheConfigsOverride(BaseConfigProject):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "saved-queries": {
                "test": {
                    "test_saved_query": {
                        "+cache": {"enabled": True},
                    }
                },
            },
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_cache_configs_defined_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_override_saved_query_config(
        self,
        project,
    ):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.config.cache.enabled is True

        # set cache to enabled=False via project config but since it's set to true at the saved_query
        # level, it should stay enabled
        config_patch = {
            "saved-queries": {"test": {"test_saved_query": {"+cache": {"enabled": False}}}}
        }
        update_config_file(config_patch, project.project_root, "dbt_project.yml")
        result = runner.invoke(["parse"])
        assert result.success
        assert saved_query.config.cache.enabled is True


# the tags defined in project yaml for the SavedQuery is additive to the query's
class TestSavedQueryTagsAdditiveWithConfig(BaseConfigProject):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "saved-queries": {"+tags": ["tag_b", "tag_c"]},
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "saved_queries.yml": saved_query_with_tags_defined_yml,
            "schema.yml": schema_yml,
            "fct_revenue.sql": fct_revenue_sql,
            "metricflow_time_spine.sql": metricflow_time_spine_sql,
            "docs.md": saved_query_description,
        }

    def test_saved_query_tags_are_additive_unique_and_sorted(
        self,
        project,
    ):
        runner = dbtTestRunner()

        # parse with default fixture project config
        result = runner.invoke(["parse"])
        assert result.success
        assert isinstance(result.result, Manifest)
        assert len(result.result.saved_queries) == 1
        saved_query = result.result.saved_queries["saved_query.test.test_saved_query"]
        assert saved_query.tags == ["tag_a", "tag_b", "tag_c"]
