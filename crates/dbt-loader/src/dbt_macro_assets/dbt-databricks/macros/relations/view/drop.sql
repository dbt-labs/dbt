{% macro databricks__drop_view(relation) -%}
  DROP VIEW IF EXISTS {{ relation.render() }}
{%- endmacro %}
