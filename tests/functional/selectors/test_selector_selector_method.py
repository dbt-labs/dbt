from typing import Any

import pytest

from dbt.events.types import SelectExcludeIgnoredWithSelectorWarning
from dbt.exceptions import DbtSelectorsError
from dbt.tests.util import run_dbt
from dbt_common.events.event_catcher import EventCatcher
from dbt_common.exceptions import RecursionError as DbtRecursionError

models__model_a_sql = """
SELECT 1 AS id
"""

models_model_b_sql = """
SELECT 1 AS id
"""

models_model_a_plus_1_sql = """
SELECT * FROM {{ ref('model_a') }}
"""

models_model_a_plus_2_sql = """
SELECT * FROM {{ ref('model_a_plus_1') }}
"""

models_model_c_sql = """
{{ config(tags=['tag_c']) }}

SELECT * FROM {{ ref('model_a') }}
UNION ALL
SELECT * FROM {{ ref('model_b') }}
"""


selectors__yml = """
selectors:
  - name: model_a_selector
    description: Selects model_a
    definition:
      method: fqn
      value: model_a

  - name: model_b_selector
    description: Selects model_b
    definition:
      method: fqn
      value: model_b

  - name: model_c_selector
    description: Selects model_c
    definition:
      method: fqn
      value: model_c

  - name: model_a_b_selector
    description: Selects model_a and model_b
    definition:
      union:
        - method: fqn
          value: model_a
        - method: fqn
          value: model_b

  - name: model_a_plus_1_selector
    description: Selects model_a_plus_1
    definition:
      method: fqn
      value: model_a_plus_1

  - name: model_a_plus_2_selector
    description: Selects model_a_plus_2
    definition:
      method: fqn
      value: model_a_plus_2
      parents: true
      parents_depth: 2

  - name: recursive_selector
    description: Selects recursive models
    definition:
      union:
        - model_b
        - selector:model_a_plus_2_selector

  - name: recursive_with_wildcards
    description: Selects recursive models with wildcards
    definition:
      union:
        - model_c
        - selector:model_[ab]_selector

  - name: circular_selection_hop
    description: Selects circular selection hop models
    definition:
      union:
        - selector:circular_dependency_selector

  - name: circular_dependency_selector
    description: Selects circular dependency models
    definition:
      union:
        - selector:circular_selection_hop

  - name: model_a_explicit_selector
    description: Selects model_a with an explicit selector method
    definition:
      method: selector
      value: model_a_selector
"""


@pytest.fixture(scope="class")
def models():
    return {
        "model_a.sql": models__model_a_sql,
        "model_a_plus_1.sql": models_model_a_plus_1_sql,
        "model_a_plus_2.sql": models_model_a_plus_2_sql,
        "model_b.sql": models_model_b_sql,
        "model_c.sql": models_model_c_sql,
    }


@pytest.fixture(scope="class")
def selectors():
    return selectors__yml


def assert_result_set(actual: Any, expected: set[str]):
    assert isinstance(actual, list)
    assert set(actual) == expected


def assert_reference_matches_inline(
    inline: str, referenced: str, args: list[str], expected: set[str]
):
    assert_result_set(run_dbt(["ls", "--select", inline, *args]), expected)
    assert_result_set(run_dbt(["ls", "--select", referenced, *args]), expected)


