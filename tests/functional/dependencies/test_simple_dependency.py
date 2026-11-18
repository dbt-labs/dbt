import os
import tempfile
from pathlib import Path

import pytest

from dbt.exceptions import DbtProjectError
from dbt.tests.util import check_relations_equal, run_dbt, write_config_file

models__disabled_one = """
{{config(enabled=False)}}

select 1
"""

models__disabled_two = """
{{config(enabled=False)}}

select * from {{ref('disabled_one')}}
"""

models__empty = """
"""

models__view_summary = """
{{
    config(
        materialized='view'
    )
}}


with t as (

    select * from {{ ref('view_model') }}

)

select date_trunc('year', updated_at) as year,
       count(*)
from t
group by 1
"""


class SimpleDependencyBase(object):
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project):
        project.run_sql_file(project.test_data_dir / Path("seed.sql"))

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "empty.sql": models__empty,
            "view_summary.sql": models__view_summary,
            "view_summary.sql": models__view_summary,
        }

    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project",
                    "revision": "1.1",
                }
            ]
        }

    # These two functions included to enable override in ...NoProfile derived test class
    @pytest.fixture(scope="class")
    def run_deps(self, project):
        return run_dbt(["deps"])

    @pytest.fixture(scope="function")
    def run_clean(self, project):
        yield

        # clear test schema
        assert os.path.exists("target")
        run_dbt(["clean"])
        assert not os.path.exists("target")


class TestSimpleDependency(SimpleDependencyBase):
    def test_simple_dependency(self, run_deps, project, run_clean):
        """dependencies should draw from a changing base table"""
        results = run_dbt()
        assert len(results) == 4

        check_relations_equal(project.adapter, ["seed", "table_model"])
        check_relations_equal(project.adapter, ["seed", "view_model"])
        check_relations_equal(project.adapter, ["seed", "incremental"])
        check_relations_equal(project.adapter, ["seed_summary", "view_summary"])

        project.run_sql_file(project.test_data_dir / Path("update.sql"))
        results = run_dbt()
        assert len(results) == 4

        check_relations_equal(project.adapter, ["seed", "table_model"])
        check_relations_equal(project.adapter, ["seed", "view_model"])
        check_relations_equal(project.adapter, ["seed", "incremental"])


class TestSimpleDependencyWithDependenciesFile(SimpleDependencyBase):
    @pytest.fixture(scope="class")
    def packages(self):
        return {}

    @pytest.fixture(scope="class")
    def dependencies(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project",
                    "warn-unpinned": True,
                }
            ]
        }

    def test_dependency_with_dependencies_file(self, run_deps, project):
        # Tests that "packages" defined in a dependencies.yml file works
        run_dbt(["deps"])
        results = run_dbt()
        assert len(results) == 4


class TestSimpleDependencyWithEmptyPackagesFile(SimpleDependencyBase):
    @pytest.fixture(scope="class")
    def packages(self):
        return " "

    def test_dependency_with_empty_packages_file(self, run_deps, project):
        # Tests that an empty packages file doesn't fail with a Python error
        run_dbt(["deps"])


class TestSimpleDependencyNoProfile(SimpleDependencyBase):
    """dbt deps and clean commands should not require a profile."""

    @pytest.fixture(scope="class")
    def run_deps(self, project):
        with tempfile.TemporaryDirectory() as tmpdir:
            result = run_dbt(["deps", "--profiles-dir", tmpdir])
        return result

    @pytest.fixture(scope="class")
    def run_clean(self, project):
        with tempfile.TemporaryDirectory() as tmpdir:
            result = run_dbt(["clean", "--profiles-dir", tmpdir])
        return result

    def test_simple_dependency_no_profile(self, project, run_deps, run_clean):
        """only need fixtures as opposed to any model assertions since those are
        irrelevant and won't occur within the same runtime as a dbt run -s ..."""
        pass


