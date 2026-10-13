import os

import pytest

from dbt.constants import DEFAULT_ENV_PLACEHOLDER
from dbt.tests.util import get_manifest, run_dbt, run_dbt_and_capture
from dbt_common.constants import SECRET_ENV_PREFIX

context_sql = """

{{
    config(
        materialized='table'
    )
}}

select

    -- compile-time variables
    '{{ this }}'        as "this",
    '{{ this.name }}'   as "this.name",
    '{{ this.schema }}' as "this.schema",
    '{{ this.table }}'  as "this.table",

    '{{ target.dbname }}'  as "target.dbname",
    '{{ target.host }}'    as "target.host",
    '{{ target.name }}'    as "target.name",
    '{{ target.schema }}'  as "target.schema",
    '{{ target.type }}'    as "target.type",
    '{{ target.user }}'    as "target.user",
    '{{ target.get("pass", "") }}'    as "target.pass", -- not actually included, here to test that it is _not_ present!
    {{ target.port }}      as "target.port",
    {{ target.threads }}   as "target.threads",

    -- runtime variables
    '{{ run_started_at }}' as run_started_at,
    '{{ invocation_id }}'  as invocation_id,
    '{{ thread_id }}'  as thread_id,

    '{{ env_var("DBT_TEST_ENV_VAR") }}' as env_var,
    '{{ env_var("DBT_TEST_IGNORE_DEFAULT", "ignored_default_val") }}' as env_var_ignore_default,
    '{{ env_var("DBT_TEST_USE_DEFAULT", "use_my_default_val") }}' as env_var_use_default,
    'secret_variable' as env_var_secret, -- make sure the value itself is scrubbed from the logs
    '{{ env_var("DBT_TEST_NOT_SECRET") }}' as env_var_not_secret

"""


