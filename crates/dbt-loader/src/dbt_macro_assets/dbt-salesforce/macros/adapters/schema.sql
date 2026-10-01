{# Salesforce does not have the concept of schemas, so these macros are hard errors. #}

-- funcsign: (relation) -> string
{% macro salesforce__create_schema(relation) -%}
{{ exceptions.raise_not_implemented('create_schema macro not implemented for adapter salesforce') }}
{% endmacro %}

-- funcsign: (relation) -> string
{% macro salesforce__drop_schema(relation) -%}
{{ exceptions.raise_not_implemented('drop_schema macro not implemented for adapter salesforce') }}
{% endmacro %}
