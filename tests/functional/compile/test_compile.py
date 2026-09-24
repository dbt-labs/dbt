import json
import pathlib
import re

import pytest
import sqlparse

from dbt.tests.util import read_file, run_dbt, run_dbt_and_capture
from dbt_common.exceptions import DbtBaseException as DbtException
from dbt_common.exceptions import DbtRuntimeError
from tests.functional.assertions.test_runner import dbtTestRunner
from tests.functional.compile.fixtures import (
    first_ephemeral_model_sql,
    first_ephemeral_model_with_alias_sql,
    first_model_sql,
    incremental_model_sql,
    model_multiline_jinja,
    schema_yml,
    second_ephemeral_model_sql,
    second_ephemeral_model_with_alias_sql,
    second_model_sql,
    table_model_using_is_incremental_sql,
    third_ephemeral_model_sql,
    with_recursive_model_sql,
)
from tests.functional.v2_parser_parity.v2_self_parser import (
    run_dbt_and_capture_for_mode,
    run_dbt_for_mode,
)


def norm_whitespace(string):
    _RE_COMBINE_WHITESPACE = re.compile(r"\s+")
    string = _RE_COMBINE_WHITESPACE.sub(" ", string).strip()
    return string


def get_lines(model_name):
    f = read_file("target", "compiled", "test", "models", model_name + ".sql")
    return [line for line in f.splitlines() if line]


def file_exists(model_name):
    from dbt.tests.util import file_exists

    return file_exists("target", "compiled", "test", "models", model_name + ".sql")


class TestIntrospectFlag:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "first_model.sql": first_model_sql,
            "second_model.sql": second_model_sql,
            "schema.yml": schema_yml,
        }

    @pytest.mark.v2_parser_parity
    def test_default(self, project, parser_mode):
        run_dbt_for_mode(parser_mode, ["compile"])
        assert get_lines("first_model") == ["select 1 as fun"]
        assert any("_test_compile as schema" in line for line in get_lines("second_model"))

    def test_no_introspect(self, project):
        with pytest.raises(DbtRuntimeError, match="connection never acquired for thread"):
            run_dbt(["compile", "--no-introspect"])


class TestEphemeralModels:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "first_ephemeral_model.sql": first_ephemeral_model_sql,
            "second_ephemeral_model.sql": second_ephemeral_model_sql,
            "third_ephemeral_model.sql": third_ephemeral_model_sql,
            "with_recursive_model.sql": with_recursive_model_sql,
        }

    @pytest.mark.v2_parser_parity
    def test_first_selector(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--select", "first_ephemeral_model"]
        )
        assert file_exists("first_ephemeral_model")
        assert not file_exists("second_ephemeral_model")
        assert not file_exists("third_ephemeral_model")
        assert "Compiled node 'first_ephemeral_model' is" in log_output

    @pytest.mark.v2_parser_parity
    def test_middle_selector(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--select", "second_ephemeral_model"]
        )
        assert file_exists("first_ephemeral_model")
        assert file_exists("second_ephemeral_model")
        assert not file_exists("third_ephemeral_model")
        assert "Compiled node 'second_ephemeral_model' is" in log_output

    @pytest.mark.v2_parser_parity
    def test_last_selector(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--select", "third_ephemeral_model"]
        )
        assert file_exists("first_ephemeral_model")
        assert file_exists("second_ephemeral_model")
        assert file_exists("third_ephemeral_model")
        assert "Compiled node 'third_ephemeral_model' is" in log_output

    @pytest.mark.v2_parser_parity
    def test_no_selector(self, project, parser_mode):
        run_dbt_for_mode(parser_mode, ["compile"])

        sql = read_file("target", "compiled", "test", "models", "first_ephemeral_model.sql")
        assert norm_whitespace(sql) == norm_whitespace("select 1 as fun")
        sql = read_file("target", "compiled", "test", "models", "second_ephemeral_model.sql")
        expected_sql = """with __dbt__cte__first_ephemeral_model as (
            select 1 as fun
            ) select * from __dbt__cte__first_ephemeral_model"""
        assert norm_whitespace(sql) == norm_whitespace(expected_sql)
        sql = read_file("target", "compiled", "test", "models", "third_ephemeral_model.sql")
        expected_sql = """with __dbt__cte__first_ephemeral_model as (
            select 1 as fun
            ),  __dbt__cte__second_ephemeral_model as (
            select * from __dbt__cte__first_ephemeral_model
            ) select * from __dbt__cte__second_ephemeral_model
            union all
            select 2 as fun"""
        assert norm_whitespace(sql) == norm_whitespace(expected_sql)

    @pytest.mark.v2_parser_parity
    def test_with_recursive_cte(self, project, parser_mode):
        run_dbt_for_mode(parser_mode, ["compile"])

        assert get_lines("with_recursive_model") == [
            "with recursive  __dbt__cte__first_ephemeral_model as (",
            "select 1 as fun",
            "), t(n) as (",
            "    select * from __dbt__cte__first_ephemeral_model",
            "  union all",
            "    select n+1 from t where n < 100",
            ")",
            "select sum(n) from t;",
        ]