class TestSelectorSelectorMethod:
    def test_ls_with_selector_returns_model_a(self, project):
        result = run_dbt(["ls", "--select", "selector:model_a_selector"])
        assert_result_set(result, {"test.model_a"})

    def test_ls_with_selector_union(self, project):
        result = run_dbt(["ls", "--select", "selector:model_a_selector selector:model_b_selector"])
        assert_result_set(result, {"test.model_a", "test.model_b"})

    def test_ls_with_selector_intersection(self, project):
        result = run_dbt(
            ["ls", "--select", "selector:model_a_b_selector,selector:model_b_selector"]
        )
        assert_result_set(result, {"test.model_b"})

    def test_ls_with_graph_operator(self, project):
        # one child one parent
        result = run_dbt(["ls", "--select", "1+selector:model_a_plus_1_selector+1"])
        assert_result_set(
            result,
            {
                "test.model_a",
                "test.model_a_plus_1",
                "test.model_a_plus_2",
            },
        )

        # two parents
        result = run_dbt(["ls", "--select", "1+selector:model_c_selector"])
        assert_result_set(result, {"test.model_a", "test.model_b", "test.model_c"})

    def test_selector_depth_overrides_operator_depth(self, project):
        result = run_dbt(["ls", "--select", "selector:model_a_plus_2_selector"])
        assert_result_set(
            result,
            {
                "test.model_a",
                "test.model_a_plus_1",
                "test.model_a_plus_2",
            },
        )

        result = run_dbt(["ls", "--select", "1+selector:model_a_plus_2_selector"])
        assert_result_set(result, {"test.model_a", "test.model_a_plus_1", "test.model_a_plus_2"})

    def test_combine_with_other_methods(self, project):
        result = run_dbt(["ls", "--select", "selector:model_a_selector tag:tag_c"])
        assert_result_set(result, {"test.model_a", "test.model_c"})

        result = run_dbt(["ls", "--select", "selector:model_a_plus_2_selector model_a model_b"])
        assert_result_set(
            result,
            {
                "test.model_a",
                "test.model_b",
                "test.model_a_plus_1",
                "test.model_a_plus_2",
            },
        )

    def test_recursive_selector(self, project):
        result = run_dbt(["ls", "--select", "selector:recursive_selector"])
        assert_result_set(
            result,
            {
                "test.model_a",
                "test.model_b",
                "test.model_a_plus_1",
                "test.model_a_plus_2",
            },
        )

    def test_select_and_exclude(self, project):
        result = run_dbt(
            [
                "ls",
                "--select",
                "1+selector:model_a_plus_1_selector",
                "--exclude",
                "selector:model_a_selector",
            ]
        )
        assert_result_set(result, {"test.model_a_plus_1"})

    def test_wildcards(self, project):
        result = run_dbt(["ls", "--select", "selector:model_?_selector"])
        assert_result_set(result, {"test.model_a", "test.model_b", "test.model_c"})

        result = run_dbt(["ls", "--select", "selector:*_c_*"])
        assert_result_set(result, {"test.model_c"})

        result = run_dbt(["ls", "--select", "selector:model_[ab]_selector"])
        assert_result_set(result, {"test.model_a", "test.model_b"})

        result = run_dbt(["ls", "--select", "selector:model_[a-c]_selector"])
        assert_result_set(result, {"test.model_a", "test.model_b", "test.model_c"})

    def test_recursive_with_wildcards(self, project):
        result = run_dbt(["ls", "--select", "selector:recursive_with_wildcards"])
        assert_result_set(result, {"test.model_a", "test.model_b", "test.model_c"})

    def test_circular_dependency(self, project):
        with pytest.raises(DbtRecursionError):
            run_dbt(["ls", "--select", "selector:circular_dependency_selector"], expect_pass=False)

    def test_selector_with_explicit_selector_method(self, project):
        result = run_dbt(["ls", "--select", "selector:model_a_explicit_selector"])
        assert_result_set(result, {"test.model_a"})

    def test_raises_if_no_selector_matches(self, project):
        with pytest.raises(DbtSelectorsError):
            run_dbt(["ls", "--select", "selector:nonexistent_selector"])


exclude_test_name_models__schema_yml = """
version: 2
models:
  - name: model_a
    columns:
      - name: id
        tests:
          - not_null
  - name: model_b
    columns:
      - name: id
        tests:
          - not_null
"""

exclude_test_name_selectors__yml = """
selectors:
  - name: model_a_selector
    description: Selects model_a
    definition:
      method: fqn
      value: model_a

  - name: exclude_not_null_selector
    description: Selects all models but excludes not_null tests
    definition:
      union:
        - method: fqn
          value: '*'
        - exclude:
            - method: test_name
              value: not_null

  - name: inherit_exclude_not_null_selector
    description: Inherits exclude_not_null_selector via selector method
    definition:
      intersection:
        - method: selector
          value: exclude_not_null_selector
"""


