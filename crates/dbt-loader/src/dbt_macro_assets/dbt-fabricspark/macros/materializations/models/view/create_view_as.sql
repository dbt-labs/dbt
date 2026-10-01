{% macro fabricspark__create_view_as(relation, sql) -%}
  create or replace view {{ relation }}
  {% if config.persist_column_docs() -%}
    {% set model_columns = model.columns %}
    {% set query_columns = get_columns_in_query(sql) %}
    (
    {{ get_persist_docs_column_list(model_columns, query_columns) }}
    )
  {% endif %}
  {{ comment_clause() }}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{ get_assert_columns_equivalent(sql) }}
  {%- endif %}
  as
    {{ sql }}
{% endmacro %}