class TestEphemeralModelWithAlias:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "first_ephemeral_model_with_alias.sql": first_ephemeral_model_with_alias_sql,
            "second_ephemeral_model_with_alias.sql": second_ephemeral_model_with_alias_sql,
        }

    @pytest.mark.v2_parser_parity
    def test_compile(self, project, parser_mode):
        run_dbt_for_mode(parser_mode, ["compile"])

        assert get_lines("second_ephemeral_model_with_alias") == [
            "with __dbt__cte__first_alias as (",
            "select 1 as fun",
            ") select * from __dbt__cte__first_alias",
        ]


class TestCompile:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "first_model.sql": first_model_sql,
            "second_model.sql": second_model_sql,
            "schema.yml": schema_yml,
        }

    @pytest.mark.v2_parser_parity
    def test_none(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(parser_mode, ["compile"])
        assert len(results) == 4
        assert "Compiled node" not in log_output

    def test_inline_pass(self, project):
        (results, log_output) = run_dbt_and_capture(
            ["compile", "--inline", "select * from {{ ref('first_model') }}"]
        )
        assert len(results) == 1
        assert "Compiled inline node is:" in log_output

    def test_inline_pass_quiet(self, project):
        (results, log_output) = run_dbt_and_capture(
            ["compile", "--quiet", "--inline", "select * from {{ ref('first_model') }}"]
        )
        assert len(results) == 1
        assert "Compiled inline node is:" not in log_output

    @pytest.mark.v2_parser_parity
    def test_select_pass(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--select", "second_model"]
        )
        assert len(results) == 3
        assert "Compiled node 'second_model' is:" in log_output

    @pytest.mark.v2_parser_parity
    def test_select_pass_quiet(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--quiet", "--select", "second_model"]
        )
        assert len(results) == 3
        assert "Compiled node 'second_model' is:" not in log_output

    @pytest.mark.v2_parser_parity
    def test_select_pass_empty(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--indirect-selection", "empty", "--select", "second_model"]
        )
        assert len(results) == 1
        assert "Compiled node 'second_model' is:" in log_output

    def test_inline_fail(self, project):
        with pytest.raises(DbtException, match="Error parsing inline query"):
            run_dbt(["compile", "--inline", "select * from {{ ref('third_model') }}"])

    def test_inline_fail_database_error(self, project):
        with pytest.raises(DbtRuntimeError, match="Database Error"):
            run_dbt(["show", "--inline", "slect asdlkjfsld;j"])

    def test_multiline_jinja(self, project):
        (results, log_output) = run_dbt_and_capture(["compile", "--inline", model_multiline_jinja])
        assert len(results) == 1
        assert "Compiled inline node is:" in log_output

    @pytest.mark.v2_parser_parity
    def test_output_json_select(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--select", "second_model", "--output", "json"]
        )
        assert len(results) == 3
        assert "node" in log_output
        assert "compiled" in log_output
        with pytest.raises(json.JSONDecodeError):
            json.loads(log_output)

    @pytest.mark.v2_parser_parity
    def test_output_json_select_quiet(self, project, parser_mode):
        (results, log_output) = run_dbt_and_capture_for_mode(
            parser_mode, ["compile", "--quiet", "--select", "second_model", "--output", "json"]
        )
        assert len(results) == 3
        assert "node" in log_output
        assert "compiled" in log_output
        json.loads(log_output)

    def test_output_json_inline(self, project):
        (results, log_output) = run_dbt_and_capture(
            ["compile", "--inline", "select * from {{ ref('second_model') }}", "--output", "json"]
        )
        assert len(results) == 1
        assert '"node"' not in log_output
        assert '"compiled"' in log_output
        with pytest.raises(json.JSONDecodeError):
            json.loads(log_output)

    def test_output_json_inline_quiet(self, project):
        (results, log_output) = run_dbt_and_capture(
            [
                "compile",
                "--quiet",
                "--inline",
                "select * from {{ ref('second_model') }}",
                "--output",
                "json",
            ]
        )
        assert len(results) == 1
        assert '"node"' not in log_output
        assert '"compiled"' in log_output
        json.loads(log_output)

    def test_compile_inline_not_add_node(self, project):
        dbt = dbtTestRunner()
        parse_result = dbt.invoke(["parse"])
        manifest = parse_result.result
        assert len(manifest.nodes) == 4
        dbt = dbtTestRunner(manifest=manifest)
        dbt.invoke(
            ["compile", "--inline", "select * from {{ ref('second_model') }}"],
            populate_cache=False,
        )
        assert len(manifest.nodes) == 4

    def test_compile_inline_syntax_error(self, project, mocker):
        patched_fire_event = mocker.patch("dbt.task.compile.fire_event")
        with pytest.raises(DbtException, match="Error parsing inline query"):
            run_dbt(["compile", "--inline", "select * from {{ ref(1) }}"])
        # Event for parsing error fired
        patched_fire_event.assert_called_once()

    def test_compile_inline_ref_node_not_exist(self, project, mocker):
        patched_fire_event = mocker.patch("dbt.task.compile.fire_event")
        with pytest.raises(DbtException, match="Error parsing inline query"):
            run_dbt(["compile", "--inline", "select * from {{ ref('third_model') }}"])
        # Event for parsing error fired
        patched_fire_event.assert_called_once()

    def test_graph_summary_output(self, project):
        """Ensure that the compile command generates a file named graph_summary.json
        in the target directory, that the file contains valid json, and that the
        json has the high level structure it should."""
        dbtTestRunner().invoke(["compile"])
        summary_path = pathlib.Path(project.project_root, "target/graph_summary.json")
        with open(summary_path, "r") as summary_file:
            summary = json.load(summary_file)
            assert "_invocation_id" in summary
            assert "linked" in summary


class TestSqlParseGroupingTokenLimit:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "first_ephemeral_model.sql": first_ephemeral_model_sql,
            "second_ephemeral_model.sql": second_ephemeral_model_sql,
            "third_ephemeral_model.sql": third_ephemeral_model_sql,
            "with_recursive_model.sql": with_recursive_model_sql,
        }

    def test_sqlparse_grouping_token_limit(self, project):
        # No flag: compile succeeds (default is no limit)
        run_dbt(["compile"])
        assert sqlparse.engine.grouping.MAX_GROUPING_TOKENS is None

        # Flag set to 0: compile fails because token limit is exceeded
        with pytest.raises(DbtRuntimeError, match="You may raise the limit via --sqlparse"):
            run_dbt(["compile", "--sqlparse", '{"MAX_GROUPING_TOKENS": "0"}'])

        # Flag set to 10000: compile succeeds
        run_dbt(["compile", "--sqlparse", '{"MAX_GROUPING_TOKENS": "10000"}'])


