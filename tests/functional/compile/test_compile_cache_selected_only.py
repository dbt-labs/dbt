from unittest import mock

import pytest

from dbt.adapters.sql.impl import SQLAdapter
from dbt.tests.util import run_dbt

model_in_default_schema_sql = """
select 1 as fun
"""

model_in_other_schema_sql = """
{{ config(schema='cache_selected_only_other') }}
select 1 as fun
"""


class TestCompileCacheSelectedOnly:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_in_default_schema.sql": model_in_default_schema_sql,
            "model_in_other_schema.sql": model_in_other_schema_sql,
        }

    def test_cache_selected_only_scopes_compile_to_selected_schema(self, project):
        # Build both models once so both schemas exist in the database.
        run_dbt(["run"])

        # dbt's default `generate_schema_name` suffixes the custom schema onto
        # the profile's target schema, rather than replacing it outright.
        other_schema = f"{project.test_schema}_cache_selected_only_other"

        original_list_relations = SQLAdapter.list_relations_without_caching

        def query_schemas(args):
            with mock.patch.object(
                SQLAdapter,
                "list_relations_without_caching",
                autospec=True,
                side_effect=original_list_relations,
            ) as listed_schemas:
                run_dbt(args)
            return {call.args[1].schema for call in listed_schemas.call_args_list}

        # With the flag, cache population is scoped to the selected model's schema only.
        queried_schemas = query_schemas(
            ["compile", "--select", "model_in_default_schema", "--cache-selected-only"]
        )
        assert queried_schemas == {project.test_schema}
        assert other_schema not in queried_schemas

        # Without the flag, every schema in the project is still scanned.
        queried_schemas = query_schemas(
            ["compile", "--select", "model_in_default_schema", "--no-cache-selected-only"]
        )
        assert other_schema in queried_schemas
