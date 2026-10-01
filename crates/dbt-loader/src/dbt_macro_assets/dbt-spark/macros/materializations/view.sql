{% materialization view, adapter='spark' -%}
    {{ return(create_or_replace_view()) }}
{%- endmaterialization %}
