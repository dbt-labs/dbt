{% macro athena__create_view_as(relation, sql) -%}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config.enforced -%}
    {{ get_assert_columns_equivalent(sql) }}
  {%- endif -%}
  create or replace view
    {{ relation }}
  as
    {{ sql }}
{% endmacro %}
