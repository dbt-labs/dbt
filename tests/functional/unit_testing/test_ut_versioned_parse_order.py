from unittest import mock

import pytest

import dbt.parser.read_files as read_files_module
from dbt.tests.util import run_dbt

my_model_sql = "select 1 as id\n"
my_model_v2_sql = "select 1 as id\n"

# Model-versions definition and unit test live in separate YAML files, as they
# commonly do (models/ vs tests/ separation). File names are chosen so that,
# once parse order is made deterministic, the unit-test file is parsed BEFORE
# the model-versions file -- the order that triggers dbt-core #11139.
model_versions_yml = """
models:
  - name: my_model
    latest_version: 1
    versions:
      - v: 2
        defined_in: my_model_v2
      - v: 1
        defined_in: my_model
        config:
          alias: my_model
"""

unit_test_yml = """
unit_tests:
  - name: test_my_model
    model: my_model
    versions:
      include:
        - 2
    given: []
    expect:
      rows:
        - {id: 1}
"""


def _ordered_filesystem_search(original, reverse):
    """Make filesystem parse order deterministic so both orderings of the
    unit-test YAML and model-versions YAML are exercised on every run."""

    def wrapper(*args, **kwargs):
        return sorted(original(*args, **kwargs), key=lambda fp: fp.relative_path, reverse=reverse)

    return wrapper


class TestUnitTestVersionedModelParseOrder:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "my_model.sql": my_model_sql,
            "my_model_v2.sql": my_model_v2_sql,
            # "a_..." sorts before "z_..." so ascending order parses the unit
            # test first, descending order parses the model versions first.
            "a_unit_test.yml": unit_test_yml,
            "z_model_versions.yml": model_versions_yml,
        }

    @pytest.mark.parametrize("reverse", [False, True])
    def test_parse_succeeds_regardless_of_file_order(self, project, reverse):
        # dbt-core #11139: parsing must succeed no matter which of the two
        # schema files is read first. Before the fix, `reverse=False` (unit
        # test parsed before model versions) raises:
        #   Parsing Error - Unit test 'test_my_model' references a model
        #   that does not exist: model.test.my_model
        original = read_files_module.filesystem_search
        with mock.patch.object(
            read_files_module,
            "filesystem_search",
            _ordered_filesystem_search(original, reverse),
        ):
            manifest = run_dbt(["parse"])

        assert "unit_test.test.my_model.test_my_model_v2" in manifest.unit_tests
