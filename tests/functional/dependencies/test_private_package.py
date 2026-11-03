import json
import os

import pytest

from dbt.tests.util import run_dbt_and_capture
from tests.functional.utils import installed_package_dirs


class TestSimpleDependencyWithDuplicates(object):
    # dbt should convert these into a single dependency internally
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "private": "dbt-labs/dbt-integration-project",
                    "revision": "dbt/1.0.0",
                },
                {
                    "private": "dbt-labs/dbt-integration-project",
                    "revision": "dbt/1.0.0",
                },
            ]
        }

    @pytest.fixture(scope="class")
    def private_package_env(self):
        # This is a public repo so any token will work.
        # This is mostly to test that the token is not exposed in the logs
        # and the whole private package install works
        os.environ["DBT_ENV_PRIVATE_GIT_PROVIDER_INFO"] = json.dumps(
            [
                {
                    "org": "dbt-labs",
                    "url": "https://{token}@github.com/dbt-labs/{repo}.git",
                    "token": "a_token",
                    "provider": "github",
                }
            ]
        )
        yield
        del os.environ["DBT_ENV_PRIVATE_GIT_PROVIDER_INFO"]

    def test_private_deps(self, project, private_package_env, logs_dir):
        _, stdout = run_dbt_and_capture(["deps"])

        # assert a package being downloaded
        assert len(installed_package_dirs()) == 1

        # package-lock.yml contain private package and deduped
        with open("package-lock.yml") as fp:
            lock_file_content = fp.read()
        assert lock_file_content == """packages:
  - name: dbt_integration_project
    private: dbt-labs/dbt-integration-project
    revision: cb15ad066afa3cb5d75f82062e35d230da086371
sha1_hash: bdd5bb1fe19294d5f5c14d347064da22bfa34657
"""

        # check that the token is not in the output
        assert "a_token" not in stdout
        with open(f"{logs_dir}/dbt.log") as fp:
            logs = fp.read()
        assert "a_token" not in logs
