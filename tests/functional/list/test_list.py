import pytest
import os
import json

from dbt.tests.util import run_dbt
from dbt.logger import log_manager

from tests.functional.list.fixtures import (  # noqa: F401
    snapshots,
    tests,
    models,
    macros,
    seeds,
    analyses,
    project_files,
)


class TestList:
    def dir(self, value):
        return os.path.normpath(value)

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "config-version": 2,
            "analysis-paths": [self.dir("analyses")],
            "snapshot-paths": [self.dir("snapshots")],
            "macro-paths": [self.dir("macros")],
            "seed-paths": [self.dir("seeds")],
            "test-paths": [self.dir("tests")],
            "seeds": {
                "quote_columns": False,
            },
        }

    def run_dbt_ls(self, args=None, expect_pass=True):
        log_manager.stdout_console()
        full_args = ["ls"]
        if args is not None:
            full_args += args

        result = run_dbt(args=full_args, expect_pass=expect_pass)

        log_manager.stdout_console()
        return result

    def assert_json_equal(self, json_str, expected):
        assert json.loads(json_str) == expected

    def expect_given_output(self, args, expectations):
        for key, values in expectations.items():
            ls_result = self.run_dbt_ls(args + ["--output", key])
            if not isinstance(values, (list, tuple)):
                values = [values]
            assert len(ls_result) == len(values)
            for got, expected in zip(ls_result, values):
                if key == "json":
                    self.assert_json_equal(got, expected)
                else:
                    assert got == expected

    def expect_snapshot_output(self, project):
        expectations = {
            "name": "my_snapshot",
            "selector": "test.snapshot.my_snapshot",
            "json": {
                "name": "my_snapshot",
                "package_name": "test",
                "depends_on": {"nodes": [], "macros": [], "public_nodes": []},
                "tags": [],
                "config": {
                    "enabled": True,
                    "group": None,
                    "materialized": "snapshot",
                    "post-hook": [],
                    "tags": [],
                    "pre-hook": [],
                    "quoting": {},
                    "column_types": {},
                    "persist_docs": {},
                    "target_database": project.database,
                    "target_schema": project.test_schema,
                    "unique_key": "id",
                    "strategy": "timestamp",
                    "updated_at": "updated_at",
                    "full_refresh": None,
                    "database": None,
                    "schema": None,
                    "alias": None,
                    "check_cols": None,
                    "on_schema_change": "ignore",
                    "on_configuration_change": "apply",
                    "meta": {},
                    "grants": {},
                    "packages": [],
                    "incremental_strategy": None,
                    "docs": {"node_color": None, "show": True},
                    "contract": {"enforced": False},
                },
                "unique_id": "snapshot.test.my_snapshot",
                "original_file_path": normalize("snapshots/snapshot.sql"),
                "alias": "my_snapshot",
                "resource_type": "snapshot",
            },
            "path": self.dir("snapshots/snapshot.sql"),
        }
        self.expect_given_output(["--resource-type", "snapshot"], expectations)

    def expect_analyses_output(self):
        expectations = {
            "name": "a",
            "selector": "test.analysis.a",
            "json": {
                "name": "a",
                "package_name": "test",
                "depends_on": {"nodes": [], "macros": [], "public_nodes": []},
                "tags": [],
                "config": {
                    "enabled": True,
                    "group": None,
                    "materialized": "view",
                    "post-hook": [],
                    "tags": [],
                    "pre-hook": [],
                    "quoting": {},
                    "column_types": {},
                    "persist_docs": {},
                    "full_refresh": None,
                    "on_schema_change": "ignore",
                    "on_configuration_change": "apply",
                    "database": None,
                    "schema": None,
                    "alias": None,
                    "meta": {},
                    "unique_key": None,
                    "grants": {},
                    "packages": [],
                    "incremental_strategy": None,
                    "docs": {"node_color": None, "show": True},
                    "contract": {"enforced": False},
                },
                "unique_id": "analysis.test.a",
                "original_file_path": normalize("analyses/a.sql"),
                "alias": "a",
                "resource_type": "analysis",
            },
            "path": self.dir("analyses/a.sql"),
        }
        self.expect_given_output(["--resource-type", "analysis"], expectations)

    def expect_model_output(self):
        expectations = {
            "name": ("ephemeral", "incremental", "inner", "outer"),
            "selector": ("test.ephemeral", "test.incremental", "test.sub.inner", "test.outer"),
            "json": (
                {
                    "name": "ephemeral",
                    "package_name": "test",
                    "depends_on": {"nodes": [], "macros": [], "public_nodes": []},
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "ephemeral",
                        "post-hook": [],
                        "tags": [],
                        "pre-hook": [],
                        "quoting": {},
                        "column_types": {},
                        "persist_docs": {},
                        "full_refresh": None,
                        "unique_key": None,
                        "on_schema_change": "ignore",
                        "on_configuration_change": "apply",
                        "database": None,
                        "schema": None,
                        "alias": None,
                        "meta": {},
                        "grants": {},
                        "packages": [],
                        "incremental_strategy": None,
                        "docs": {"node_color": None, "show": True},
                        "contract": {"enforced": False},
                    },
                    "original_file_path": normalize("models/ephemeral.sql"),
                    "unique_id": "model.test.ephemeral",
                    "alias": "ephemeral",
                    "resource_type": "model",
                },
                {
                    "name": "incremental",
                    "package_name": "test",
                    "depends_on": {
                        "nodes": ["seed.test.seed"],
                        "macros": ["macro.dbt.is_incremental"],
                        "public_nodes": [],
                    },
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "incremental",
                        "post-hook": [],
                        "tags": [],
                        "pre-hook": [],
                        "quoting": {},
                        "column_types": {},
                        "persist_docs": {},
                        "full_refresh": None,
                        "unique_key": None,
                        "on_schema_change": "ignore",
                        "on_configuration_change": "apply",
                        "database": None,
                        "schema": None,
                        "alias": None,
                        "meta": {},
                        "grants": {},
                        "packages": [],
                        "incremental_strategy": "delete+insert",
                        "docs": {"node_color": None, "show": True},
                        "contract": {"enforced": False},
                    },
                    "original_file_path": normalize("models/incremental.sql"),
                    "unique_id": "model.test.incremental",
                    "alias": "incremental",
                    "resource_type": "model",
                },
                {
                    "name": "inner",
                    "package_name": "test",
                    "depends_on": {
                        "nodes": ["model.test.outer"],
                        "macros": [],
                        "public_nodes": [],
                    },
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "view",
                        "post-hook": [],
                        "tags": [],
                        "pre-hook": [],
                        "quoting": {},
                        "column_types": {},
                        "persist_docs": {},
                        "full_refresh": None,
                        "unique_key": None,
                        "on_schema_change": "ignore",
                        "on_configuration_change": "apply",
                        "database": None,
                        "schema": None,
                        "alias": None,
                        "meta": {},
                        "grants": {},
                        "packages": [],
                        "incremental_strategy": None,
                        "docs": {"node_color": None, "show": True},
                        "contract": {"enforced": False},
                    },
                    "original_file_path": normalize("models/sub/inner.sql"),
                    "unique_id": "model.test.inner",
                    "alias": "inner",
                    "resource_type": "model",
                },
                {
                    "name": "outer",
                    "package_name": "test",
                    "depends_on": {
                        "nodes": ["model.test.ephemeral"],
                        "macros": [],
                        "public_nodes": [],
                    },
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "view",
                        "post-hook": [],
                        "tags": [],
                        "pre-hook": [],
                        "quoting": {},
                        "column_types": {},
                        "persist_docs": {},
                        "full_refresh": None,
                        "unique_key": None,
                        "on_schema_change": "ignore",
                        "on_configuration_change": "apply",
                        "database": None,
                        "schema": None,
                        "alias": None,
                        "meta": {},
                        "grants": {},
                        "packages": [],
                        "incremental_strategy": None,
                        "docs": {"node_color": None, "show": True},
                        "contract": {"enforced": False},
                    },
                    "original_file_path": normalize("models/outer.sql"),
                    "unique_id": "model.test.outer",
                    "alias": "outer",
                    "resource_type": "model",
                },
            ),
            "path": (
                self.dir("models/ephemeral.sql"),
                self.dir("models/incremental.sql"),
                self.dir("models/sub/inner.sql"),
                self.dir("models/outer.sql"),
            ),
        }
        self.expect_given_output(["--resource-type", "model"], expectations)

    # Do not include ephemeral model - it was not selected
    def expect_model_ephemeral_output(self):
        expectations = {
            "name": ("outer"),
            "selector": ("test.outer"),
            "json": (
                {
                    "name": "outer",
                    "package_name": "test",
                    "depends_on": {"nodes": [], "macros": [], "public_nodes": []},
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "materialized": "view",
                        "post-hook": [],
                        "tags": [],
                        "pre-hook": [],
                        "quoting": {},
                        "column_types": {},
                        "persist_docs": {},
                        "full_refresh": None,
                        "on_schema_change": "ignore",
                        "on_configuration_change": "apply",
                        "database": None,
                        "schema": None,
                        "alias": None,
                        "meta": {},
                        "grants": {},
                        "packages": [],
                        "incremental_strategy": None,
                        "docs": {"node_color": None, "show": True},
                    },
                    "unique_id": "model.test.ephemeral",
                    "original_file_path": normalize("models/ephemeral.sql"),
                    "alias": "outer",
                    "resource_type": "model",
                },
            ),
            "path": (self.dir("models/outer.sql"),),
        }
        self.expect_given_output(["--model", "outer"], expectations)

    def expect_source_output(self):
        expectations = {
            "name": "my_source.my_table",
            "selector": "source:test.my_source.my_table",
            "json": {
                "config": {
                    "enabled": True,
                },
                "unique_id": "source.test.my_source.my_table",
                "original_file_path": normalize("models/schema.yml"),
                "package_name": "test",
                "name": "my_table",
                "source_name": "my_source",
                "resource_type": "source",
                "tags": [],
            },
            "path": self.dir("models/schema.yml"),
        }
        # should we do this --select automatically for a user if if 'source' is
        # in the resource types and there is no '--select' or '--exclude'?
        self.expect_given_output(
            ["--resource-type", "source", "--select", "source:*"], expectations
        )

    def expect_seed_output(self):
        expectations = {
            "name": "seed",
            "selector": "test.seed",
            "json": {
                "name": "seed",
                "package_name": "test",
                "tags": [],
                "config": {
                    "enabled": True,
                    "group": None,
                    "materialized": "seed",
                    "post-hook": [],
                    "tags": [],
                    "pre-hook": [],
                    "quoting": {},
                    "column_types": {},
                    "persist_docs": {},
                    "quote_columns": False,
                    "full_refresh": None,
                    "unique_key": None,
                    "on_schema_change": "ignore",
                    "on_configuration_change": "apply",
                    "database": None,
                    "schema": None,
                    "alias": None,
                    "meta": {},
                    "grants": {},
                    "packages": [],
                    "incremental_strategy": None,
                    "docs": {"node_color": None, "show": True},
                    "contract": {"enforced": False},
                },
                "depends_on": {"macros": []},
                "unique_id": "seed.test.seed",
                "original_file_path": normalize("seeds/seed.csv"),
                "alias": "seed",
                "resource_type": "seed",
            },
            "path": self.dir("seeds/seed.csv"),
        }
        self.expect_given_output(["--resource-type", "seed"], expectations)

    def expect_test_output(self):
        expectations = {
            "name": ("not_null_outer_id", "t", "unique_outer_id"),
            "selector": ("test.not_null_outer_id", "test.t", "test.unique_outer_id"),
            "json": (
                {
                    "name": "not_null_outer_id",
                    "package_name": "test",
                    "depends_on": {
                        "nodes": ["model.test.outer"],
                        "macros": ["macro.dbt.test_not_null"],
                        "public_nodes": [],
                    },
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "test",
                        "severity": "ERROR",
                        "store_failures": None,
                        "warn_if": "!= 0",
                        "error_if": "!= 0",
                        "fail_calc": "count(*)",
                        "where": None,
                        "limit": None,
                        "tags": [],
                        "database": None,
                        "schema": "dbt_test__audit",
                        "alias": None,
                        "meta": {},
                    },
                    "unique_id": "test.test.not_null_outer_id.a226f4fb36",
                    "original_file_path": normalize("models/schema.yml"),
                    "alias": "not_null_outer_id",
                    "resource_type": "test",
                },
                {
                    "name": "t",
                    "package_name": "test",
                    "depends_on": {"nodes": [], "macros": [], "public_nodes": []},
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "test",
                        "severity": "ERROR",
                        "store_failures": None,
                        "warn_if": "!= 0",
                        "error_if": "!= 0",
                        "fail_calc": "count(*)",
                        "where": None,
                        "limit": None,
                        "tags": [],
                        "database": None,
                        "schema": "dbt_test__audit",
                        "alias": None,
                        "meta": {},
                    },
                    "unique_id": "test.test.t",
                    "original_file_path": normalize("tests/t.sql"),
                    "alias": "t",
                    "resource_type": "test",
                },
                {
                    "name": "unique_outer_id",
                    "package_name": "test",
                    "depends_on": {
                        "nodes": ["model.test.outer"],
                        "macros": ["macro.dbt.test_unique"],
                        "public_nodes": [],
                    },
                    "tags": [],
                    "config": {
                        "enabled": True,
                        "group": None,
                        "materialized": "test",
                        "severity": "ERROR",
                        "store_failures": None,
                        "warn_if": "!= 0",
                        "error_if": "!= 0",
                        "fail_calc": "count(*)",
                        "where": None,
                        "limit": None,
                        "tags": [],
                        "database": None,
                        "schema": "dbt_test__audit",
                        "alias": None,
                        "meta": {},
                    },
                    "unique_id": "test.test.unique_outer_id.2195e332d3",
                    "original_file_path": normalize("models/schema.yml"),
                    "alias": "unique_outer_id",
                    "resource_type": "test",
                },
            ),
            "path": (
                self.dir("models/schema.yml"),
                self.dir("tests/t.sql"),
                self.dir("models/schema.yml"),
            ),
        }
        self.expect_given_output(["--resource-type", "test"], expectations)

    def expect_all_output(self):
        # generic test FQNS include the resource + column they're defined on
        # models are just package, subdirectory path, name
        # sources are like models, ending in source_name.table_name
        expected_default = {
            "test.ephemeral",
            "test.incremental",
            "test.snapshot.my_snapshot",
            "test.sub.inner",
            "test.outer",
            "test.seed",
            "source:test.my_source.my_table",
            "test.not_null_outer_id",
            "test.unique_outer_id",
            "test.t",
        }
        # analyses have their type inserted into their fqn like tests
        expected_all = expected_default | {"test.analysis.a"}

        results = self.run_dbt_ls(["--resource-type", "all", "--select", "*", "source:*"])
        assert set(results) == expected_all

        results = self.run_dbt_ls(["--select", "*", "source:*"])
        assert set(results) == expected_default

        results = self.run_dbt_ls(["--resource-type", "default", "--select", "*", "source:*"])
        assert set(results) == expected_default

        results = self.run_dbt_ls

    def expect_select(self):
        results = self.run_dbt_ls(["--resource-type", "test", "--select", "outer"])
        assert set(results) == {"test.not_null_outer_id", "test.unique_outer_id"}

        self.run_dbt_ls(["--resource-type", "test", "--select", "inner"], expect_pass=True)

        results = self.run_dbt_ls(["--resource-type", "test", "--select", "+inner"])
        assert set(results) == {"test.not_null_outer_id", "test.unique_outer_id"}

        results = self.run_dbt_ls(["--resource-type", "model", "--select", "outer+"])
        assert set(results) == {"test.outer", "test.sub.inner"}

        results = self.run_dbt_ls(["--resource-type", "model", "--exclude", "inner"])
        assert set(results) == {"test.ephemeral", "test.outer", "test.incremental"}

        results = self.run_dbt_ls(["--select", "config.incremental_strategy:delete+insert"])
        assert set(results) == {"test.incremental"}

        self.run_dbt_ls(
            ["--select", "config.incremental_strategy:insert_overwrite"], expect_pass=True
        )

    def expect_resource_type_multiple(self):
        """Expect selected resources when --resource-type given multiple times"""
        results = self.run_dbt_ls(["--resource-type", "test", "--resource-type", "model"])
        assert set(results) == {
            "test.ephemeral",
            "test.incremental",
            "test.not_null_outer_id",
            "test.outer",
            "test.sub.inner",
            "test.t",
            "test.unique_outer_id",
        }

        results = self.run_dbt_ls(
            ["--resource-type", "test", "--resource-type", "model", "--exclude", "unique_outer_id"]
        )
        assert set(results) == {
            "test.ephemeral",
            "test.incremental",
            "test.not_null_outer_id",
            "test.outer",
            "test.sub.inner",
            "test.t",
        }

        results = self.run_dbt_ls(
            [
                "--resource-type",
                "test",
                "--resource-type",
                "model",
                "--select",
                "+inner",
                "outer+",
                "--exclude",
                "inner",
            ]
        )
        assert set(results) == {
            "test.ephemeral",
            "test.not_null_outer_id",
            "test.unique_outer_id",
            "test.outer",
        }

    def expect_selected_keys(self, project):
        """Expect selected fields of the the selected model"""
        expectations = [
            {"database": project.database, "schema": project.test_schema, "alias": "inner"}
        ]
        results = self.run_dbt_ls(
            [
                "--model",
                "inner",
                "--output",
                "json",
                "--output-keys",
                "database",
                "schema",
                "alias",
            ]
        )
        assert len(results) == len(expectations)

        for got, expected in zip(results, expectations):
            self.assert_json_equal(got, expected)

        """Expect selected fields when --output-keys given multiple times
        """
        expectations = [{"database": project.database, "schema": project.test_schema}]
        results = self.run_dbt_ls(
            [
                "--model",
                "inner",
                "--output",
                "json",
                "--output-keys",
                "database",
                "--output-keys",
                "schema",
            ]
        )
        assert len(results) == len(expectations)

        for got, expected in zip(results, expectations):
            self.assert_json_equal(got, expected)

        """Expect selected fields of the test resource types
        """
        expectations = [
            {"name": "not_null_outer_id", "column_name": "id"},
            {"name": "t"},
            {"name": "unique_outer_id", "column_name": "id"},
        ]
        results = self.run_dbt_ls(
            [
                "--resource-type",
                "test",
                "--output",
                "json",
                "--output-keys",
                "name",
                "column_name",
            ]
        )
        assert len(results) == len(expectations)

        for got, expected in zip(
            sorted(results, key=lambda x: json.loads(x).get("name")),
            sorted(expectations, key=lambda x: x.get("name")),
        ):
            self.assert_json_equal(got, expected)

        """Expect nothing (non-existent keys) for the selected models
        """
        expectations = [{}, {}]
        results = self.run_dbt_ls(
            [
                "--model",
                "inner outer",
                "--output",
                "json",
                "--output-keys",
                "non_existent_key",
            ]
        )
        assert len(results) == len(expectations)

        for got, expected in zip(results, expectations):
            self.assert_json_equal(got, expected)

    def test_ls(self, project):
        self.expect_snapshot_output(project)
        self.expect_analyses_output()
        self.expect_model_output()
        self.expect_source_output()
        self.expect_seed_output()
        self.expect_test_output()
        self.expect_select()
        self.expect_resource_type_multiple()
        self.expect_all_output()
        self.expect_selected_keys(project)


def normalize(path):
    """On windows, neither is enough on its own:
    >>> normcase('C:\\documents/ALL CAPS/subdir\\..')
    'c:\\documents\\all caps\\subdir\\..'
    >>> normpath('C:\\documents/ALL CAPS/subdir\\..')
    'C:\\documents\\ALL CAPS'
    >>> normpath(normcase('C:\\documents/ALL CAPS/subdir\\..'))
    'c:\\documents\\all caps'
    """
    return os.path.normcase(os.path.normpath(path))
