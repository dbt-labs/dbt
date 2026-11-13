{{ config(static_analysis='off') }}

with tagged_statements as (
    select count(*) as statement_count
    from system.query.history
    where query_tags['dbt_test_suite'] = 'databricks_query_tags'
      and query_tags['@@dbt_model_name'] = 'tagged_model'
      and query_tags['@@dbt_materialized'] = 'table'
      and query_tags['@@dbt_core_version'] is not null
)

select statement_count
from tagged_statements
where statement_count = 0
