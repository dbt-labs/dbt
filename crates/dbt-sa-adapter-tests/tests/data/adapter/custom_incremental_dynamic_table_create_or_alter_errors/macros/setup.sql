{% macro _fq() -%}
  {{ target.database }}.{{ target.schema }}
{%- endmacro %}

{% macro set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ _fq()) %}
  {% for name in ['err_reorder', 'err_type_narrow', 'err_type_change', 'ok_drop_column'] %}
    {% do adapter.execute('DROP DYNAMIC TABLE IF EXISTS ' ~ _fq() ~ '.' ~ name) %}
  {% endfor %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ _fq() ~ '.orders (order_id int, amount number(10,2), note varchar(100)) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute("INSERT INTO " ~ _fq() ~ ".orders VALUES (1, 10.00, 'a')") %}
{% endmacro %}

{# Each table's columns, `name type` in order, printed for the stdout goldie. #}
{% macro show_columns() %}
  {% for name in ['err_reorder', 'err_type_narrow', 'err_type_change', 'ok_drop_column'] %}
    {% set t = run_query(
        "select column_name || ' ' || data_type || coalesce('(' || character_maximum_length || ')', '') from "
        ~ target.database ~ ".information_schema.columns where table_schema = upper('" ~ target.schema
        ~ "') and table_name = upper('" ~ name ~ "') order by ordinal_position"
    ) %}
    {% do log(name ~ ": " ~ (t.rows | map(attribute=0) | join(', ')), info=True) %}
  {% endfor %}
{% endmacro %}
