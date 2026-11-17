{# Temporary tables belong to the connection and must be referenced without a schema. #}
{% macro redis_adbc__make_temp_relation(base_relation, suffix) -%}
  {{ return(base_relation.incorporate(path={"identifier": base_relation.identifier ~ suffix}).include(database=False, schema=False)) }}
{%- endmacro %}

{% macro redis_adbc__get_limit_sql(sql, limit) -%}
  {%- if limit is none -%}
    {{ sql }}
  {%- else -%}
    select * from ({{ sql }}) as dbt_limit_subq limit {{ limit }}
  {%- endif -%}
{%- endmacro %}

{% macro redis_adbc__current_timestamp() -%}current_timestamp{%- endmacro %}
{% macro redis_adbc__snapshot_get_time() -%}localtimestamp{%- endmacro %}

{% macro redis_adbc__get_columns_in_relation(relation) -%}
  {{ return(adapter.get_columns_in_relation(relation)) }}
{%- endmacro %}

{% macro redis_adbc__get_incremental_default_sql(arg_dict) -%}
  {%- if arg_dict['unique_key'] -%}
    {{ return(get_incremental_delete_insert_sql(arg_dict)) }}
  {%- else -%}
    {{ return(get_incremental_append_sql(arg_dict)) }}
  {%- endif -%}
{%- endmacro %}

{% macro redis_adbc__current_timestamp_in_utc_backcompat() -%}
  (current_timestamp at time zone 'utc')::timestamp
{%- endmacro %}

{% macro redis_adbc__validate_sql(sql) -%}
  {% call statement('validate_sql') -%}
    select * from ({{ sql }}) as dbt_validate_sql where false limit 0
  {%- endcall %}
  {{ return(load_result('validate_sql')) }}
{%- endmacro %}

{% macro redis_adbc__list_relations_without_caching(schema_relation) -%}
  {% set query %}
    select table_catalog as database, table_name as name, table_schema as schema,
           case when table_type = 'VIEW' then 'view' else 'table' end as type
    from information_schema.tables
    where table_schema = '{{ schema_relation.schema | replace("'", "''") }}'
  {% endset %}
  {{ return(run_query(query)) }}
{%- endmacro %}
