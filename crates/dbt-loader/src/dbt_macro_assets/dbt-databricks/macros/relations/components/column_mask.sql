{% macro fetch_column_masks(relation) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Column masks are not supported for Hive Metastore") }}
  {%- endif %}
  {% call statement('list_column_masks', fetch_result=True) -%}
    {{ fetch_column_masks_sql(relation) }}
  {% endcall %}
  {% do return(load_result('list_column_masks').table) %}
{%- endmacro -%}

{% macro fetch_column_masks_sql(relation) -%}
  SELECT 
    column_name,
    mask_name,
    using_columns
  FROM `system`.`information_schema`.`column_masks`
  WHERE table_catalog = '{{ relation.database|lower }}'
    AND table_schema = '{{ relation.schema|lower }}'
    AND table_name = '{{ relation.identifier|lower }}';
{%- endmacro -%}

{% macro apply_column_masks(relation, column_masks) -%}
  {% if relation.is_hive_metastore() %}
    {{ exceptions.raise_compiler_error("Column masks are not supported for Hive Metastore") }}
  {%- endif %}
  {{ log("Applying column masks to relation " ~ relation) }}
  {%- if column_masks.unset_column_masks %}
    {%- for column in column_masks.unset_column_masks -%}
      {%- call statement('main') -%}
        {{ alter_drop_column_mask(relation, column) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
  {%- if column_masks.set_column_masks %}
    {%- for column, mask in column_masks.set_column_masks.items() -%}
      {%- call statement('main') -%}
        {{ alter_set_column_mask(relation, column, mask) }}
      {%- endcall -%}
    {%- endfor -%}
  {%- endif %}
{%- endmacro -%}

{% macro alter_drop_column_mask(relation, column) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }}
  {#- DIVERGENCE END -#}
  ALTER COLUMN `{{ column }}`
  DROP MASK;
{%- endmacro -%}

{% macro alter_set_column_mask(relation, column, mask) -%}
  {#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
  ALTER {{ render_type(relation.type) }} {{ relation.render() }}
  {#- DIVERGENCE END -#}
  ALTER COLUMN `{{ column }}`
  SET MASK {{ mask.function }}
  {%- if mask.using_columns %}
  USING COLUMNS ({{ mask.using_columns }})
  {%- endif %};
{%- endmacro -%}

{% macro column_mask_exists() %}
  {% for column_name, column in model.columns.items() %}
    {% if column is mapping and column.get('column_mask') %}
      {{ return(true) }}
    {% endif %}
  {% endfor %}
  {{ return(false) }}
{% endmacro %}
