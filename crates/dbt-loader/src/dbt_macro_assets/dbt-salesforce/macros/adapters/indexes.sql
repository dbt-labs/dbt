-- funcsign: (relation) -> string
{% macro salesforce__create_indexes(relation) -%}
{{ exceptions.raise_not_implemented('create_indexes macro not implemented for adapter salesforce') }}
{%- endmacro %}
