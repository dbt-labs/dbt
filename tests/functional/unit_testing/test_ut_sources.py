import pytest
from dbt.tests.util import run_dbt

raw_customers_csv = """id,first_name,last_name,email
1,Michael,Perez,mperez0@chronoengine.com
2,Shawn,Mccoy,smccoy1@reddit.com
3,Kathleen,Payne,kpayne2@cargocollective.com
4,Jimmy,Cooper,jcooper3@cargocollective.com
5,Katherine,Rice,krice4@typepad.com
6,Sarah,Ryan,sryan5@gnu.org
7,Martin,Mcdonald,mmcdonald6@opera.com
8,Frank,Robinson,frobinson7@wunderground.com
9,Jennifer,Franklin,jfranklin8@mail.ru
10,Henry,Welch,hwelch9@list-manage.com
"""

schema_sources_yml = """
sources:
  - name: seed_sources
    schema: "{{ target.schema }}"
    tables:
      - name: raw_customers
        columns:
          - name: id
            tests:
              - not_null:
                  severity: "{{ 'error' if target.name == 'prod' else 'warn' }}"
              - unique
          - name: first_name
          - name: last_name
          - name: email
unit_tests:
  - name: test_customers
    model: customers
    given:
      - input: source('seed_sources', 'raw_customers')
        rows:
          - {id: 1, first_name: Emily}
    expect:
      rows:
        - {id: 1, first_name: Emily}
"""

customers_sql = """
select * from {{ source('seed_sources', 'raw_customers') }}
"""


class TestUnitTestSourceInput:
    @pytest.fixture(scope="class")
    def seeds(self):
        return {
            "raw_customers.csv": raw_customers_csv,
        }

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "customers.sql": customers_sql,
            "sources.yml": schema_sources_yml,
        }

    def test_source_input(self, project):
        results = run_dbt(["seed"])
        results = run_dbt(["run"])
        len(results) == 1

        results = run_dbt(["test"])
        # following includes 2 non-unit tests
        assert len(results) == 3
