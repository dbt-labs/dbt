{# A hermetic dbt-utils-shaped macro for render-trace coverage. #}
{% macro star(from, relation_alias=false, except=[], prefix='', suffix='', quote_identifiers=true) -%}
    {% set columns = ['base_value', 'created_at', 'internal_note'] %}
    {% for column in columns if column not in except -%}
        {% if relation_alias %}{{ relation_alias }}.{% endif -%}
        {% if quote_identifiers %}{{ adapter.quote(column) }}{% else %}{{ column }}{% endif %}
        as {{ adapter.quote(prefix ~ column ~ suffix) }}
        {%- if not loop.last %},
        {% endif -%}
    {% endfor %}
{%- endmacro %}
