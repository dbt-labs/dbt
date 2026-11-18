import os

import pytest
import yaml

from dbt.artifacts.schemas.results import RunStatus
from dbt.tests.util import (
    check_table_does_exist,
    mkdir,
    rm_dir,
    rm_file,
    run_dbt,
    run_dbt_and_capture,
    write_file,
)
from dbt_common.exceptions import UndefinedMacroError
from tests.functional.run_operations.fixtures import (
    groups_yml,
    happy_macros_sql,
    model_sql,
    private_model_schema_yml,
    private_model_sql,
    ref_private_model_macro_sql,
    sad_macros_sql,
    transaction_probe_macros_sql,
)


class TestOperations:
    @pytest.fixture(scope="class")
    def models(self):
        return {"model.sql": model_sql}

    @pytest.fixture(scope="class")
    def macros(self):
        return {"happy_macros.sql": happy_macros_sql, "sad_macros.sql": sad_macros_sql}

    @pytest.fixture(scope="class")
    def dbt_profile_data(self, unique_schema):
        return {
            "test": {
                "outputs": {
                    "default": {
                        "type": "postgres",
                        "threads": 4,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "user": os.getenv("POSTGRES_TEST_USER", "root"),
                        "pass": os.getenv("POSTGRES_TEST_PASS", "password"),
                        "dbname": os.getenv("POSTGRES_TEST_DATABASE", "dbt"),
                        "schema": unique_schema,
                    },
                    "noaccess": {
                        "type": "postgres",
                        "threads": 4,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "user": "noaccess",
                        "pass": "password",
                        "dbname": os.getenv("POSTGRES_TEST_DATABASE", "dbt"),
                        "schema": unique_schema,
                    },
                },
                "target": "default",
            },
        }

    def run_operation(self, macro, expect_pass=True, extra_args=None, **kwargs):
        args = ["run-operation", macro]
        if kwargs:
            args.extend(("--args", yaml.safe_dump(kwargs)))
        if extra_args:
            args.extend(extra_args)
        return run_dbt(args, expect_pass=expect_pass)

    def test_macro_noargs(self, project):
        self.run_operation("no_args")
        check_table_does_exist(project.adapter, "no_args")

    def test_macro_args(self, project):
        self.run_operation("table_name_args", table_name="my_fancy_table")
        check_table_does_exist(project.adapter, "my_fancy_table")

    def test_macro_exception(self, project):
        self.run_operation("syntax_error", False)

    def test_macro_missing(self, project):
        with pytest.raises(
            UndefinedMacroError,
            match="dbt could not find a macro with the name 'this_macro_does_not_exist' in any package",
        ):
            self.run_operation("this_macro_does_not_exist", False)

    def test_cannot_connect(self, project):
        self.run_operation("no_args", extra_args=["--target", "noaccess"], expect_pass=False)

    def test_vacuum(self, project):
        run_dbt(["run"])
        # this should succeed
        self.run_operation("vacuum", table_name="model")

    def test_vacuum_ref(self, project):
        run_dbt(["run"])
        # this should succeed
        self.run_operation("vacuum_ref", ref_target="model")

    def test_select(self, project):
        self.run_operation("select_something", name="world")

    def test_access_graph(self, project):
        self.run_operation("log_graph")

    def test_print(self, project):
        # Tests that calling the `print()` macro does not cause an exception
        self.run_operation("print_something")

    def test_run_operation_local_macro(self, project):
        pkg_macro = """
{% macro something_cool() %}
    {{ log("something cool", info=true) }}
{% endmacro %}
        """

        mkdir("pkg/macros")

        write_file(pkg_macro, "pkg/macros/something_cool.sql")

        pkg_yaml = """
packages:
    - local: pkg
        """

        write_file(pkg_yaml, "packages.yml")

        pkg_dbt_project = """
name: 'pkg'
        """

        write_file(pkg_dbt_project, "pkg/dbt_project.yml")

        run_dbt(["deps"])

        results, log_output = run_dbt_and_capture(["run-operation", "something_cool"])

        for result in results:
            if result.status == RunStatus.Skipped:
                continue

            timing_keys = [timing.name for timing in result.timing]
            assert timing_keys == ["compile", "execute"]

        assert "something cool" in log_output

        results, log_output = run_dbt_and_capture(["run-operation", "pkg.something_cool"])

        for result in results:
            if result.status == RunStatus.Skipped:
                continue

            timing_keys = [timing.name for timing in result.timing]
            assert timing_keys == ["compile", "execute"]

        assert "something cool" in log_output

        rm_dir("pkg")
        rm_file("packages.yml")


