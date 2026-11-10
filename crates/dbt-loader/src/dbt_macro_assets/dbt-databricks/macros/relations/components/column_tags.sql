{% macro fetch_column_tags(relation) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Column tags are only supported for Unity Catalog") }}
  {%- endif %}
  {% call statement('list_column_tags', fetch_result=True) -%}
    {{ fetch_column_tags_sql(relation) }}
  {% endcall %}
  {% do return(load_result('list_column_tags').table) %}
{%- endmacro -%}

{% macro fetch_column_tags_sql(relation) -%}
  SELECT 
    column_name,
    tag_name,
    tag_value
  FROM `system`.`information_schema`.`column_tags`
  WHERE catalog_name = '{{ relation.database|lower }}'
    AND schema_name = '{{ relation.schema|lower }}'
    AND table_name = '{{ relation.identifier|lower }}';
{%- endmacro -%}

{% macro apply_column_tags(relation, column_tags) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Column tags are only supported for Unity Catalog") }}
  {%- endif %}
  {{ log("Applying column tags to relation " ~ relation) }}
  {%- if column_tags.set_column_tags %}
    {%- for column, tags in column_tags.set_column_tags.items() -%}
      {%- call statement('main') -%}
        {{ alter_set_column_tags(relation, column, tags) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
{%- endmacro -%}

{% macro alter_set_column_tags(relation, column, tags) -%}
  {# ALTER VIEW does not support setting column tags, but ALTER TABLE works for views #}
  {%- if relation.type == 'view' -%}
    ALTER TABLE {{ relation.render() }}
  {%- else -%}
    {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
    ALTER {{ render_type(relation.type) }} {{ relation.render() }}
    {#- DIVERGENCE END -#}
  {%- endif -%}
  ALTER COLUMN `{{ column }}`
  SET TAGS (
    {%- for tag_name, tag_value in tags.items() -%}
      '{{ tag_name }}' = '{{ tag_value }}'{%- if not loop.last %}, {% endif -%}
    {%- endfor -%}
  )
{%- endmacro -%}

{% macro unset_column_tags(relation, columns) -%}
  {%- if not relation.is_hive_metastore() and columns -%}
    {%- set column_names = [] -%}
    {%- for column in columns -%}
      {%- do column_names.append(column.name | lower) -%}
    {%- endfor -%}
    {%- set existing_tags = fetch_column_tags(relation) -%}
    {%- set tags_by_column = {} -%}
    {%- for row in existing_tags -%}
      {%- if (row[0] | lower) in column_names -%}
        {%- if row[0] not in tags_by_column -%}
          {%- do tags_by_column.update({row[0]: []}) -%}
        {%- endif -%}
        {%- do tags_by_column[row[0]].append(row[1]) -%}
      {%- endif -%}
    {%- endfor -%}
    {%- for column, tag_names in tags_by_column.items() -%}
      {%- call statement('unset_column_tags') -%}
        {{ alter_unset_column_tags(relation, column, tag_names) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif -%}
{%- endmacro -%}

{% macro alter_unset_column_tags(relation, column, tag_names) -%}
  {# Only reached from the DROP COLUMNS path, which never runs on views. #}
  ALTER {{ relation.type.render() }} {{ relation.render() }}
  ALTER COLUMN `{{ column }}`
  UNSET TAGS (
    {%- for tag_name in tag_names -%}
      '{{ tag_name }}'{%- if not loop.last %}, {% endif -%}
    {%- endfor -%}
  )
{%- endmacro -%}

{% macro column_tags_exist() %}
  {% for column_name, column in model.columns.items() %}
    {% if column is mapping and column.get('databricks_tags') %}
      {{ return(true) }}
    {% endif %}
  {% endfor %}
  {{ return(false) }}
{% endmacro %}

 
