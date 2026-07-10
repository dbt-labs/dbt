"""Functional tests for overloaded UDF support via overloads (dbt-labs/dbt-core#12250).

Overloaded UDFs are defined as a root function with an `overloads` block in YAML.
Each overload references a separate SQL file (via `defined_in`) with different
argument signatures. They appear as a single node in the DAG.
"""

from typing import Dict

import pytest

from dbt.contracts.graph.nodes import FunctionNode
from dbt.tests.util import get_artifact, run_dbt, run_dbt_and_capture, write_file

# -- Fixtures: root function + one overload ------------------------------------

double_int_sql = """
SELECT val * 2
"""

double_float_sql = """
SELECT val * 2.0
"""

overloaded_functions_yml = """
functions:
  - name: double_int
    description: "Doubles a value (integer and float overloads)"
    arguments:
      - name: val
        data_type: integer
    returns:
      data_type: integer
    overloads:
      - defined_in: double_float
        arguments:
          - name: val
            data_type: float
        returns:
          data_type: float
"""

model_using_overloaded_function_sql = """
SELECT {{ function('double_int') }}(5) as result
"""

# -- Fixtures: an overload body that references another function ----------------

helper_sql = """
SELECT val + 1
"""

wrapper_root_sql = """
SELECT val
"""

# The overload body — and only the overload body — calls another function.
wrapper_overload_sql = """
SELECT {{ function('helper') }}(val)
"""

wrapper_with_overload_ref_yml = """
functions:
  - name: helper
    arguments:
      - name: val
        data_type: integer
    returns:
      data_type: integer
  - name: wrapper
    arguments:
      - name: val
        data_type: integer
    returns:
      data_type: integer
    overloads:
      - defined_in: wrapper_overload
        arguments:
          - name: val
            data_type: float
        returns:
          data_type: float
"""


class TestOverloadedUDFParsing:
    """Parsing a root function with overloads."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "schema.yml": overloaded_functions_yml,
        }

    def test_overloads_parsed_as_single_node(self, project):
        manifest = run_dbt(["parse"])

        # Only the root function should exist — overload is absorbed
        assert len(manifest.functions) == 1
        assert "function.test.double_int" in manifest.functions
        assert "function.test.double_float" not in manifest.functions

        fn = manifest.functions["function.test.double_int"]
        assert isinstance(fn, FunctionNode)

        # Root has its own arguments
        assert fn.arguments[0].data_type == "integer"

        # And one overload with different arguments
        assert len(fn.overloads) == 1
        overload = fn.overloads[0]
        assert overload.defined_in == "double_float"
        assert overload.arguments[0].data_type == "float"
        assert overload.raw_body is not None
        assert "2.0" in overload.raw_body


class TestOverloadedUDFDependency:
    """Model depends on one node, not multiple."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "schema.yml": overloaded_functions_yml,
        }

    @pytest.fixture(scope="class")
    def models(self) -> Dict[str, str]:
        return {
            "my_model.sql": model_using_overloaded_function_sql,
        }

    def test_model_depends_on_single_function_node(self, project):
        manifest = run_dbt(["parse"])

        model = manifest.nodes["model.test.my_model"]
        fn_deps = [d for d in model.depends_on.nodes if d.startswith("function.")]
        assert fn_deps == ["function.test.double_int"]


class TestOverloadedUDFOverloadBodyDependencies:
    """An overload body's ref()/source()/function() calls must land on the root.

    The overload SQL file is absorbed into the root and never becomes its own
    node, so a dependency that appears only inside an overload body would be lost
    from the DAG unless it is carried onto the root during absorption.
    """

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "helper.sql": helper_sql,
            "wrapper.sql": wrapper_root_sql,
            "wrapper_overload.sql": wrapper_overload_sql,
            "schema.yml": wrapper_with_overload_ref_yml,
        }

    def test_root_depends_on_function_referenced_only_in_overload(self, project):
        manifest = run_dbt(["parse"])

        # The overload file is absorbed, not a standalone node.
        assert "function.test.wrapper_overload" not in manifest.functions

        wrapper = manifest.functions["function.test.wrapper"]
        # `helper` is referenced only in the overload body, yet the root must
        # carry the dependency so build order and lineage stay correct.
        assert "function.test.helper" in wrapper.depends_on.nodes


