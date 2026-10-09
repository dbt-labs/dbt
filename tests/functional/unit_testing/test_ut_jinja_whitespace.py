"""
Regression test for https://github.com/dbt-labs/dbt-core/issues/11999

When Jinja whitespace-stripping modifiers are used around ``ref()`` or
``source()`` calls -- e.g. ``{{- ref("model") -}}`` -- the space before
``{{`` is stripped from the rendered SQL.  In a regular dbt run this is
harmless because ``ref()`` returns a quoted relation
(e.g. ``"schema"."table"``), so ``from"schema"."table"`` is valid SQL.

In a unit-test run, however, every input is represented as an ephemeral CTE
with an *unquoted* identifier (e.g. ``__dbt__cte__add_input``).  Whitespace
stripping collapsed ``from `` + identifier into the single invalid token
``from__dbt__cte__add_input``, causing a SQL syntax error.

The fix is to quote the ephemeral CTE identifier returned by ``ref()`` and
``source()`` in unit-test context.  ``from"__dbt__cte__add_input"`` is
correctly parsed by every major SQL engine as keyword FROM followed by a
quoted identifier.
"""
import pytest
from dbt.tests.util import run_dbt

# ---------------------------------------------------------------------------
# fixtures
# ---------------------------------------------------------------------------

add_input_sql = """
select 1 as a, 2 as b
union all
select 3 as a, 4 as b
"""

# Model that uses both-sides whitespace stripping around ref().
# The leading {{- strips the space between "from" and the Jinja tag.
add_strip_sql = """
{{ config(materialized='table') }}
select
    a + b as c
from {{- ref("add_input") -}}
"""

schema_yml = """
unit_tests:
  - name: test_whitespace_stripped_ref_works
    model: add_strip
    given:
      - input: ref('add_input')
        rows:
          - {a: 1, b: 2}
          - {a: 10, b: 20}
    expect:
      rows:
        - {c: 3}
        - {c: 30}
"""


# ---------------------------------------------------------------------------
# test class
# ---------------------------------------------------------------------------

class TestJinjaWhitespaceStrippedRef:
    """Regression test for #11999: {{- ref() -}} must not break unit tests."""

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "add_input.sql": add_input_sql,
            "add_strip.sql": add_strip_sql,
            "schema.yml": schema_yml,
        }

    def test_unit_test_passes_with_whitespace_stripped_ref(self, project):
        """Unit test must succeed even when the model uses {{- ref("...") -}}."""
        run_dbt(["run"])
        results = run_dbt(["test"])
        assert len(results) == 1
        assert results[0].status == "pass"