import os

import pytest
from click.testing import CliRunner

models__schema_yml = """
version: 2
models:
  - name: sample_model
    columns:
      - name: sample_num
        data_tests:
          - accepted_values:
              values: [1, 2]
          - not_null
      - name: sample_bool
        data_tests:
          - not_null
          - unique
"""

models__sample_model = """
select * from {{ ref('sample_seed') }}
"""

snapshots__sample_snapshot = """
{% snapshot orders_snapshot %}

{{
    config(
      target_database='dbt',
      target_schema='snapshots',
      unique_key='sample_num',
      strategy='timestamp',
      updated_at='updated_at',
    )
}}

select * from {{ ref('sample_model') }}

{% endsnapshot %}
"""

seeds__sample_seed = """sample_num,sample_bool
1,true
2,false
,true
"""

tests__failing_sql = """
{{ config(severity = 'warn') }}
select 1
"""


class BaseConfigProject:
    @pytest.fixture()
    def runner(self):
        return CliRunner()

    @pytest.fixture(scope="class")
    def project_config_update(self):
        return {
            "name": "jaffle_shop",
            "profile": "jaffle_shop",
            "version": "0.1.0",
            "config-version": 2,
            "clean-targets": ["target", "dbt_packages", "logs"],
        }

    @pytest.fixture(scope="class")
    def profiles_config_update(self):
        return {
            "jaffle_shop": {
                "outputs": {
                    "dev": {
                        "type": "postgres",
                        "dbname": "dbt",
                        "schema": "jaffle_shop",
                        "host": "localhost",
                        "user": "root",
                        "port": int(os.getenv("POSTGRES_TEST_PORT", 5432)),
                        "pass": "password",
                    }
                },
                "target": "dev",
            }
        }

    @pytest.fixture(scope="class")
    def packages(self):
        return {"packages": [{"package": "dbt-labs/dbt_utils", "version": "1.0.0"}]}

    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "sample_model.sql": models__sample_model,
        }

    @pytest.fixture(scope="class")
    def snapshots(self):
        return {"sample_snapshot.sql": snapshots__sample_snapshot}

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"sample_seed.csv": seeds__sample_seed}

    @pytest.fixture(scope="class")
    def tests(self):
        return {
            "failing.sql": tests__failing_sql,
        }
