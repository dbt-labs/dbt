import pytest
from dbt.tests.util import run_dbt, get_unique_ids_in_results

from tests.functional.unit_testing.fixtures import (
    my_model_versioned_yml,
    test_my_model_all_versions_yml,
    test_my_model_exclude_versions_yml,
    test_my_model_include_versions_yml,
    my_model_v1_sql,
    my_model_v2_sql,
    my_model_v3_sql,
    my_model_a_sql,
    my_model_b_sql,
)


# test with no version specified, should create a separate unit test for each version
class TestNoVersionSpecified:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model_a.sql": my_model_a_sql,
            "my_model_b.sql": my_model_b_sql,
            "my_model_v1.sql": my_model_v1_sql,
            "my_model_v2.sql": my_model_v2_sql,
            "my_model_v3.sql": my_model_v3_sql,
            "schema.yml": my_model_versioned_yml,
            "unit_tests.yml": test_my_model_all_versions_yml,
        }

    def test_no_version_specified(self, project):
        results = run_dbt(["run"])
        assert len(results) == 5

        results = run_dbt(["test"])
        assert len(results) == 3
        unique_ids = get_unique_ids_in_results(results)
        expected_ids = [
            "unit_test.test.my_model.test_my_model_v1",
            "unit_test.test.my_model.test_my_model_v2",
            "unit_test.test.my_model.test_my_model_v3",
        ]
        assert sorted(expected_ids) == sorted(unique_ids)


# with with an exclude version specified, should create a separate unit test for each version except the excluded version
class TestExcludeVersionSpecified:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model_a.sql": my_model_a_sql,
            "my_model_b.sql": my_model_b_sql,
            "my_model_v1.sql": my_model_v1_sql,
            "my_model_v2.sql": my_model_v2_sql,
            "my_model_v3.sql": my_model_v3_sql,
            "schema.yml": my_model_versioned_yml,
            "unit_tests.yml": test_my_model_exclude_versions_yml,
        }

    def test_exclude_version_specified(self, project):
        results = run_dbt(["run"])
        assert len(results) == 5

        results = run_dbt(["test"])
        assert len(results) == 2
        unique_ids = get_unique_ids_in_results(results)
        # v2 model should be excluded
        expected_ids = [
            "unit_test.test.my_model.test_my_model_v1",
            "unit_test.test.my_model.test_my_model_v3",
        ]
        assert sorted(expected_ids) == sorted(unique_ids)


# test with an include version specified, should create a single unit test for only the version specified
class TestIncludeVersionSpecified:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model_a.sql": my_model_a_sql,
            "my_model_b.sql": my_model_b_sql,
            "my_model_v1.sql": my_model_v1_sql,
            "my_model_v2.sql": my_model_v2_sql,
            "my_model_v3.sql": my_model_v3_sql,
            "schema.yml": my_model_versioned_yml,
            "unit_tests.yml": test_my_model_include_versions_yml,
        }

    def test_include_version_specified(self, project):
        results = run_dbt(["run"])
        assert len(results) == 5

        results = run_dbt(["test"])
        assert len(results) == 1
        unique_ids = get_unique_ids_in_results(results)
        # v2 model should be only one included
        expected_ids = [
            "unit_test.test.my_model.test_my_model_v2",
        ]
        assert sorted(expected_ids) == sorted(unique_ids)


# test with an include and exclude version specified, should get ValidationError

# test with an include for an unversioned model, should error

# partial parsing test: test with no version specified, then add an exclude version, then switch to include version and make sure the right unit tests are generated for each

# test with no version specified in the schema file and use selection logic for a specific version

# test specifying the fixture version with {{ ref(name, version) }}
