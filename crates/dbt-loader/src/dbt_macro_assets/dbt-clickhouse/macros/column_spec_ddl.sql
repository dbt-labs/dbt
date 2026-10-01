{% macro clickhouse__get_assert_columns_equivalent(sql) -%}
  {%- set user_defined_columns = model['columns'] -%}

  {%- if not user_defined_columns -%}
      {{ exceptions.raise_contract_error([], []) }}
  {%- endif -%}

  {%- set yaml_columns = user_defined_columns.values() -%}

  {# Apply model query_settings so schema introspection matches runtime types
     (e.g. Nullable results when join_use_nulls=1). Fixes #667. #}
  {%- set query_settings = config.get('query_settings', {}) or {} -%}
  {%- set sql_file_provided_columns = adapter.get_column_schema_from_query(sql, query_settings=query_settings) -%}
  {%- set sql_columns = adapter.format_columns(sql_file_provided_columns) -%}

  {%- if sql_columns|length != yaml_columns|length -%}
    {%- do exceptions.raise_contract_error(yaml_columns, sql_columns) -%}
  {%- endif -%}

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
    {%- if sql_col['data_type'] != yaml_col[0]['data_type'] -%}
      {#-- Column data types don't match #}
      {%- do exceptions.raise_contract_error(yaml_columns, sql_columns) -%}
    {%- endif -%}
  {%- endfor -%}

{% endmacro %}
