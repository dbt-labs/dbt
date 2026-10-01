-- funcsign: (relation) -> string
{%- macro get_drop_sql(relation) -%}
    {{- log('Applying DROP to: ' ~ relation) -}}
    {{- adapter.dispatch('get_drop_sql', 'dbt')(relation) -}}
{%- endmacro -%}


-- funcsign: (relation) -> string
{%- macro default__get_drop_sql(relation) -%}

    {%- if relation.is_view -%}
        {{ drop_view(relation) }}

    {%- elif relation.is_table -%}
        {{ drop_table(relation) }}

    {%- elif relation.is_materialized_view -%}
        {{ drop_materialized_view(relation) }}

    {%- else -%}
        drop {{ relation.type }} if exists {{ relation.render() }} cascade

    {%- endif -%}

{%- endmacro -%}


-- funcsign: (relation) -> string
{% macro drop_relation(relation) -%}
    {{ return(adapter.dispatch('drop_relation', 'dbt')(relation)) }}
{% endmacro %}

-- funcsign: (relation) -> string
{% macro default__drop_relation(relation) -%}
    {% call statement('drop_relation', auto_begin=False) -%}
        {{ get_drop_sql(relation) }}
    {%- endcall %}
{% endmacro %}


-- funcsign: (optional[relation]) -> string
{% macro drop_relation_if_exists(relation) %}
  {% if relation is not none %}
    {{ adapter.drop_relation(relation) }}
  {% endif %}
{% endmacro %}