class TestSqlParseGroupingDepthLimit:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "first_ephemeral_model.sql": first_ephemeral_model_sql,
            "second_ephemeral_model.sql": second_ephemeral_model_sql,
            "third_ephemeral_model.sql": third_ephemeral_model_sql,
            "with_recursive_model.sql": with_recursive_model_sql,
        }

    def test_sqlparse_grouping_depth_limit(self, project):
        # No flag: compile succeeds (default is no limit)
        run_dbt(["compile"])
        assert sqlparse.engine.grouping.MAX_GROUPING_DEPTH is None

        # Flag set to 0: compile fails because depth limit is exceeded
        with pytest.raises(DbtRuntimeError, match="You may raise the limit via --sqlparse"):
            run_dbt(["compile", "--sqlparse", '{"MAX_GROUPING_DEPTH": "0"}'])

        # Flag set to 10000: compile succeeds
        run_dbt(["compile", "--sqlparse", '{"MAX_GROUPING_DEPTH": "10000"}'])


class TestSqlParseOnlyTokensLeavesDepthNone:
    def test_only_tokens_set_leaves_depth_none(self, project):
        run_dbt(["compile", "--sqlparse", '{"MAX_GROUPING_TOKENS": "10000"}'])
        assert sqlparse.engine.grouping.MAX_GROUPING_DEPTH is None


class TestSqlParseOnlyDepthLeavesTokensNone:
    def test_only_depth_set_leaves_tokens_none(self, project):
        run_dbt(["compile", "--sqlparse", '{"MAX_GROUPING_DEPTH": "10000"}'])
        assert sqlparse.engine.grouping.MAX_GROUPING_TOKENS is None


class TestNoFullRefreshFlag:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "incremental_model.sql": incremental_model_sql,
            "table_model.sql": table_model_using_is_incremental_sql,
        }

    def _compiled(self, model_name):
        return norm_whitespace(read_file("target", "compiled", "test", "models", model_name + ".sql"))

    def test_incremental_model_treated_as_incremental(self, project):
        run_dbt(["compile", "--no-full-refresh"])
        assert "where fun > 0" in self._compiled("incremental_model")

    def test_default_does_not_treat_as_incremental(self, project):
        run_dbt(["compile"])
        assert "where fun > 0" not in self._compiled("incremental_model")

    def test_non_incremental_model_unaffected(self, project):
        run_dbt(["compile", "--no-full-refresh"])
        assert "where fun > 0" not in self._compiled("table_model")

    def test_mutually_exclusive_with_full_refresh(self, project):
        with pytest.raises(DbtRuntimeError, match="mutually exclusive"):
            run_dbt(["compile", "--no-full-refresh", "--full-refresh"])