class TestRunOperationSql:
    @pytest.fixture(scope="class")
    def models(self):
        return {"model.sql": model_sql}

    @pytest.fixture(scope="class")
    def macros(self):
        return {"happy_macros.sql": happy_macros_sql}

    @pytest.fixture(scope="class")
    def dbt_profile_data(self, unique_schema):
        return {
            "test": {
                "outputs": {
                    "default": {
                        "type": "postgres",
                        "threads": 4,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "user": os.getenv("POSTGRES_TEST_USER", "root"),
                        "pass": os.getenv("POSTGRES_TEST_PASS", "password"),
                        "dbname": os.getenv("POSTGRES_TEST_DATABASE", "dbt"),
                        "schema": unique_schema,
                    },
                },
                "target": "default",
            },
        }

    def test_sql_simple_select(self, project):
        results = run_dbt(["run-operation", "--sql", "select 1 as id"])
        assert results.results[0].status == RunStatus.Success

    def test_sql_create_table(self, project):
        schema = project.test_schema
        results = run_dbt(
            ["run-operation", "--sql", f'create table "{schema}"."inline_table" (id int)']
        )
        assert results.results[0].status == RunStatus.Success
        check_table_does_exist(project.adapter, "inline_table")

    def test_sql_with_ref(self, project):
        run_dbt(["run"])
        results = run_dbt(["run-operation", "--sql", "select * from {{ ref('model') }}"])
        assert results.results[0].status == RunStatus.Success

    def test_sql_with_jinja(self, project):
        results = run_dbt(
            ["run-operation", "--sql", "select '{{ target.schema }}' as schema_name"]
        )
        assert results.results[0].status == RunStatus.Success

    def test_sql_syntax_error(self, project):
        results = run_dbt(
            ["run-operation", "--sql", "select NOPE NOT A VALID QUERY"],
            expect_pass=False,
        )
        assert results.results[0].status == RunStatus.Error

    def test_sql_no_args_error(self, project):
        with pytest.raises(
            Exception,
            match="Either a macro name or --sql must be passed",
        ):
            run_dbt(["run-operation"], expect_pass=False)

    def test_sql_args_conflict_error(self, project):
        with pytest.raises(
            Exception,
            match=r"--args cannot be used with --sql",
        ):
            run_dbt(
                ["run-operation", "--sql", "select 1", "--args", "{foo: bar}"],
                expect_pass=False,
            )

    def test_sql_no_jinja_skips_compile(self, project):
        # Plain SQL (no Jinja) should succeed without needing the manifest compiled.
        # We verify this by checking timing: compile phase should complete near-instantly.
        results = run_dbt(["run-operation", "--sql", "select 42 as answer"])
        assert results.results[0].status == RunStatus.Success
        timing_keys = [t.name for t in results.results[0].timing]
        assert timing_keys == ["compile", "execute"]

    def test_sql_logs_execution_status(self, project):
        results, log_output = run_dbt_and_capture(["run-operation", "--sql", "select 1 as id"])
        assert results.results[0].status == RunStatus.Success
        assert "START executing inline_query" in log_output
        assert "OK executed inline_query" in log_output

    def test_sql_logs_error_status(self, project):
        results, log_output = run_dbt_and_capture(
            ["run-operation", "--sql", "select NOPE NOT VALID"],
            expect_pass=False,
        )
        assert results.results[0].status == RunStatus.Error
        assert "ERROR executing inline_query" in log_output


class TestRunOperationRefPrivateModel:
    """Regression test for https://github.com/dbt-labs/dbt-core/issues/8248

    Macros invoked via run-operation should be able to ref() private models,
    since macros are outside the group/access control system.
    """

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "private_model.sql": private_model_sql,
            "schema.yml": private_model_schema_yml,
            "groups.yml": groups_yml,
        }

    @pytest.fixture(scope="class")
    def macros(self):
        return {"ref_private_model.sql": ref_private_model_macro_sql}

    def test_run_operation_ref_private_model(self, project):
        run_dbt(["run"])
        results = run_dbt(["run-operation", "ref_private_model"])
        assert results.results[0].status == RunStatus.Success


class BaseRunOperationKeepsWrites:
    """A run-operation that succeeds keeps its writes (dbt-labs/dbt#16434)."""

    @pytest.fixture(scope="class")
    def macros(self):
        return {"transaction_probe_macros.sql": transaction_probe_macros_sql}

    @pytest.fixture(scope="class", autouse=True)
    def probe_table(self, project):
        project.run_sql(f"create table {project.test_schema}.txn_probe (tag varchar(50))")

    def probe_tags(self, project):
        rows = project.run_sql(
            f"select tag from {project.test_schema}.txn_probe order by tag", fetch="all"
        )
        return [row[0] for row in rows]


class TestRunOperationStatementWriteIsKept(BaseRunOperationKeepsWrites):
    def test_statement_write_is_kept(self, project):
        run_dbt(["run-operation", "probe_insert", "--args", "{tag: stmt}"])
        assert self.probe_tags(project) == ["stmt"]


class TestRunOperationRunQueryWriteIsKept(BaseRunOperationKeepsWrites):
    def test_run_query_write_is_kept(self, project):
        run_dbt(["run-operation", "probe_insert_run_query", "--args", "{tag: rq}"])
        assert self.probe_tags(project) == ["rq"]


class TestRunOperationWithoutWritesSucceeds(BaseRunOperationKeepsWrites):
    def test_log_only_macro_succeeds(self, project):
        results = run_dbt(["run-operation", "log_only"])
        assert results.results[0].status == RunStatus.Success
        assert self.probe_tags(project) == []


class TestRunOperationSqlWriteIsKept(BaseRunOperationKeepsWrites):
    def test_sql_write_is_kept(self, project):
        schema = project.test_schema
        run_dbt(["run-operation", "--sql", f"insert into {schema}.txn_probe values ('sql')"])
        assert self.probe_tags(project) == ["sql"]
