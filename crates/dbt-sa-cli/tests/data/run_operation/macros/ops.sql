{% macro ok() %}
  {% do log('ok-macro', info=True) %}
{% endmacro %}

{% macro boom_compile() %}
  {% do exceptions.raise_compiler_error('intentional compile boom') %}
{% endmacro %}

{% macro boom_sql() %}
  {% do run_query('select * from this_table_does_not_exist_zz') %}
{% endmacro %}
