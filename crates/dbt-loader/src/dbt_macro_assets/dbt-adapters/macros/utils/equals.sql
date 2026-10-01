-- ai
-- funcsign: (string, string) -> string
{% macro equals(expr1, expr2) %}
    {{ return(adapter.dispatch('equals', 'dbt') (expr1, expr2)) }}
{%- endmacro %}

-- ai
-- funcsign: (string, string) -> string
{# FIXME: bring v1 and v2 back in sync on https://github.com/dbt-labs/fs/pull/9805 #}
{% macro default__equals(expr1, expr2) -%}
    {%- if dbt_version.startswith('2.') -%}
    {{ adapter.render_equals(expr1, expr2) }}
    {%- else -%}
    {%- if adapter.behavior.enable_truthy_nulls_equals_macro.no_warn %}
    case when (({{ expr1 }} = {{ expr2 }}) or ({{ expr1 }} is null and {{ expr2 }} is null))
        then 0
        else 1
    end = 0
    {%- else -%}
    ({{ expr1 }} = {{ expr2 }})
    {%- endif %}
    {%- endif %}
{%- endmacro %}