class TestOverloadedUDFBuild:
    """Building creates root + overload functions in one node execution."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "schema.yml": overloaded_functions_yml,
        }

    def test_build_creates_root_and_overloads(self, project):
        results = run_dbt(["build"])
        # Only 1 function node in the DAG
        assert len(results) == 1

        fn_node = results[0].node
        assert isinstance(fn_node, FunctionNode)
        assert fn_node.name == "double_int"


updated_double_float_sql = """
SELECT val * 3.0
"""


class TestOverloadedUDFPartialParsing:
    """Partial parsing picks up changes to overload SQL files."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "schema.yml": overloaded_functions_yml,
        }

    def test_overload_file_change_updates_root(self, project):
        # Initial parse
        manifest = run_dbt(["parse"])
        fn = manifest.functions["function.test.double_int"]
        assert "2.0" in fn.overloads[0].raw_body

        # Change the overload SQL file
        write_file(updated_double_float_sql, project.project_root, "functions", "double_float.sql")

        # Partial parse should pick up the change
        manifest = run_dbt(["parse"])
        fn = manifest.functions["function.test.double_int"]
        assert len(fn.overloads) == 1
        assert "3.0" in fn.overloads[0].raw_body


# -- Fixtures for partial-failure / retry / multi-overload tests ---------------

triple_int_sql = """
SELECT val * 3
"""

double_text_sql = """
SELECT val || val
"""

# An overload that references a non-existent column — Postgres validates
# function bodies at CREATE FUNCTION time, so this fails on creation.
broken_float_sql = """
SELECT val * THIS_COLUMN_DOES_NOT_EXIST
"""

broken_text_sql = """
SELECT val || ANOTHER_BAD_COL
"""

two_overloads_yml = """
functions:
  - name: double_int
    arguments:
      - name: val
        data_type: integer
    returns:
      data_type: integer
    overloads:
      - defined_in: double_float
        arguments:
          - name: val
            data_type: float
        returns:
          data_type: float
      - defined_in: double_text
        arguments:
          - name: val
            data_type: text
        returns:
          data_type: text
"""

one_overload_yml = """
functions:
  - name: double_int
    arguments:
      - name: val
        data_type: integer
    returns:
      data_type: integer
    overloads:
      - defined_in: double_float
        arguments:
          - name: val
            data_type: float
        returns:
          data_type: float
"""


class TestOverloadedUDFPartialFailure:
    """When multiple overloads fail, all of them are attempted (no early exit),
    each error is logged separately, and the result is PARTIAL_SUCCESS with both
    failures recorded in overload_results."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": broken_float_sql,
            "double_text.sql": broken_text_sql,
            "schema.yml": two_overloads_yml,
        }

    def test_all_overloads_attempted_on_failure(self, project):
        _, output = run_dbt_and_capture(["build"], expect_pass=False)

        # Both overload errors should be surfaced (no early exit on first failure)
        assert "double_float" in output
        assert "double_text" in output
        assert "PARTIAL SUCCESS" in output

        # Result artifact tracks each overload's outcome
        run_results = get_artifact(project.project_root, "target", "run_results.json")
        fn_result = next(
            r for r in run_results["results"] if r["unique_id"] == "function.test.double_int"
        )
        assert fn_result["status"] == "partial success"
        assert fn_result["overload_results"] is not None
        assert sorted(fn_result["overload_results"]["failed"]) == ["double_float", "double_text"]
        assert fn_result["overload_results"]["successful"] == []


class TestOverloadedUDFRetrySkipsSuccessful:
    """`dbt retry` should re-attempt only previously-failed overloads, skipping
    overloads that already succeeded."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,  # this one will succeed
            "double_text.sql": broken_text_sql,  # this one will fail
            "schema.yml": two_overloads_yml,
        }

    def test_retry_only_attempts_failed_overloads(self, project):
        # First build: double_float succeeds, double_text fails.
        run_dbt(["build"], expect_pass=False)

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        fn_result = next(
            r for r in run_results["results"] if r["unique_id"] == "function.test.double_int"
        )
        assert fn_result["overload_results"]["successful"] == ["double_float"]
        assert fn_result["overload_results"]["failed"] == ["double_text"]

        # Fix the broken overload, then retry.
        write_file(double_text_sql, project.project_root, "functions", "double_text.sql")
        run_dbt(["retry"])

        run_results = get_artifact(project.project_root, "target", "run_results.json")
        fn_result = next(
            r for r in run_results["results"] if r["unique_id"] == "function.test.double_int"
        )
        # On retry, only double_text was re-attempted. double_float was skipped
        # (already successful), so it shouldn't appear as a fresh success in
        # this invocation's overload_results.
        assert fn_result["status"] == "success"
        assert fn_result["overload_results"]["failed"] == []
        assert "double_text" in fn_result["overload_results"]["successful"]


