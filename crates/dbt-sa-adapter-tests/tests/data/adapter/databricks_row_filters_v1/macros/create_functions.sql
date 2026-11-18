{% macro set_up() %}
  {% do adapter.dispatch("set_up")() %}
{% endmacro %}

{% macro default__set_up() %}
{% endmacro %}

{% macro databricks__set_up() %}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS `' ~ target.database ~ '`.`' ~ target.schema ~ '`') %}

  {% call statement('create_region_filter') %}
    CREATE OR REPLACE FUNCTION {{ target.database }}.{{ t_schema_name() }}.region_filter(
      region STRING
    )
    RETURNS BOOLEAN
    RETURN region = 'region_a'
  {% endcall %}

  {% call statement('create_user_filter') %}
    CREATE OR REPLACE FUNCTION {{ target.database }}.{{ t_schema_name() }}.user_filter(
      user_id STRING
    )
    RETURNS BOOLEAN
    RETURN user_id = 'user1'
  {% endcall %}
{% endmacro %}
