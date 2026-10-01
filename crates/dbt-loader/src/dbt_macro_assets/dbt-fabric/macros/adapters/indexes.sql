{% macro fabric__create_clustered_columnstore_index(relation) -%}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro drop_xml_indexes() -%}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro drop_spatial_indexes() -%}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro drop_fk_constraints() -%}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro drop_pk_constraints() -%}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro drop_fk_indexes_on_table(relation) -%}
  {% call statement('find_references', fetch_result=true) %}
      USE [{{ relation.database }}];
      SELECT  obj.name AS FK_NAME,
      sch.name AS [schema_name],
      tab1.name AS [table],
      col1.name AS [column],
      tab2.name AS [referenced_table],
      col2.name AS [referenced_column]
      FROM sys.foreign_key_columns fkc
      INNER JOIN sys.objects obj
          ON obj.object_id = fkc.constraint_object_id
      INNER JOIN sys.tables tab1
          ON tab1.object_id = fkc.parent_object_id
      INNER JOIN sys.schemas sch
          ON tab1.schema_id = sch.schema_id
      INNER JOIN sys.columns col1
          ON col1.column_id = parent_column_id AND col1.object_id = tab1.object_id
      INNER JOIN sys.tables tab2
          ON tab2.object_id = fkc.referenced_object_id
      INNER JOIN sys.columns col2
          ON col2.column_id = referenced_column_id AND col2.object_id = tab2.object_id
      WHERE sch.name = '{{ relation.schema }}' and tab2.name = '{{ relation.identifier }}'
  {% endcall %}
      {% set references = load_result('find_references')['data'] %}
      {% for reference in references -%}
        {% call statement('main') -%}
           alter table [{{reference[1]}}].[{{reference[2]}}] drop constraint [{{reference[0]}}]
        {%- endcall %}
      {% endfor %}
{% endmacro %}

{% macro create_clustered_index(columns, unique=False) -%}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro create_nonclustered_index(columns, includes=False) %}
  {# {% exceptions.raise_compiler_error('Indexes are not supported') %} #}
{% endmacro %}

{% macro fabric__list_nonclustered_rowstore_indexes(relation) -%}
  {% call statement('list_nonclustered_rowstore_indexes', fetch_result=True) -%}

    SELECT i.name AS index_name
    , i.name + '__dbt_backup' as index_new_name
    , COL_NAME(ic.object_id,ic.column_id) AS column_name
    FROM sys.indexes AS i
    INNER JOIN sys.index_columns AS ic
        ON i.object_id = ic.object_id AND i.index_id = ic.index_id and i.type <> 5
    WHERE i.object_id = OBJECT_ID('{{ relation.schema }}.{{ relation.identifier }}')

    UNION ALL

    SELECT  obj.name AS index_name
    , obj.name + '__dbt_backup' as index_new_name
    , col1.name AS column_name
    FROM sys.foreign_key_columns fkc
    INNER JOIN sys.objects obj
        ON obj.object_id = fkc.constraint_object_id
    INNER JOIN sys.tables tab1
        ON tab1.object_id = fkc.parent_object_id
    INNER JOIN sys.schemas sch
        ON tab1.schema_id = sch.schema_id
    INNER JOIN sys.columns col1
        ON col1.column_id = parent_column_id AND col1.object_id = tab1.object_id
    INNER JOIN sys.tables tab2
        ON tab2.object_id = fkc.referenced_object_id
    INNER JOIN sys.columns col2
        ON col2.column_id = referenced_column_id AND col2.object_id = tab2.object_id
    WHERE sch.name = '{{ relation.schema }}' and tab1.name = '{{ relation.identifier }}'

  {% endcall %}
  {{ return(load_result('list_nonclustered_rowstore_indexes').table) }}
{% endmacro %}
