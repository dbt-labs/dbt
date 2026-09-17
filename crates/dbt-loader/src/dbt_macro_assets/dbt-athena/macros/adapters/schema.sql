{% macro athena__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    create schema if not exists {{ relation.without_identifier().render_hive() }}
  {% endcall %}

  {{ adapter.add_lf_tags_to_database(relation) }}

{% endmacro %}


{% macro athena__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation.without_identifier().render_hive() }} cascade
  {% endcall %}
{% endmacro %}


{% macro drop_glue_database(database_name, catalog_name='awsdatacatalog') -%}
  {{ adapter.drop_glue_database(database_name, catalog_name) }}
{% endmacro %}