class TestEnvVars:
    @pytest.fixture(scope="class")
    def models(self):
        return {"context.sql": context_sql}

    @pytest.fixture(scope="class", autouse=True)
    def setup(self):
        os.environ["DBT_TEST_ENV_VAR"] = "1"
        os.environ["DBT_TEST_USER"] = "root"
        os.environ["DBT_TEST_PASS"] = "password"
        os.environ[SECRET_ENV_PREFIX + "_SECRET"] = "secret_variable"
        os.environ["DBT_TEST_NOT_SECRET"] = "regular_variable"
        os.environ["DBT_TEST_IGNORE_DEFAULT"] = "ignored_default"
        yield
        del os.environ["DBT_TEST_ENV_VAR"]
        del os.environ["DBT_TEST_USER"]
        del os.environ[SECRET_ENV_PREFIX + "_SECRET"]
        del os.environ["DBT_TEST_NOT_SECRET"]
        del os.environ["DBT_TEST_IGNORE_DEFAULT"]

    @pytest.fixture(scope="class")
    def profiles_config_update(self, unique_schema):
        return {
            "test": {
                "outputs": {
                    # don't use env_var's here so the integration tests can run
                    # seed sql statements and the like. default target is used
                    "dev": {
                        "type": "postgres",
                        "threads": 1,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "user": "root",
                        "pass": "password",
                        "dbname": "dbt",
                        "schema": unique_schema,
                    },
                    "prod": {
                        "type": "postgres",
                        "threads": 1,
                        "host": "localhost",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        # root/password
                        "user": "{{ env_var('DBT_TEST_USER') }}",
                        "pass": "{{ env_var('DBT_TEST_PASS') }}",
                        "dbname": "dbt",
                        "schema": unique_schema,
                    },
                },
                "target": "dev",
            }
        }

    def get_ctx_vars(self, project):
        fields = [
            "this",
            "this.name",
            "this.schema",
            "this.table",
            "target.dbname",
            "target.host",
            "target.name",
            "target.port",
            "target.schema",
            "target.threads",
            "target.type",
            "target.user",
            "target.pass",
            "run_started_at",
            "invocation_id",
            "thread_id",
            "env_var",
        ]
        field_list = ", ".join(['"{}"'.format(f) for f in fields])
        query = "select {field_list} from {schema}.context".format(
            field_list=field_list, schema=project.test_schema
        )
        vals = project.run_sql(query, fetch="all")
        ctx = dict([(k, v) for (k, v) in zip(fields, vals[0])])
        return ctx

    def test_env_vars_dev(
        self,
        project,
    ):
        results = run_dbt(["run"])
        assert len(results) == 1
        ctx = self.get_ctx_vars(project)

        manifest = get_manifest(project.project_root)
        expected = {
            "DBT_TEST_ENV_VAR": "1",
            "DBT_TEST_NOT_SECRET": "regular_variable",
            "DBT_TEST_IGNORE_DEFAULT": "ignored_default",
            "DBT_TEST_USE_DEFAULT": DEFAULT_ENV_PLACEHOLDER,
        }
        assert manifest.env_vars == expected

        this = '"{}"."{}"."context"'.format(project.database, project.test_schema)
        assert ctx["this"] == this

        assert ctx["this.name"] == "context"
        assert ctx["this.schema"] == project.test_schema
        assert ctx["this.table"] == "context"

        assert ctx["target.dbname"] == "dbt"
        assert ctx["target.host"] == "localhost"
        assert ctx["target.name"] == "dev"
        assert ctx["target.port"] == int(os.getenv("POSTGRES_TEST_PORT", 5432))
        assert ctx["target.schema"] == project.test_schema
        assert ctx["target.threads"] == 1
        assert ctx["target.type"] == "postgres"
        assert ctx["target.user"] == "root"
        assert ctx["target.pass"] == ""

        assert ctx["env_var"] == "1"

    def test_env_vars_prod(self, project):
        results = run_dbt(["run", "--target", "prod"])
        assert len(results) == 1
        ctx = self.get_ctx_vars(project)

        this = '"{}"."{}"."context"'.format(project.database, project.test_schema)
        assert ctx["this"] == this

        assert ctx["this.name"] == "context"
        assert ctx["this.schema"] == project.test_schema
        assert ctx["this.table"] == "context"

        assert ctx["target.dbname"] == "dbt"
        assert ctx["target.host"] == "localhost"
        assert ctx["target.name"] == "prod"
        assert ctx["target.port"] == int(os.getenv("POSTGRES_TEST_PORT", 5432))
        assert ctx["target.schema"] == project.test_schema
        assert ctx["target.threads"] == 1
        assert ctx["target.type"] == "postgres"
        assert ctx["target.user"] == "root"
        assert ctx["target.pass"] == ""
        assert ctx["env_var"] == "1"

    def test_env_vars_secrets(self, project):
        os.environ["DBT_DEBUG"] = "True"
        _, log_output = run_dbt_and_capture(["run", "--target", "prod"])

        assert not ("secret_variable" in log_output)
        assert "regular_variable" in log_output
        del os.environ["DBT_DEBUG"]


class TestEnvVarCaseInsensitiveOnWindows:
    """On Windows, env var names are case-insensitive (GH-10422).

    env_var('dbt_test_env_var') should resolve an env var set as DBT_TEST_ENV_VAR.
    This test only validates the behavior on Windows where os.name == 'nt'.
    """

    @pytest.fixture(scope="class", autouse=True)
    def setup(self):
        os.environ["DBT_TEST_ENV_VAR"] = "1"
        yield
        del os.environ["DBT_TEST_ENV_VAR"]

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "model.sql": """select '{{ env_var("dbt_test_env_var") }}' as val""",
        }

    @pytest.mark.skipif(
        os.name != "nt",
        reason="Environment variables are only case-insensitive on Windows",
    )
    def test_env_var_case_insensitive_parse(self, project):
        run_dbt(["parse"])


class TestEnvVarInCreateSchema:
    """Test that the env_var() method works in overrides of the create_schema
    macro, which is called during a different phase of execution than most
    macros, causing problems."""

    @pytest.fixture(scope="class", autouse=True)
    def setup(self):
        os.environ["DBT_TEST_ENV_VAR"] = "1"

    @pytest.fixture(scope="class")
    def macros(self):
        return {
            "macros.sql": """
                {% macro create_schema(relation) %}
                  {%- call statement('create_schema') -%}
                     SELECT {{ env_var('DBT_TEST_ENV_VAR') }} as TEST
                  {% endcall %}
                {% endmacro %}%
            """
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "mymodel.sql": """
            SELECT 1 as TEST -- {%- do adapter.create_schema(this) -%}
        """
        }

    def test_env_var_in_create_schema(self, project):
        run_dbt(["run"])
