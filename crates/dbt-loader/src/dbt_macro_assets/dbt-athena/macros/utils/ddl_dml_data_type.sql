{# Athena has different types between DML and DDL #}
{# ref: https://docs.aws.amazon.com/athena/latest/ug/data-types.html #}
{% macro ddl_data_type(col_type, table_type = 'hive') -%}
  -- transform varchar
  {% set re = modules.re %}
  {% set data_type = re.sub('(?:varchar|character varying)(?:\(\d+\))?', 'string', col_type) %}

  -- transform array and map
  {%- if 'array' in data_type or 'map' in data_type -%}
    {% set data_type = data_type.replace('(', '<').replace(')', '>') -%}
  {%- endif -%}

  -- transform int
  {%- if 'integer' in data_type -%}
    {% set data_type = data_type.replace('integer', 'int') -%}
  {%- endif -%}

  -- transform timestamp
  {%- if table_type == 'iceberg' -%}
    {%- if 'timestamp' in data_type -%}
        {% set data_type = 'timestamp' -%}
    {%- endif -%}

    {%- if 'binary' in data_type -%}
        {% set data_type = 'binary' -%}
    {%- endif -%}
  {%- endif -%}

  {{ return(data_type) }}
{% endmacro %}

{% macro dml_data_type(col_type) -%}
  {%- set re = modules.re -%}
  -- transform int to integer
  {%- set data_type = re.sub('\bint\b', 'integer', col_type) -%}
  -- transform string to varchar because string does not work in DML
  {%- set data_type = re.sub('string', 'varchar', data_type) -%}
  -- transform float to real because float does not work in DML
  {%- set data_type = re.sub('float', 'real', data_type) -%}
  {{ return(data_type) }}
{% endmacro %}
