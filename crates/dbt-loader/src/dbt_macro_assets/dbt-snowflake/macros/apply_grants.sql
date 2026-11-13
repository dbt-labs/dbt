{% macro snowflake__copy_grants() %}
    {% set copy_grants = config.get('copy_grants', False) %}
    {{ return(copy_grants) }}
{% endmacro %}

{%- macro snowflake__support_multiple_grantees_per_dcl_statement() -%}
    {{ return(False) }}
{%- endmacro -%}

{#-- DIVERGENCE: Snowflake needs the argument types to name a function in GRANT,
  -- REVOKE and SHOW GRANTS, because one name can have several overloads. --#}
{%- macro snowflake__get_function_grant_signature_sql() -%}
    {{ function_grant_argument_types_sql() }}
{%- endmacro -%}
