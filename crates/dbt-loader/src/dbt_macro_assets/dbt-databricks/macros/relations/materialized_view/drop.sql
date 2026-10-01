{% macro databricks__drop_materialized_view(relation) -%}
    drop materialized view if exists {{ relation.render() }}
{%- endmacro %}
