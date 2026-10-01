-- funcsign: (string, string, optional[string]) -> any
{% macro collect_freshness(source, loaded_at_field, filter) %}
  {{ return(adapter.dispatch('collect_freshness', 'dbt')(source, loaded_at_field, filter))}}
{% endmacro %}

-- funcsign: (string, string, optional[string]) -> any
{% macro default__collect_freshness(source, loaded_at_field, filter) %}
  {% call statement('collect_freshness', fetch_result=True, auto_begin=False) -%}
    select
      max({{ loaded_at_field }}) as max_loaded_at,
      {{ current_timestamp() }} as snapshotted_at
    from {{ source }}
    {% if filter %}
    where {{ filter }}
    {% endif %}
  {% endcall %}
  {{ return(load_result('collect_freshness')) }}
{% endmacro %}

-- funcsign: (string, string) -> any
{% macro collect_freshness_custom_sql(source, loaded_at_query) %}
  {{ return(adapter.dispatch('collect_freshness_custom_sql', 'dbt')(source, loaded_at_query))}}
{% endmacro %}

-- funcsign: (string, string) -> any
{% macro default__collect_freshness_custom_sql(source, loaded_at_query) %}
  {% call statement('collect_freshness_custom_sql', fetch_result=True, auto_begin=False) -%}
  with source_query as (
    {{ loaded_at_query }}
  )
  select
    (select * from source_query) as max_loaded_at,
    {{ current_timestamp() }} as snapshotted_at
  {% endcall %}
  {{ return(load_result('collect_freshness_custom_sql')) }}
{% endmacro %}
