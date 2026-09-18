{#- Athena CREATE SCHEMA is Hive DDL: schema name only, backtick-quoted (what
    dbt-athena's Relation.render_hive() produces). Fusion relations have no
    render_hive(); a catalog-qualified, double-quoted name is rejected.
    add_lf_tags_to_database has no Rust counterpart yet and is skipped. -#}
{% macro athena__create_schema(relation) -%}
  {%- call statement('create_schema') -%}
    create schema if not exists `{{ relation.schema }}`
  {% endcall %}
{% endmacro %}


{% macro athena__drop_schema(relation) -%}
  {%- call statement('drop_schema') -%}
    drop schema if exists {{ relation.without_identifier().render_hive() }} cascade
  {% endcall %}
{% endmacro %}


{% macro drop_glue_database(database_name, catalog_name='awsdatacatalog') -%}
  {{ adapter.drop_glue_database(database_name, catalog_name) }}
{% endmacro %}
