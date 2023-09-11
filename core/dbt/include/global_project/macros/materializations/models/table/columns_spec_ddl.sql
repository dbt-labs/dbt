{%- macro get_table_columns_and_constraints() -%}
  {{ adapter.dispatch('get_table_columns_and_constraints', 'dbt')() }}
{%- endmacro -%}

{% macro default__get_table_columns_and_constraints() -%}
  {{ return(table_columns_and_constraints()) }}
{%- endmacro %}

{% macro table_columns_and_constraints() %}
  {# loop through user_provided_columns to create DDL with data types and constraints #}
    {%- if defer_relation and config.get('contract').inferred -%}
      {% set prod_columns = get_column_schema_from_query(get_empty_subquery_sql("select * from " ~ defer_relation)) %}
      {% set raw_columns = {} %}
      {% for column in prod_columns -%}
        {# TODO: reconsider this mutating operation, could be done with an additional warehouse call #}
        {%- if column.name not in model['columns'] -%}
          {% do model['columns'].update(
            {column.name: {'name': column.name, 'data_type': column.data_type, 'description': '', 'constraints': [], 'tags': [], 'meta': {}}}
            ) %}
          {%- endif -%}
      {% endfor -%}
    {%- endif -%}

    {%- set raw_column_constraints = adapter.render_raw_columns_constraints(raw_columns=model['columns']) -%}
    {%- set raw_model_constraints = adapter.render_raw_model_constraints(raw_constraints=model['constraints']) -%}

    (
    {% for c in raw_column_constraints -%}
      {{ c }}{{ "," if not loop.last or raw_model_constraints }}
    {% endfor %}
    {% for c in raw_model_constraints -%}
        {{ c }}{{ "," if not loop.last }}
    {% endfor -%}
    )
{% endmacro %}

{%- macro get_assert_columns_equivalent(sql) -%}
  {{ adapter.dispatch('get_assert_columns_equivalent', 'dbt')(sql) }}
{%- endmacro -%}

{% macro default__get_assert_columns_equivalent(sql) -%}
  {{ return(assert_columns_equivalent(sql)) }}
{%- endmacro %}

{#
  Compares the column schema provided by a model's sql file to the column schema provided by a model's schema file.
  If any differences in name, data_type or number of columns exist between the two schemas, raises a compiler error
#}
{% macro assert_columns_equivalent(sql) %}

  {#-- First ensure the user has defined 'columns' in yaml specification --#}
  {%- set user_defined_columns = model['columns'] -%}

  {%- if not user_defined_columns and not config.get('contract').inferred -%}
      {{ exceptions.raise_contract_error([], []) }}
  {%- endif -%}

  {#-- Obtain the column schema provided by sql file. #}
  {%- set sql_file_provided_columns = get_column_schema_from_query(sql, config.get('sql_header', none)) -%}
  {#--Obtain the column schema provided by the schema file by generating an 'empty schema' query from the model's columns. #}
  {%- set schema_file_provided_columns = get_column_schema_from_query(get_empty_schema_sql(user_defined_columns)) -%}
  {%- if defer_relation and config.get('contract').inferred -%}
      {% set prod_columns = get_column_schema_from_query(get_empty_subquery_sql("select * from " ~ defer_relation)) %}

      {% set provided_column_dict = {} %}
      {%- for provided_column in  schema_file_provided_columns-%}
        {%- do provided_column_dict.update({provided_column.name: provided_column}) %}
      {%- endfor -%}

      {#-- Add any inferred columns not provided to full list of provided columns #}
      {%- for prod_colum in prod_columns -%}
        {%- if prod_colum.name not in provided_column_dict -%}
          {% do schema_file_provided_columns.append(prod_colum) -%}
        {%- endif -%}
      {%- endfor -%}
  {%- endif -%}
  {#-- create dictionaries with name and formatted data type and strings for exception #}
  {%- set sql_columns = format_columns(sql_file_provided_columns) -%}
  {%- set yaml_columns = format_columns(schema_file_provided_columns)  -%}

  {%- if sql_columns|length != yaml_columns|length -%}
    {%- do exceptions.raise_contract_error(yaml_columns, sql_columns) -%}
  {%- endif -%}

  {%- for sql_col in sql_columns -%}
    {%- set yaml_col = [] -%}
    {%- for this_col in yaml_columns -%}
      {%- if this_col['name'] == sql_col['name'] -%}
        {%- do yaml_col.append(this_col) -%}
        {%- break -%}
      {%- endif -%}
    {%- endfor -%}
    {%- if not yaml_col -%}
      {#-- Column with name not found in yaml #}
      {%- do exceptions.raise_contract_error(yaml_columns, sql_columns) -%}
    {%- endif -%}
    {%- if sql_col['formatted'] != yaml_col[0]['formatted'] -%}
      {#-- Column data types don't match #}
      {%- do exceptions.raise_contract_error(yaml_columns, sql_columns) -%}
    {%- endif -%}
  {%- endfor -%}

{% endmacro %}

{% macro format_columns(columns) %}
  {% set formatted_columns = [] %}
  {% for column in columns %}
    {%- set formatted_column = adapter.dispatch('format_column', 'dbt')(column) -%}
    {%- do formatted_columns.append(formatted_column) -%}
  {% endfor %}
  {{ return(formatted_columns) }}
{% endmacro %}

{% macro default__format_column(column) -%}
  {% set data_type = column.dtype %}
  {% set formatted = column.column.lower() ~ " " ~ data_type %}
  {{ return({'name': column.name, 'data_type': data_type, 'formatted': formatted}) }}
{%- endmacro -%}
