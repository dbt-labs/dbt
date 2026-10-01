{# DIVERGENCE: in core, drop_relation is implemented via the googleapi inside the adapter.drop_relation method. #}
{% macro bigquery__drop_relation(relation) -%}
    {%- call statement('drop_relation', auto_begin=False) -%}
        {{ get_drop_sql(relation) }}
    {%- endcall -%}
{% endmacro %}
