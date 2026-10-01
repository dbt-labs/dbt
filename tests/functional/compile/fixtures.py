first_model_sql = """
select 1 as fun
"""

second_model_sql = """
{%- set columns = adapter.get_columns_in_relation(ref('first_model')) -%}
select
    *,
    {{ this.schema }} as schema
from {{ ref('first_model') }}
"""

first_ephemeral_model_sql = """
{{ config(materialized = 'ephemeral') }}
select 1 as fun
"""

second_ephemeral_model_sql = """
{{ config(materialized = 'ephemeral') }}
select * from {{ ref('first_ephemeral_model') }}
"""

third_ephemeral_model_sql = """
select * from {{ ref('second_ephemeral_model')}}
union all
select 2 as fun
"""

model_multiline_jinja = """
select {{
    1 + 1
}} as fun
"""

with_recursive_model_sql = """
{{ config(materialized = 'ephemeral') }}
with recursive t(n) as (
    select * from {{ ref('first_ephemeral_model') }}
  union all
    select n+1 from t where n < 100
)
select sum(n) from t;
"""

first_ephemeral_model_with_alias_sql = """
{{ config(materialized = 'ephemeral', alias = 'first_alias') }}
select 1 as fun
"""

second_ephemeral_model_with_alias_sql = """
select * from {{ ref('first_ephemeral_model_with_alias') }}
"""

schema_yml = """
version: 2

models:
  - name: second_model
    description: "The second model"
    columns:
      - name: fun
        data_tests:
          - not_null
      - name: schema
        data_tests:
          - unique
"""

incremental_model_sql = """
{{ config(materialized='incremental') }}
select 1 as fun
{% if is_incremental() %}
where fun > 0
{% endif %}
"""

table_model_using_is_incremental_sql = """
{{ config(materialized='table') }}
select 1 as fun
{% if is_incremental() %}
where fun > 0
{% endif %}
"""
