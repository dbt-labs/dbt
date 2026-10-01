{% macro intersect() %}
  {{ return(adapter.dispatch('intersect', 'dbt')()) }}
{% endmacro %}

{% macro default__intersect() %}

    intersect

{% endmacro %}
