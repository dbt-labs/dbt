import pytest

models__schema_yml = """
version: 2
sources:
  - name: raw
    database: "{{ target.database }}"
    schema: "{{ target.schema }}"
    tables:
      - name: 'seed'
        identifier: "{{ var('seed_name', 'invalid') }}"
        columns:
          - name: id
            data_tests:
              - unique:
                  enabled: "{{ var('enabled_direct', None) | as_native }}"
              - accepted_values:
                  enabled: "{{ var('enabled_direct', None) | as_native }}"
                  severity: "{{ var('severity_direct', None) | as_native }}"
                  values: [1,2]

models:
  - name: model
    columns:
      - name: id
        data_tests:
          - unique
          - accepted_values:
              values: [1,2,3,4]

"""

models__untagged_sql = """
{{
    config(materialized=table)
}}

select id, value from {{ source('raw', 'seed') }}

"""

models__tagged__model_sql = """
{{
    config(
        materialized='view',
        tags=['tag_two'],
    )
}}

{{
    config(
        materialized='table',
        tags=['tag_three'],
    )
}}

select 4 as id, 2 as value

"""

seeds__seed_csv = """id,value
4,2
"""

tests__failing_sql = """

select 1 as fun

"""

tests__sleeper_agent_sql = """
{{ config(
    enabled = var('enabled_direct', False),
    severity = var('severity_direct', 'WARN')
) }}

select 1 as fun

"""

my_model = """
select 1 as user
"""

my_model_2 = """
select * from {{ ref('my_model') }}
"""

my_model_3 = """
select * from {{ ref('my_model_2') }}
"""

my_model_2_disabled = """
{{ config(enabled=false) }}
select * from {{ ref('my_model') }}
"""

my_model_3_disabled = """
{{ config(enabled=false) }}
select * from {{ ref('my_model_2') }}
"""

my_model_2_enabled = """
{{ config(enabled=true) }}
select * from {{ ref('my_model') }}
"""

my_model_3_enabled = """
{{ config(enabled=true) }}
select * from {{ ref('my_model') }}
"""

schema_all_disabled_yml = """
version: 2
models:
  - name: my_model
  - name: my_model_2
    config:
      enabled: false
  - name: my_model_3
    config:
      enabled: false
"""

schema_explicit_enabled_yml = """
version: 2
models:
  - name: my_model
  - name: my_model_2
    config:
      enabled: true
  - name: my_model_3
    config:
      enabled: true
"""

schema_partial_disabled_yml = """
version: 2
models:
  - name: my_model
  - name: my_model_2
    config:
      enabled: false
  - name: my_model_3
"""

schema_partial_enabled_yml = """
version: 2
models:
  - name: my_model
  - name: my_model_2
    config:
      enabled: True
  - name: my_model_3
"""

schema_invalid_enabled_yml = """
version: 2
models:
  - name: my_model
    config:
      enabled: True and False
  - name: my_model_3
"""

simple_snapshot = """{% snapshot mysnapshot %}

    {{
        config(
          target_schema='snapshots',
          strategy='timestamp',
          unique_key='id',
          updated_at='updated_at'
        )
    }}

    select * from dummy

{% endsnapshot %}"""


class BaseConfigProject:
    @pytest.fixture(scope="class")
    def models(self):
        return {
            "schema.yml": models__schema_yml,
            "untagged.sql": models__untagged_sql,
            "tagged": {"model.sql": models__tagged__model_sql},
        }

    @pytest.fixture(scope="class")
    def seeds(self):
        return {"seed.csv": seeds__seed_csv}

    @pytest.fixture(scope="class")
    def tests(self):
        return {
            "failing.sql": tests__failing_sql,
            "sleeper_agent.sql": tests__sleeper_agent_sql,
        }