class TestExcludeTestNameInheritedViaSelectorMethod:
    """Regression test: exclude: test_name must be preserved when a selector is
    inherited via `method: selector`. Previously, expand_selection() ran again on
    the result of get_selected() and re-introduced tests that had been explicitly
    excluded (because their parent models were still in the selected set).
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": exclude_test_name_models__schema_yml,
            "model_a.sql": models__model_a_sql,
            "model_b.sql": models_model_b_sql,
        }

    @pytest.fixture(scope="class")
    def selectors(self):
        return exclude_test_name_selectors__yml

    def test_exclude_test_name_directly(self, project):
        result = run_dbt(["ls", "--selector", "exclude_not_null_selector"])
        assert not any(
            "not_null" in node for node in result
        ), "not_null tests should be excluded by the direct selector"

    def test_exclude_test_name_inherited_via_selector_method(self, project):
        # Confirm not_null tests exist in the project
        all_not_null = run_dbt(["ls", "--select", "test_name:not_null"])
        assert len(all_not_null) > 0, "not_null tests must exist to make this test meaningful"

        direct_result = set(run_dbt(["ls", "--selector", "exclude_not_null_selector"]))
        inherited_result = set(run_dbt(["ls", "--selector", "inherit_exclude_not_null_selector"]))

        assert not any("not_null" in node for node in inherited_result), (
            "not_null tests should still be excluded when the selector is inherited "
            "via method: selector"
        )
        assert (
            direct_result == inherited_result
        ), "inherited selector result should match the directly-referenced selector result"


class TestSelectExcludeIgnoredWithSelectorWarning:
    """Test that SelectExcludeIgnoredWithSelectorWarning is raised when CLI is invoked with
    --selector together with --select or --exclude.
    """

    def test_warning_raised_when_selector_and_select(self, project):
        event_catcher = EventCatcher(SelectExcludeIgnoredWithSelectorWarning)
        run_dbt(
            ["ls", "--selector", "model_a_selector", "--select", "model_a"],
            callbacks=[event_catcher.catch],
        )
        assert len(event_catcher.caught_events) == 1

    def test_warning_raised_when_selector_and_exclude(self, project):
        event_catcher = EventCatcher(SelectExcludeIgnoredWithSelectorWarning)
        run_dbt(
            ["ls", "--selector", "model_a_selector", "--exclude", "model_b"],
            callbacks=[event_catcher.catch],
        )
        assert len(event_catcher.caught_events) == 1

    def test_no_warning_when_selector_only(self, project):
        event_catcher = EventCatcher(SelectExcludeIgnoredWithSelectorWarning)
        run_dbt(
            ["ls", "--selector", "model_a_selector"],
            callbacks=[event_catcher.catch],
        )
        assert len(event_catcher.caught_events) == 0

    def test_warning_raised_when_selector_and_select_and_exclude(self, project):
        event_catcher = EventCatcher(SelectExcludeIgnoredWithSelectorWarning)
        run_dbt(
            [
                "ls",
                "--selector",
                "model_a_selector",
                "--select",
                "model_a",
                "--exclude",
                "model_b",
            ],
            callbacks=[event_catcher.catch],
        )
        assert len(event_catcher.caught_events) == 1

    def test_no_warning_when_no_selector(self, project):
        event_catcher = EventCatcher(SelectExcludeIgnoredWithSelectorWarning)
        run_dbt(
            ["ls", "--select", "model_a", "--exclude", "model_b"],
            callbacks=[event_catcher.catch],
        )
        assert len(event_catcher.caught_events) == 0

    def test_all_commands_requiring_selectors_support_selectors(self, project):
        # tests all commands except for clone as clone functional tests are in adapters
        commands = [
            "build",
            ("docs", "generate"),
            "compile",
            ("show", "--select", "selector:model_a_selector"),
            "list",
            "ls",
            "run",
            "seed",
            "snapshot",
            ("source", "freshness"),
            "test",
        ]

        for command in commands:
            if isinstance(command, tuple):
                run_dbt([*command, "--selector", "model_a_explicit_selector"], expect_pass=True)
            else:
                run_dbt([command, "--selector", "model_a_explicit_selector"], expect_pass=True)


graph_operator_models__schema_yml = """
version: 2
models:
  - name: model_a
    columns:
      - name: id
        tests:
          - not_null
  - name: model_a_plus_1
    columns:
      - name: id
        tests:
          - not_null
