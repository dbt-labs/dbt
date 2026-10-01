{% macro wrap_with_time_ingestion_partitioning_sql(partition_by, sql, is_nested) %}

  select TIMESTAMP({{ partition_by.field }}) as {{ partition_by.insertable_time_partitioning_field() }}, * EXCEPT({{ partition_by.field }}) from (
    {{ sql }}
  ){%- if not is_nested -%};{%- endif -%}

{% endmacro %}

{% macro get_quoted_with_types_csv(columns) %}
    {% set quoted = [] %}
    {% for col in columns -%}
        {%- do quoted.append(adapter.quote(col.name) ~ " " ~ col.data_type) -%}
    {%- endfor %}
    {%- set dest_cols_csv = quoted | join(', ') -%}
    {{ return(dest_cols_csv) }}

{% endmacro %}

{% macro columns_without_partition_fields_csv(partition_config, columns) -%}
  {%- set columns_no_partition = partition_config.reject_partition_field_column(columns) -%}
  {% set columns_names = get_quoted_with_types_csv(columns_no_partition) %}
  {{ return(columns_names) }}

{%- endmacro -%}

{% macro bq_insert_into_ingestion_time_partitioned_table_sql(target_relation, sql) -%}
  {%- set sql_header = config.get('sql_header', none) -%}
  {{ sql_header if sql_header is not none }}
  {%- set raw_partition_by = config.get('partition_by', none) -%}
  {%- set partition_by = adapter.parse_partition_by(raw_partition_by) -%}
  {% set dest_columns = adapter.get_columns_in_relation(target_relation) %}
  {%- set dest_columns_csv = get_quoted_csv(dest_columns | map(attribute="name")) -%}

  insert into {{ target_relation }} ({{ partition_by.insertable_time_partitioning_field() }}, {{ dest_columns_csv }})
    {{ wrap_with_time_ingestion_partitioning_sql(partition_by, sql, False) }}

{%- endmacro -%}

{% macro get_columns_with_types_in_query_sql(select_sql) %}
  {% set sql %}
    {%- set sql_header = config.get('sql_header', none) -%}
    {{ sql_header if sql_header is not none }}
    select * from (
      {{ select_sql }}
    ) as __dbt_sbq
    where false
    limit 0
  {% endset %}
  {{ return(adapter.get_columns_in_select_sql(sql)) }}
{% endmacro %}
