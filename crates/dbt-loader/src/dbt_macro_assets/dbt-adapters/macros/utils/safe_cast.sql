-- funcsign: (string, string) -> string
{% macro safe_cast(field, type) %}
  {{ return(adapter.dispatch('safe_cast', 'dbt') (field, type)) }}
{% endmacro %}

-- funcsign: (string, string) -> string
{% macro default__safe_cast(field, type) %}
    {# most databases don't support this function yet
    so we just need to use cast #}
    cast({{field}} as {{type}})
{% endmacro %}
