-- funcsign: (relation, model, optional[bool], optional[bool]) -> string
{% macro salesforce__persist_docs(relation, model, for_relation, for_columns) -%}
{#- NOOP since this is seemingly not possible for D360 ... -#}
{#- We may want to raise an error instead though -#}
{% endmacro %}
