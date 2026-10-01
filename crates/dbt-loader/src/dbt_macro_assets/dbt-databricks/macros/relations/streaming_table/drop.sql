{% macro drop_streaming_table(relation) -%}
    {{ return(adapter.dispatch('drop_streaming_table', 'dbt')(relation)) }}
{%- endmacro %}

{% macro default__drop_streaming_table(relation) -%}
    drop table if exists {{ relation.render() }}
{%- endmacro %}