class TestOverloadedUDFStateModified:
    """Changes to an overload SQL file should be detected by `same_contents`,
    which is what powers `state:modified` selection."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "schema.yml": one_overload_yml,
        }

    def test_overload_change_breaks_same_contents(self, project):
        baseline_manifest = run_dbt(["parse"])
        old_fn = baseline_manifest.functions["function.test.double_int"]

        # Change the overload SQL body
        write_file(updated_double_float_sql, project.project_root, "functions", "double_float.sql")
        new_manifest = run_dbt(["parse"])
        new_fn = new_manifest.functions["function.test.double_int"]

        # Root SQL is unchanged but the overload body differs — same_contents
        # must return False so state:modified picks it up.
        assert not new_fn.same_contents(old_fn, adapter_type=None)
        assert not new_fn.same_overloads(old_fn)


jinja_overload_sql = """
SELECT val * {{ 2.0 }}
"""


class TestOverloadedUDFCompile:
    """`dbt compile` must populate `compiled_body` (Jinja-rendered) on each
    overload so manifest consumers (e.g. catalog tools) see the same compiled
    output regardless of whether the function was actually built."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": jinja_overload_sql,
            "schema.yml": one_overload_yml,
        }

    def test_compile_renders_overload_jinja(self, project):
        run_dbt(["compile", "--select", "double_int"])

        manifest_data = get_artifact(project.project_root, "target", "manifest.json")
        fn = manifest_data["functions"]["function.test.double_int"]
        overload = fn["overloads"][0]

        assert "{{" in overload["raw_body"]  # raw still has Jinja
        assert overload["compiled_body"] is not None
        assert "{{" not in overload["compiled_body"]  # compiled is rendered
        assert "2.0" in overload["compiled_body"]


class TestOverloadedUDFAddOverloadViaYAML:
    """Adding a new overload to YAML on partial parse should re-absorb both
    the existing and new overloads. Regression test for a bug where the
    duplicate-claim check fired on the existing overload's stale ownership
    record."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "double_text.sql": double_text_sql,
            "schema.yml": one_overload_yml,
        }

    def test_add_overload_partial_parse(self, project):
        manifest = run_dbt(["parse"])
        fn = manifest.functions["function.test.double_int"]
        assert [o.defined_in for o in fn.overloads] == ["double_float"]

        # Add double_text as an overload
        write_file(two_overloads_yml, project.project_root, "functions", "schema.yml")
        manifest = run_dbt(["parse"])
        fn = manifest.functions["function.test.double_int"]
        assert sorted(o.defined_in for o in fn.overloads) == ["double_float", "double_text"]


class TestOverloadedUDFRemoveOverloadViaYAML:
    """Removing an overload from YAML on partial parse should leave the
    standalone SQL file as a regular FunctionNode (since dbt doesn't delete
    SQL files automatically)."""

    @pytest.fixture(scope="class")
    def functions(self) -> Dict[str, str]:
        return {
            "double_int.sql": double_int_sql,
            "double_float.sql": double_float_sql,
            "double_text.sql": double_text_sql,
            "schema.yml": two_overloads_yml,
        }

    def test_remove_overload_partial_parse(self, project):
        manifest = run_dbt(["parse"])
        fn = manifest.functions["function.test.double_int"]
        assert sorted(o.defined_in for o in fn.overloads) == ["double_float", "double_text"]

        # Drop double_text from YAML
        write_file(one_overload_yml, project.project_root, "functions", "schema.yml")
        manifest = run_dbt(["parse"])

        # double_int still has double_float as an overload
        fn = manifest.functions["function.test.double_int"]
        assert [o.defined_in for o in fn.overloads] == ["double_float"]

        # double_text re-emerges as a standalone function — its SQL file is
        # still on disk and no longer claimed by any root.
        assert "function.test.double_text" in manifest.functions
