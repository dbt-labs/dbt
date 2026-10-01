{% macro fabricspark__list_relations_without_caching(relation) %}
  {% call statement('list_relations_without_caching', fetch_result=True) -%}
    show table extended in {{ relation.schema }} like '*'
  {% endcall %}

  {% do return(load_result('list_relations_without_caching').table) %}
{% endmacro %}

{% macro list_relations_show_tables_without_caching(schema_relation) %}
  {#-- Spark with iceberg tables don't work with show table extended for #}
  {#-- V2 iceberg tables #}
  {#-- https://issues.apache.org/jira/browse/SPARK-33393 #}
  {% call statement('list_relations_without_caching_show_tables', fetch_result=True) -%}
    show tables in {{ schema_relation.schema }} like '*'
  {% endcall %}

  {% do return(load_result('list_relations_without_caching_show_tables').table) %}
{% endmacro %}

{% macro fabricspark__rename_relation(from_relation, to_relation) -%}
  {% call statement('rename_relation') -%}
    {% if not from_relation.type %}
      {% do exceptions.raise_database_error("Cannot rename a relation with a blank type: " ~ from_relation.identifier) %}
    {% elif from_relation.type in ('table') %}
        alter table {{ from_relation }} rename to {{ to_relation }}
    {% elif from_relation.type == 'view' %}
        alter view {{ from_relation }} rename to {{ to_relation }}
    {% else %}
      {% do exceptions.raise_database_error("Unknown type '" ~ from_relation.type ~ "' for relation: " ~ from_relation.identifier) %}
    {% endif %}
  {%- endcall %}
{% endmacro %}

{% macro fabricspark__drop_relation(relation) -%}
  {% call statement('drop_relation', auto_begin=False) -%}
    drop {{ relation.type }} if exists {{ relation }}
  {%- endcall %}
{% endmacro %}

{% macro fabricspark__make_temp_relation(base_relation, suffix) %}
    {% set tmp_identifier = base_relation.identifier ~ suffix %}
    {% set tmp_relation = base_relation.incorporate(path = {
        "identifier": tmp_identifier
    }) -%}
    {%- set tmp_relation = tmp_relation.include(database=true, schema=false) -%}
    {% do return(tmp_relation) %}
{% endmacro %}
