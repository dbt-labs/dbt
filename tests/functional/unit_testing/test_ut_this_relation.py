"""
Regression test for https://github.com/dbt-labs/dbt-core/issues/12313

``this`` in unit test context was returned as a raw string (the ephemeral
CTE name), so macros that accessed Relation attributes like
``this.database``, ``this.schema``, or ``this.identifier`` raised
``AttributeError: 'str' object has no attribute 'database'``.
"""
import pytest
from dbt.tests.util import run_dbt

macro_use_relation_attrs_sql = """
{% macro assert_relation_attrs(rel) %}
    {% if rel.database is none and rel.schema is none and rel.identifier is none %}
        {{ exceptions.raise_compiler_error("all relation attributes are None") }}
    {% endif %}
    {{ return(true) }}
{% endmacro %}
"""

model_with_this_relation_sql = """
{{ config(materialized='incremental') }}
{% do assert_relation_attrs(this) %}
select id
from {{ ref('my_source') }}
{% if is_incremental() %}
where id not in (select id from {{ this }})
{% endif %}
"""

model_my_source_sql = """
select 1 as id
union all
select 2 as id
"""

schema_yml = """
unit_tests:
  - name: test_this_exposes_relation_attrs
    model: model_with_this_relation
    overrides:
      macros:
        is_incremental: true
    given:
      - input: ref('my_source')
        rows:
          - {id: 1}
          - {id: 2}
      - input: this
        rows:
          - {id: 1}
    expect:
      rows:
        - {id: 2}
"""


class TestUnitTestThisExposesRelationAttrs:
    """Regression test for #12313: this.database/.schema/.identifier fail in unit tests."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_this_relation.sql": model_with_this_relation_sql,
            "my_source.sql": model_my_source_sql,
            "schema.yml": schema_yml,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"assert_relation_attrs.sql": macro_use_relation_attrs_sql}

    def test_this_relation_attrs_accessible_in_unit_test(self, project):
        """Unit test must pass without 'str object has no attribute database'."""
        run_dbt(["run"])
        results = run_dbt(["test"])
        assert len(results) == 1
        assert results[0].status == "pass"