class TestSimpleDependencyWithModels(SimpleDependencyBase):
    def test_simple_dependency_with_models(self, run_deps, project, run_clean):
        results = run_dbt(["run", "--models", "view_model+"])
        len(results) == 2

        check_relations_equal(project.adapter, ["seed", "view_model"])
        check_relations_equal(project.adapter, ["seed_summary", "view_summary"])

        created_models = project.get_tables_in_schema()

        assert "table_model" not in created_models
        assert "incremental" not in created_models
        assert created_models["view_model"] == "view"
        assert created_models["view_summary"] == "view"


class TestSimpleDependencyUnpinned(object):
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project):
        project.run_sql_file(project.test_data_dir / Path("seed.sql"))

    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project",
                    "warn-unpinned": True,
                }
            ]
        }

    def test_simple_dependency(self, project):
        run_dbt(["deps"])


class TestSimpleDependencyWithDuplicates(object):
    # dbt should convert these into a single dependency internally
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project",
                    "revision": "dbt/1.0.0",
                },
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project.git",
                    "revision": "dbt/1.0.0",
                },
            ]
        }

    def test_simple_dependency_deps(self, project):
        run_dbt(["deps"])


class TestSimpleDependencyWithSubdirs(object):
    # dbt should convert these into a single dependency internally
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-multipe-packages.git",
                    "subdirectory": "dbt-utils-main",
                    "revision": "v0.1.0",
                },
                {
                    "git": "https://github.com/dbt-labs/dbt-multipe-packages.git",
                    "subdirectory": "dbt-date-main",
                    "revision": "v0.1.0",
                },
            ]
        }

    def test_git_with_multiple_subdir(self, project):
        run_dbt(["deps"])
        assert os.path.exists("package-lock.yml")
        expected = """packages:
  - git: https://github.com/dbt-labs/dbt-multipe-packages.git
    name: dbt_utils
    revision: 53782f3ede8fdf307ee1d8e418aa65733a4b72fa
    subdirectory: dbt-utils-main
  - git: https://github.com/dbt-labs/dbt-multipe-packages.git
    name: dbt_date
    revision: 53782f3ede8fdf307ee1d8e418aa65733a4b72fa
    subdirectory: dbt-date-main
sha1_hash: b9c8042f29446c55a33f9f211737f445a640c7a1
"""
        with open("package-lock.yml") as fp:
            contents = fp.read()
        assert contents == expected
        assert len(os.listdir("dbt_packages")) == 2


class TestRekeyedDependencyWithSubduplicates(object):
    # this revision of dbt-integration-project requires dbt-utils.git@0.5.0, which the
    # package config handling should detect
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project",
                    "revision": "config-1.0.0-deps",
                },
                {
                    "git": "https://github.com/dbt-labs/dbt-utils",
                    "revision": "0.5.0",
                },
            ]
        }

    def test_simple_dependency_deps(self, project):
        run_dbt(["deps"])
        assert len(os.listdir("dbt_packages")) == 2


class TestTarballNestedDependencies(object):
    # this version of dbt_expectations has a dependency on dbt_date, which the
    # package config handling should detect
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "tarball": "https://github.com/calogica/dbt-expectations/archive/refs/tags/0.9.0.tar.gz",
                    "name": "dbt_expectations",
                },
            ]
        }

    def test_simple_dependency_deps(self, project):
        run_dbt(["deps"])
        assert set(os.listdir("dbt_packages")) == set(["dbt_expectations", "dbt_date"])


class DependencyBranchBase(object):
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project):
        project.run_sql_file(project.test_data_dir / Path("seed.sql"))

    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "git": "https://github.com/dbt-labs/dbt-integration-project",
                    "revision": "dbt/1.0.0",
                },
            ]
        }

    def deps_run_assert_equality(self, project):
        run_dbt(["deps"])
        results = run_dbt()
        assert len(results) == 4

        check_relations_equal(project.adapter, ["seed", "table_model"])
        check_relations_equal(project.adapter, ["seed", "view_model"])
        check_relations_equal(project.adapter, ["seed", "incremental"])

        created_models = project.get_tables_in_schema()

        assert created_models["table_model"] == "table"
        assert created_models["view_model"] == "view"
        assert created_models["view_summary"] == "view"
        assert created_models["incremental"] == "table"


