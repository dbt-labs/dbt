{% macro bigquery__snapshot_check_column_exists(column_name, existing_columns) -%}
    {# BigQuery column names are case-insensitive, even when quoted:
       https://cloud.google.com/bigquery/docs/reference/standard-sql/lexical#case_sensitivity #}
    {{ return(column_name | lower in existing_columns | map('lower') | list) }}
{%- endmacro %}
