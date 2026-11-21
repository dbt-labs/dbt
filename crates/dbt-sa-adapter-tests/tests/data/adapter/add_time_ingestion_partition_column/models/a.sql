{% set table_name = 'b' %}
{% set db_schema = target.schema %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ config(
    materialized = 'incremental',
    incremental_strategy = 'insert_overwrite',
    partition_by = {
      "field": "day",
      "data_type": "date",
      "time_ingestion_partitioning": true
    }
) }}

{% set relation = api.Relation.create(db_name, db_schema, table_name) %}
{% set columns = adapter.get_columns_in_relation(relation) %}
{% set result = adapter.add_time_ingestion_partition_column(model.config.get('partition_by'), columns) %}
{{ print(result) }}

select 1 as id
