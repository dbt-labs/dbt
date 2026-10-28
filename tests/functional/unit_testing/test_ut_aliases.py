import pytest

from dbt.tests.util import run_dbt

model_with_alias_sql = """
{{
    config(
        alias='beautiful_alias',
        schema='events',
        materialized='view'
    )
}}

select
    'foo' as foo
"""

model_tested = """
select * from {{ ref('model_with_alias') }}
"""

unit_test_yml = """
unit_tests:
  - name: test_model_with_alias_input
    model: model_tested
    given:
      - input: ref('model_with_alias')
        rows:
          - {foo: bar }
          - {foo: foo }
    expect:
      rows:
          - {foo: bar }
          - {foo: foo }
"""


class TestUnitTestInputWithAlias:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_alias.sql": model_with_alias_sql,
            "model_tested.sql": model_tested,
            "unit_test.yml": unit_test_yml,
        }

    def test_input_with_alias(self, project):
        results = run_dbt(["run"])
        assert len(results) == 2

        results = run_dbt(["test"])
        assert len(results) == 1


model_with_alias_a_sql = """
{{
    config(
        alias='shared_alias',
        schema='schema_a',
        materialized='view'
    )
}}
select 1 as id_a
"""

model_with_alias_b_sql = """
{{
    config(
        alias='shared_alias',
        schema='schema_b',
        materialized='view'
    )
}}
select 2 as id_b
"""

model_tested_duplicate_alias_sql = """
select id_a from {{ ref('model_with_alias_a') }}
union all
select id_b from {{ ref('model_with_alias_b') }}
"""

unit_test_duplicate_alias_yml = """
unit_tests:
  - name: test_duplicate_alias_inputs
    model: model_tested_duplicate_alias
    given:
      - input: ref('model_with_alias_a')
        rows:
          - {id_a: 10}
      - input: ref('model_with_alias_b')
        rows:
          - {id_b: 20}
    expect:
      rows:
        - {id_a: 10}
        - {id_a: 20}
"""


class TestUnitTestDuplicateAliasInputs:
    """Regression test for https://github.com/dbt-labs/dbt-core/issues/10740.

    When two upstream models share the same alias (but have different model
    names/schemas), the unit test compiler previously generated duplicate CTE
    names (__dbt__cte__<alias>) for both input nodes, causing:

        Parser Error: Duplicate CTE name "__dbt__cte__<alias>"

    The fix is to use the model's *name* (always unique within a project) as
    the alias for ephemeral unit-test input nodes, not its *identifier*
    (which reflects the configured alias and can collide across models).
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model_with_alias_a.sql": model_with_alias_a_sql,
            "model_with_alias_b.sql": model_with_alias_b_sql,
            "model_tested_duplicate_alias.sql": model_tested_duplicate_alias_sql,
            "unit_test.yml": unit_test_duplicate_alias_yml,
        }

    def test_duplicate_alias_inputs(self, project):
        run_dbt(["run"])
        results = run_dbt(["test"])
        assert len(results) == 1
