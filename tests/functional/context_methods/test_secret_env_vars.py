import os

import pytest

from dbt.exceptions import DbtInternalError, ParsingError
from dbt.tests.util import read_file, run_dbt, run_dbt_and_capture
from dbt_common.constants import SECRET_ENV_PREFIX
from tests.functional.context_methods.first_dependency import FirstDependencyProject

secret_bad__context_sql = """

{{
    config(
        materialized='table'
    )
}}

select

    '{{ env_var("DBT_TEST_ENV_VAR") }}' as env_var,
    '{{ env_var("DBT_ENV_SECRET_SECRET") }}' as env_var_secret, -- this should raise an error!
    '{{ env_var("DBT_TEST_NOT_SECRET") }}' as env_var_not_secret

"""


class TestDisallowSecretModel:
    @pytest.fixture(scope="class")
    def models(self):
        return {"context.sql": secret_bad__context_sql}

    def test_disallow_secret(self, project):
        with pytest.raises(ParsingError):
            run_dbt(["compile"])


models__context_sql = """
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
    'secret_variable' as env_var_secret, -- make sure the value itself is scrubbed from the logs
    '{{ env_var("DBT_TEST_NOT_SECRET") }}' as env_var_not_secret
"""


class TestAllowSecretProfilePackage(FirstDependencyProject):
    @pytest.fixture(scope="class", autouse=True)
    def setup(self):
        os.environ[SECRET_ENV_PREFIX + "_USER"] = "root"
        os.environ[SECRET_ENV_PREFIX + "_PASS"] = "password"
        os.environ[SECRET_ENV_PREFIX + "_PACKAGE"] = "first_dependency"
        os.environ[SECRET_ENV_PREFIX + "_GIT_TOKEN"] = "abc123"
        yield
        del os.environ[SECRET_ENV_PREFIX + "_USER"]
        del os.environ[SECRET_ENV_PREFIX + "_PASS"]
        del os.environ[SECRET_ENV_PREFIX + "_PACKAGE"]
        del os.environ[SECRET_ENV_PREFIX + "_GIT_TOKEN"]

    @pytest.fixture(scope="class")
    def models(self):
        return {"context.sql": models__context_sql}

    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    # the raw value of this secret *will* be written to lock file
                    "local": "{{ env_var('DBT_ENV_SECRET_PACKAGE') }}"
                },
                {
                    # this secret env var will *not* be written to lock file
                    "git": "https://{{ env_var('DBT_ENV_SECRET_GIT_TOKEN') }}@github.com/dbt-labs/dbt-external-tables.git"
                },
                {
                    # this secret env var will *not* be written to lock file
                    "tarball": "https://{{ env_var('DBT_ENV_SECRET_GIT_TOKEN') }}@github.com/dbt-labs/dbt-utils/archive/refs/tags/1.1.1.tar.gz",
                    "name": "dbt_utils",
                },
            ]
        }

    @pytest.fixture(scope="class")
    def profile_target(self):
        return {
            "type": "postgres",
            "threads": 1,
            "host": "localhost",
            "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
            # root/password
            "user": "{{ env_var('DBT_ENV_SECRET_USER') }}",
            "pass": "{{ env_var('DBT_ENV_SECRET_PASS') }}",
            "dbname": "dbt",
        }

    def test_allow_secrets(self, project, first_dependency):
        _, log_output = run_dbt_and_capture(["deps"])
        lock_file_contents = read_file("package-lock.yml")

        # this will not be written to logs or lock file
        assert not ("abc123" in log_output)
        assert not ("abc123" in lock_file_contents)
        assert "{{ env_var('DBT_ENV_SECRET_GIT_TOKEN') }}" in lock_file_contents

        # this will be scrubbed from logs, but not from the lock file
        assert not ("first_dependency" in log_output)
        assert "first_dependency" in lock_file_contents


class TestCloneFailSecretScrubbed:
    @pytest.fixture(scope="class", autouse=True)
    def setup(self):
        os.environ[SECRET_ENV_PREFIX + "_GIT_TOKEN"] = "abc123"

    @pytest.fixture(scope="class")
    def models(self):
        return {"context.sql": models__context_sql}

    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://fakeuser:{{ env_var('DBT_ENV_SECRET_GIT_TOKEN') }}@github.com/dbt-labs/fake-repo.git"
                },
            ]
        }

    def test_fail_clone_with_scrubbing(self, project):
        with pytest.raises(DbtInternalError) as excinfo:
            _, log_output = run_dbt_and_capture(["deps"])

        assert "abc123" not in str(excinfo.value)


class TestCloneFailSecretNotRendered(TestCloneFailSecretScrubbed):
    # as above, with some Jinja manipulation
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://fakeuser:{{ env_var('DBT_ENV_SECRET_GIT_TOKEN') | join(' ') }}@github.com/dbt-labs/fake-repo.git"
                },
            ]
        }

    def test_fail_clone_with_scrubbing(self, project):
        with pytest.raises(DbtInternalError) as excinfo:
            _, log_output = run_dbt_and_capture(["deps"])

        # we should not see any manipulated form of the secret value (abc123) here
        # we should see a manipulated form of the placeholder instead
        assert "a b c 1 2 3" not in str(excinfo.value)
        assert "D B T _ E N V _ S E C R E T _ G I T _ T O K E N" in str(excinfo.value)
