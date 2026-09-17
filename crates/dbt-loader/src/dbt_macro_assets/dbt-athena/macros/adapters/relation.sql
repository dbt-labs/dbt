{% macro athena__drop_relation(relation) -%}
  {%- set native_drop = config.get('native_drop', default=false) -%}
  {%- set rel_type_object = adapter.get_glue_table_type(relation) -%}
  {%- set rel_type = none if rel_type_object == none else rel_type_object.value -%}
  {%- set natively_droppable = rel_type == 'iceberg_table' or relation.type == 'view' -%}

  {%- if native_drop and natively_droppable -%}
    {%- do drop_relation_sql(relation) -%}
  {%- else -%}
    {%- do drop_relation_glue(relation) -%}
  {%- endif -%}
{% endmacro %}

{% macro drop_relation_glue(relation) -%}
  {%- do log('Dropping relation via Glue and S3 APIs') -%}
  {%- do adapter.clean_up_table(relation) -%}
  {%- do adapter.delete_from_glue_catalog(relation) -%}
{% endmacro %}

{% macro drop_relation_sql(relation) -%}

  {%- do log('Dropping relation via SQL only') -%}
  {% call statement('drop_relation', auto_begin=False) -%}
    {%- if relation.type == 'view' -%}
      drop {{ relation.type }} if exists {{ relation.render() }}
    {%- else -%}
      drop {{ relation.type }} if exists {{ relation.render_hive() }}
    {% endif %}
  {%- endcall %}
{% endmacro %}

{% macro set_table_classification(relation) -%}
  {%- set format = config.get('format', default='parquet') -%}
  {% call statement('set_table_classification', auto_begin=False) -%}
    alter table {{ relation.render_hive() }} set tblproperties ('classification' = '{{ format }}')
  {%- endcall %}
{%- endmacro %}

{% macro make_temp_relation(base_relation, suffix='__dbt_tmp', temp_schema=none) %}
  {%- set temp_identifier = base_relation.identifier ~ suffix -%}
  {%- set temp_relation = base_relation.incorporate(path={"identifier": temp_identifier}) -%}

  {%- if temp_schema is not none -%}
    {%- set temp_relation = temp_relation.incorporate(path={
      "identifier": temp_identifier,
      "schema": temp_schema
      }) -%}
      {%- do create_schema(temp_relation) -%}
  {% endif %}

  {{ return(temp_relation) }}
{% endmacro %}

{% macro athena__rename_relation(from_relation, to_relation) %}
  {% call statement('rename_relation') -%}
    alter table {{ from_relation.render_hive() }} rename to `{{ to_relation.schema }}`.`{{ to_relation.identifier }}`
  {%- endcall %}
{%- endmacro %}
