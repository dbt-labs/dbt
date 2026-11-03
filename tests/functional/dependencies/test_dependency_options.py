import os
import shutil

import pytest

from dbt.tests.util import run_dbt
from tests.functional.utils import installed_package_dirs


class TestDepsOptions(object):
    # this revision of dbt-integration-project requires dbt-utils.git@0.5.0, which the
    # package config handling should detect
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "package": "fivetran/fivetran_utils",
                    "version": "0.4.7",
                },
            ]
        }

    @pytest.fixture
    def clean_start(self, project):
        if os.path.exists("dbt_packages"):
            shutil.rmtree("dbt_packages")
        if os.path.exists("package-lock.yml"):
            os.remove("package-lock.yml")

    def test_deps_lock(self, clean_start):
        run_dbt(["deps", "--lock"])
        assert not os.path.exists("dbt_packages")
        assert os.path.exists("package-lock.yml")
        with open("package-lock.yml") as fp:
            contents = fp.read()

        fivetran_package = """
  - name: fivetran_utils
    package: fivetran/fivetran_utils
    version: 0.4.7
"""

        # dbt-utils is a dep in fivetran so we can't check for a specific version or this test fails everytime a new dbt-utils version comes out
        dbt_labs_package = """
  - name: dbt_utils
    package: dbt-labs/dbt_utils
"""
        package_sha = "sha1_hash: 71304bca2138cf8004070b3573a1e17183c0c1a8"
        assert fivetran_package in contents
        assert dbt_labs_package in contents
        assert package_sha in contents

    def test_deps_default(self, clean_start):
        run_dbt(["deps"])
        assert len(installed_package_dirs()) == 2
        assert os.path.exists("package-lock.yml")
        with open("package-lock.yml") as fp:
            contents = fp.read()
        fivetran_package = """
  - name: fivetran_utils
    package: fivetran/fivetran_utils
    version: 0.4.7
"""

        # dbt-utils is a dep in fivetran so we can't check for a specific version or this test fails everytime a new dbt-utils version comes out
        dbt_labs_package = """
  - name: dbt_utils
    package: dbt-labs/dbt_utils
"""
        package_sha = "sha1_hash: 71304bca2138cf8004070b3573a1e17183c0c1a8"
        assert fivetran_package in contents
        assert dbt_labs_package in contents
        assert package_sha in contents

    def test_deps_add(self, clean_start):
        run_dbt(["deps", "--add-package", "dbt-labs/audit_helper@0.9.0"])
        with open("packages.yml") as fp:
            contents = fp.read()
        assert contents == """packages:
  - package: fivetran/fivetran_utils
    version: 0.4.7
  - package: dbt-labs/audit_helper
    version: 0.9.0
"""
        assert len(installed_package_dirs()) == 3

    def test_deps_add_without_install(self, clean_start):
        os.rename("packages.yml", "dependencies.yml")
        run_dbt(
            [
                "deps",
                "--add-package",
                "dbt-labs/audit_helper@0.9.0",
                "--lock",
            ]
        )
        assert not os.path.exists("dbt_packages")
        assert not os.path.exists("packages.yml")
        with open("dependencies.yml") as fp:
            contents = fp.read()
        assert contents == """packages:
  - package: fivetran/fivetran_utils
    version: 0.4.7
  - package: dbt-labs/audit_helper
    version: 0.9.0
"""

    def test_deps_upgrade(self, clean_start, mocker):
        run_dbt(["deps", "--lock"])
        patched_lock = mocker.patch("dbt.task.deps.DepsTask.lock")
        run_dbt(["deps", "--upgrade"])
        assert patched_lock.call_count == 1