"""

graph_operator_seed_child_sql = """
SELECT * FROM {{ ref('my_seed') }}
"""

graph_operator_selectors__yml = """
selectors:
  - name: sel_a
    definition: 'model_a'

  - name: sel_a_plus_1
    definition: 'model_a_plus_1'

  - name: sel_seed
    definition: 'my_seed'

  - name: sel_a_plus_1_without_not_null
    definition:
      union:
        - method: fqn
          value: model_a_plus_1
        - exclude:
            - method: test_name
              value: not_null
"""


class TestSelectorReferenceWithGraphOperatorAndResourceType:
    """A `selector:` reference with a graph operator must select the same nodes as
    the equivalent inline selection, whatever resource types the command keeps.
    The resource-type filter applies once, after graph operators.
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": graph_operator_models__schema_yml,
            "model_a.sql": models__model_a_sql,
            "model_a_plus_1.sql": models_model_a_plus_1_sql,
            "model_seed_child.sql": graph_operator_seed_child_sql,
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"my_seed.csv": "id\n1\n"}

    @pytest.fixture(scope="class")
    def selectors(self):
        return graph_operator_selectors__yml

    @pytest.mark.parametrize(
        "case",
        [
            (
                "model_a+1",
                "selector:sel_a+1",
                ["--resource-type", "test"],
                {"test.not_null_model_a_id", "test.not_null_model_a_plus_1_id"},
            ),
            (
                "+model_a_plus_1",
                "+selector:sel_a_plus_1",
                ["--resource-type", "test"],
                {"test.not_null_model_a_id", "test.not_null_model_a_plus_1_id"},
            ),
            (
                "my_seed+1",
                "selector:sel_seed+1",
                ["--resource-type", "model"],
                {"test.model_seed_child"},
            ),
        ],
        ids=["children_tests", "parents_tests", "seed_children"],
    )
    def test_matches_inline(self, project, case):
        assert_reference_matches_inline(*case)

    def test_referenced_exclusions_kept_with_parents_operator(self, project):
        # The referenced selector's not_null exclusion still applies to its own
        # model, while the parent reached by `+` gets its tests.
        result = run_dbt(
            [
                "ls",
                "--select",
                "+selector:sel_a_plus_1_without_not_null",
                "--resource-type",
                "test",
            ]
        )
        assert_result_set(result, {"test.not_null_model_a_id"})


multi_parent_models__schema_yml = """
version: 2
models:
  - name: model_x
    columns:
      - name: id
        tests:
          - not_null
  - name: model_a
    columns:
      - name: id
        tests:
          - relationships:
              arguments:
                to: ref('model_x')
                field: id
  - name: model_b
    columns:
      - name: id
        tests:
          - not_null
          - relationships:
              arguments:
                to: ref('model_a')
                field: id
"""

multi_parent_selectors__yml = """
selectors:
  - name: sel_a
    definition: 'model_a'

  - name: sel_x
    definition: 'model_x'

  - name: sel_x_and_relationships
    definition:
      union:
        - 'model_x'
        - 'relationships_model_a_id__id__ref_model_x_'

  - name: all_but_b
    definition:
      union:
        - method: fqn
          value: '*'
        - exclude:
            - method: fqn
              value: model_b

  - name: tests_of_a
    definition:
      intersection:
        - 'model_a'
        - 'resource_type:test'

  - name: sel_a_empty
    definition:
      method: fqn
      value: model_a
      indirect_selection: empty

  - name: sel_a_without_relationships
    definition:
      union:
        - method: fqn
          value: model_a
        - exclude:
            - method: test_name
              value: relationships

  - name: sel_b_without_relationships
    definition:
      union:
        - method: fqn
          value: model_b
        - exclude:
            - method: test_name
              value: relationships
"""


