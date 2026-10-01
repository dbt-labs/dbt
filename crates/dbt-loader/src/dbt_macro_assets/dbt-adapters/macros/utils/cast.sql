-- funcsign: (string, string) -> string
{% macro cast(field, type) %}
  {{ return(adapter.dispatch('cast', 'dbt') (field, type)) }}
{% endmacro %}

-- funcsign: (string, string) -> string
{% macro default__cast(field, type) %}
    cast({{field}} as {{type}})
{% endmacro %}