class TestSimpleDependencyBranch(DependencyBranchBase):
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "view_summary.sql": models__view_summary,
        }

    def test_simple_dependency(self, project):
        self.deps_run_assert_equality(project)
        check_relations_equal(project.adapter, ["seed_summary", "view_summary"])

        project.run_sql_file(project.test_data_dir / Path("update.sql"))
        self.deps_run_assert_equality(project)


class TestSimpleDependencyBranchWithEmpty(DependencyBranchBase):
    @pytest.fixture(scope="class")
    def models(self):
        """extra models included"""
        return {
            "disabled_one.sql": models__disabled_one,
            "disabled_two.sql": models__disabled_two,
            "view_summary.sql": models__view_summary,
            "empty.sql": models__empty,
        }

    def test_empty_models_not_compiled_in_dependencies(self, project):
        self.deps_run_assert_equality(project)

        models = project.get_tables_in_schema()

        assert "empty" not in models.keys()


class TestSimpleDependencyBadProfile(object):
    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "config-version": 2,
            "models": {
                "+any_config": "{{ target.name }}",
                "+enabled": "{{ target.name in ['redshift', 'postgres'] | as_bool }}",
            },
        }

    # Write out the profile data as a yaml file
    @pytest.fixture(scope="class", autouse=True)
    def dbt_profile_target(self):
        # Need to set the environment variable here initially because
        # the unittest setup does a load_config.
        os.environ["PROFILE_TEST_HOST"] = "localhost"
        return {
            "type": "postgres",
            "threads": 4,
            "host": "{{ env_var('PROFILE_TEST_HOST') }}",
            "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
            "user": "root",
            "pass": "password",
            "dbname": "dbt",
        }

    def test_deps_bad_profile(self, project):
        del os.environ["PROFILE_TEST_HOST"]
        run_dbt(["deps"])
        run_dbt(["clean"])


class TestSimpleDependcyTarball(object):
    @pytest.fixture(scope="class")
    def packages(self):
        return {
            "packages": [
                {
                    "tarball": "https://codeload.github.com/dbt-labs/dbt-utils/tar.gz/0.9.6",
                    "name": "dbt_utils",
                }
            ]
        }

    def test_deps_simple_tarball_doesnt_error_out(self, project):
        run_dbt(["deps"])
        assert len(os.listdir("dbt_packages")) == 1


class TestBadTarballDependency(object):
    def test_malformed_tarball_package_causes_exception(self, project):
        # We have to specify the bad formatted package here because if we do it
        # in a `packages` fixture, the test will blow up in the setup phase, meaning
        # we can't appropriately catch it with a `pytest.raises`
        bad_tarball_package_spec = {
            "packages": [
                {
                    "tarball": "https://codeload.github.com/dbt-labs/dbt-utils/tar.gz/0.9.6",
                    "version": "dbt_utils",
                }
            ]
        }
        write_config_file(bad_tarball_package_spec, "packages.yml")

        with pytest.raises(
            DbtProjectError, match=r"The packages.yml file in this project is malformed"
        ) as e:
            run_dbt(["deps"])
            assert e is not None


class TestEmptyDependency:
    def test_empty_package(self, project):
        # We have to specify the bad formatted package here because if we do it
        # in a `packages` fixture, the test will blow up in the setup phase, meaning
        # we can't appropriately catch it with a `pytest.raises`
        empty_hub_package = {
            "packages": [
                {
                    "package": "",
                    "version": "1.0.0",
                }
            ]
        }
        write_config_file(empty_hub_package, "packages.yml")
        with pytest.raises(DbtProjectError, match="A hub package is missing the value"):
            run_dbt(["deps"])

        empty_git_package = {
            "packages": [
                {
                    "git": "",
                    "revision": "1.0.0",
                }
            ]
        }
        write_config_file(empty_git_package, "packages.yml")
        with pytest.raises(DbtProjectError, match="A git package is missing the value"):
            run_dbt(["deps"])

        empty_local_package = {
            "packages": [
                {
                    "local": "",
                }
            ]
        }
        write_config_file(empty_local_package, "packages.yml")
        with pytest.raises(DbtProjectError, match="A local package is missing the value"):
            run_dbt(["deps"])
