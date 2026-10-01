{# Salesforce adapter does not have support for grants. #}

{# These just always return False #}

-- funcsign: () -> bool
{% macro salesforce__copy_grants() %}
{{ return(False) }}
{% endmacro %}

-- funcsign: () -> bool
{%- macro salesforce__support_multiple_grantees_per_dcl_statement() -%}
{{ return(False) }}
{%- endmacro -%}

{# These raise an exception #}

-- funcsign: (relation) -> string
{% macro salesforce__get_show_grant_sql(relation) %}
{{ exceptions.raise_not_implemented('get_show_grant_sql macro not implemented for adapter salesforce') }}
{% endmacro %}

-- funcsign: (relation, string, list[string]) -> string
{%- macro salesforce__get_grant_sql(relation, privilege, grantees) -%}
{{ exceptions.raise_not_implemented('get_grant_sql macro not implemented for adapter salesforce') }}
{%- endmacro -%}

-- funcsign: (relation, string, list[string]) -> string
{%- macro salesforce__get_revoke_sql(relation, privilege, grantees) -%}
{{ exceptions.raise_not_implemented('get_revoke_sql macro not implemented for adapter salesforce') }}
{%- endmacro -%}

-- funcsign: (relation, dict[string, list[string]], (relation, string, list[string]) -> string) -> list[string]
{%- macro salesforce__get_dcl_statement_list(relation, grant_config, get_dcl_macro) -%}
{{ exceptions.raise_not_implemented('get_dcl_statement_list macro not implemented for adapter salesforce') }}
{%- endmacro -%}

-- funcsign: (list[string]) -> string
{%- macro salesforce__call_dcl_statements(dcl_statement_list) -%}
{{ exceptions.raise_not_implemented('call_dcl_statements macro not implemented for adapter salesforce') }}
{%- endmacro -%}

-- funcsign: (relation, optional[dict[string, list[string]]], bool) -> string
{% macro salesforce__apply_grants(relation, grant_config, should_revoke=True) %}
{{ exceptions.raise_not_implemented('apply_grants macro not implemented for adapter salesforce') }}
{% endmacro %}