class MultiParentProject:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": multi_parent_models__schema_yml,
            "model_x.sql": "select 1 as id",
            "model_a.sql": "select 1 as id",
            "model_b.sql": "select id from {{ ref('model_a') }}",
        }

    @pytest.fixture(scope="class")
    def selectors(self):
        return multi_parent_selectors__yml


class TestSelectorReferenceWithMultiParentTests(MultiParentProject):
    """Tests attached by a referenced selector must not act as a starting point for
    graph operators, and tests it excluded must stay excluded, also when they touch
    a node the operator adds.
    """

    def test_excluded_test_touching_added_parent_stays_excluded(self, project):
        result = run_dbt(
            [
                "ls",
                "--select",
                "+selector:sel_b_without_relationships",
                "--resource-type",
                "test",
            ]
        )
        assert_result_set(
            result,
            {"test.not_null_model_b_id", "test.relationships_model_a_id__id__ref_model_x_"},
        )

    def test_excluded_test_is_not_selected_as_child(self, project):
        result = run_dbt(
            [
                "ls",
                "--select",
                "selector:sel_a_without_relationships+1",
                "--resource-type",
                "test",
            ]
        )
        assert_result_set(result, {"test.not_null_model_b_id"})


REL_A_X = "test.relationships_model_a_id__id__ref_model_x_"
REL_B_A = "test.relationships_model_b_id__id__ref_model_a_"


class TestSelectorReferenceMatchesInline(MultiParentProject):
    """A `selector:` reference selects the same nodes as the equivalent inline
    selection: operators start from the nodes the referenced selector matched (not
    the tests it attached), and tests it holds back under cautious selection are
    selected once their parents are.
    """

    @pytest.mark.parametrize(
        "case",
        [
            (
                "+model_a",
                "+selector:sel_a",
                ["--resource-type", "model"],
                {"test.model_a"},
            ),
            (
                "model_a+1",
                "selector:sel_a+1",
                ["--resource-type", "test", "--indirect-selection", "cautious"],
                {"test.not_null_model_b_id", REL_A_X, REL_B_A},
            ),
            (
                "model_a model_x",
                "selector:sel_a selector:sel_x",
                ["--resource-type", "test", "--indirect-selection", "cautious"],
                {"test.not_null_model_x_id", REL_A_X},
            ),
            (
                "model_a+1",
                "selector:sel_a+1",
                ["--indirect-selection", "empty"],
                {"test.model_a", "test.model_b", REL_A_X, REL_B_A},
            ),
            (
                "@model_a",
                "@selector:sel_a",
                [],
                {
                    "test.model_a",
                    "test.model_b",
                    "test.model_x",
                    "test.not_null_model_b_id",
                    "test.not_null_model_x_id",
                    REL_A_X,
                    REL_B_A,
                },
            ),
            (
                "+model_x +relationships_model_a_id__id__ref_model_x_",
                "+selector:sel_x_and_relationships",
                ["--resource-type", "model"],
                {"test.model_x", "test.model_a"},
            ),
            (
                "model_a+1",
                "selector:sel_a_empty+1",
                ["--resource-type", "test"],
                {"test.not_null_model_b_id", REL_A_X, REL_B_A},
            ),
            (
                "model_a+1 model_x+1",
                "selector:all_but_b+1",
                ["--resource-type", "test"],
                {"test.not_null_model_b_id", "test.not_null_model_x_id", REL_A_X, REL_B_A},
            ),
            (
                "+relationships_model_a_id__id__ref_model_x_ "
                "+relationships_model_b_id__id__ref_model_a_",
                "+selector:tests_of_a",
                [],
                {
                    "test.model_a",
                    "test.model_b",
                    "test.model_x",
                    "test.not_null_model_b_id",
                    "test.not_null_model_x_id",
                    REL_A_X,
                    REL_B_A,
                },
            ),
        ],
        ids=[
            "parents_skip_attached_tests",
            "cautious_children",
            "cautious_union",
            "empty_mode_children",
            "at_operator",
            "directly_selected_test_is_a_starting_point",
            "empty_mode_reference_under_eager_selection",
            "excluded_model_brought_back_gets_its_tests",
            "intersection_tests_are_starting_points",
        ],
    )
    def test_matches_inline(self, project, case):
        assert_reference_matches_inline(*case)
