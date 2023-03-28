import pytest

models__schema_yml = """
version: 2
models:
  - name: sample_model
    columns:
      - name: sample_num
        tests:
          - accepted_values:
              values: [1, 2]
          - not_null
      - name: sample_bool
        tests:
          - not_null
          - unique
"""

models__sample_model = """
select * from {{ ref('sample_seed') }}
"""

models__second_model = """
select
    sample_num as col_one,
    sample_bool as col_two,
    42 as answer
from {{ ref('sample_model') }}
"""

models__ephemeral_model = """
{{ config(materialized = 'ephemeral') }}
select
    coalesce(sample_num, 0) + 10 as col_deci
from {{ ref('sample_model') }}
"""

models__second_ephemeral_model = """
{{ config(materialized = 'ephemeral') }}
select
    col_deci + 100 as col_hundo
from {{ ref('ephemeral_model') }}
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
4,false
5,true
6,false
7,true
"""

tests__failing_sql = """
{{ config(severity = 'warn') }}
select 1
"""


class BaseConfigProject:
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
                        "port": 5432,
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
            "second_model.sql": models__second_model,
            "ephemeral_model.sql": models__ephemeral_model,
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
