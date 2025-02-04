happy_macros_sql = """
{% macro select_something(name) %}
  {% set query %}
    select 'hello, {{ name }}' as name
  {% endset %}
  {% set table = run_query(query) %}
{% endmacro %}

{% macro select_something_with_return(name) %}
  {% set query %}
    select 'hello, {{ name }}' as name
  {% endset %}
  {% set table = run_query(query) %}
  {% do return(table) %}
{% endmacro %}
"""
