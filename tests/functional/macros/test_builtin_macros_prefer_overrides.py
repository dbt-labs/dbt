import pytest

from dbt.tests.util import run_dbt, run_dbt_and_capture

# A project macro that shares a builtin's name but not its signature.
# dbt's default__get_drop_sql calls `drop_table(relation)`.
drop_table_sql = """
{% macro drop_table(source_name, table_name) %}
    {{ source(source_name, table_name) }}
{% endmacro %}
"""

seed_csv = """id
1
"""


class BaseBuiltinMacrosPreferOverrides:
    @pytest.fixture(scope="class")
    def macros(self):
        return {"drop_table.sql": drop_table_sql}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"my_seed.csv": seed_csv}


class TestBuiltinMacrosPreferOverridesDefault(BaseBuiltinMacrosPreferOverrides):
    def test_root_macro_does_not_shadow_builtin(self, project):
        run_dbt(["seed"])
        # --full-refresh drops the existing seed via adapter.drop_relation
        results = run_dbt(["seed", "--full-refresh"])
        assert len(results) == 1


class TestBuiltinMacrosPreferOverridesEnabled(BaseBuiltinMacrosPreferOverrides):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {"flags": {"builtin_macros_prefer_overrides": True}}

    def test_root_macro_shadows_builtin(self, project):
        run_dbt(["seed"])
        _, output = run_dbt_and_capture(["seed", "--full-refresh"], expect_pass=False)
        assert "argument to source() must be a string" in output
        assert "in macro drop_table" in output
