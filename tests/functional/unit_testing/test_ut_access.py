import pytest

from dbt.tests.fixtures.project import write_project_files
from dbt.tests.util import run_dbt

restricted_dep__dbt_project_yml = """
name: 'restricted_dep'
version: '1.0'
config-version: 2
restrict-access: true

models:
  restricted_dep:
    +access: public
"""

restricted_dep__upstream_sql = """
select 1 as id
"""

downstream_sql = """
select id from {{ ref('restricted_dep', 'upstream') }}
"""

downstream_unit_tests_yml = """
unit_tests:
  - name: test_downstream
    model: downstream
    given:
      - input: ref('restricted_dep', 'upstream')
        rows:
          - {id: 1}
    expect:
      rows:
        - {id: 1}
"""


class TestUnitTestPublicInputFromRestrictedPackage:
    @pytest.fixture(scope="class", autouse=True)
    def setUp(self, project_root):
        restricted_dep_files = {
            "dbt_project.yml": restricted_dep__dbt_project_yml,
            "models": {"upstream.sql": restricted_dep__upstream_sql},
        }
        write_project_files(project_root, "restricted_dep", restricted_dep_files)

    @pytest.fixture(scope="class")
    def packages(self):
        return {"packages": [{"local": "restricted_dep"}]}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "downstream.sql": downstream_sql,
            "unit_tests.yml": downstream_unit_tests_yml,
        }

    def test_public_input_from_restricted_package(self, project):
        run_dbt(["deps"])
        run_dbt(["run"])
        results = run_dbt(["test", "--select", "test_type:unit"])
        assert len(results) == 1


private_upstream_sql = """
select 1 as id
"""

private_downstream_sql = """
select id from {{ ref('private_upstream') }}
"""

private_schema_yml = """
groups:
  - name: analytics
    owner:
      name: analytics_owner

models:
  - name: private_upstream
    config:
      access: private
      group: analytics
  - name: private_downstream
    config:
      group: analytics

unit_tests:
  - name: test_private_downstream
    model: private_downstream
    given:
      - input: ref('private_upstream')
        rows:
          - {id: 1}
    expect:
      rows:
        - {id: 1}
"""


class TestUnitTestPrivateInputInSameGroup:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "private_upstream.sql": private_upstream_sql,
            "private_downstream.sql": private_downstream_sql,
            "schema.yml": private_schema_yml,
        }

    def test_private_input_in_same_group(self, project):
        run_dbt(["run"])
        results = run_dbt(["test", "--select", "test_type:unit"])
        assert len(results) == 1
