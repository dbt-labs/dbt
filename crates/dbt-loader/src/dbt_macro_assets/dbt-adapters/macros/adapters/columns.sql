-- funcsign: (relation) -> optional[agate_table]
{% macro get_columns_in_relation(relation) -%}
  {{ return(adapter.dispatch('get_columns_in_relation', 'dbt')(relation)) }}
{% endmacro %}

-- funcsign: (relation) -> optional[agate_table]
{% macro default__get_columns_in_relation(relation) -%}
  {{ exceptions.raise_not_implemented(
    'get_columns_in_relation macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}

{# helper for adapter-specific implementations of get_columns_in_relation #}
-- funcsign: (agate_table) -> list[api.column]
{% macro sql_convert_columns_in_relation(table) -%}
  {% set columns = [] %}
  {% for row in table %}
    {% do columns.append(api.Column(*row)) %}
  {% endfor %}
  {{ return(columns) }}
{% endmacro %}

-- funcsign: (list[base_column]) -> list[string]
{%- macro get_list_of_column_names(columns) -%}
  {% set col_names = [] %}
  {% for col in columns %}
    {% do col_names.append(col.name) %}
  {% endfor %}
  {{ return(col_names) }}
{% endmacro %}

-- funcsign: (string, optional[string]) -> string
{% macro get_empty_subquery_sql(select_sql, select_sql_header=none) -%}
  {#-- DIVERGENCE BEGIN: strip a terminal `;` so it does not land inside the
       wrapper built below; done at the dispatcher so the adapter overrides are
       covered too. See `StmtSplitter::strip_trailing_statement_terminator`.
       Upstream: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/include/global_project/macros/adapters/columns.sql --#}
  {%- if dbt_version.startswith('2.') -%}
    {%- set select_sql = adapter.strip_trailing_statement_terminator(select_sql) -%}
  {%- endif -%}
  {#-- DIVERGENCE END --#}
  {{ return(adapter.dispatch('get_empty_subquery_sql', 'dbt')(select_sql, select_sql_header)) }}
{% endmacro %}

{#
  Builds a query that results in the same schema as the given select_sql statement, without necessitating a data scan.
  Useful for running a query in a 'pre-flight' context, such as model contract enforcement (assert_columns_equivalent macro).
#}
-- funcsign: (string, optional[string]) -> string
{% macro default__get_empty_subquery_sql(select_sql, select_sql_header=none) %}
    {%- if select_sql_header is not none -%}
    {{ select_sql_header }}
    {%- endif -%}
    select * from (
        {{ select_sql }}
    ) as __dbt_sbq
    where false
    limit 0
{% endmacro %}

-- funcsign: (dict[string, api.column]) -> string
{% macro get_empty_schema_sql(columns) -%}
  {{ return(adapter.dispatch('get_empty_schema_sql', 'dbt')(columns)) }}
{% endmacro %}

-- funcsign: (dict[string, api.column]) -> string
{% macro default__get_empty_schema_sql(columns) %}
    {%- set col_err = [] -%}
    {%- set col_naked_numeric = [] -%}
    select
    {% for i in columns %}
      {%- set col = columns[i] -%}
      {%- if col['data_type'] is not defined -%}
        {%- do col_err.append(col['name']) -%}
      {#-- If this column's type is just 'numeric' then it is missing precision/scale, raise a warning --#}
      {#-- TYPE CHECK: col['data_type'] is optional[string] but user have this constraint --#}
      {%- elif col['data_type'].strip().lower() in ('numeric', 'decimal', 'number') -%}
        {%- do col_naked_numeric.append(col['name']) -%}
      {%- endif -%}
      {% set col_name = adapter.quote(col['name']) if col.get('quote') else col['name'] %}
      {{ cast('null', col['data_type']) }} as {{ col_name }}{{ ", " if not loop.last }}
    {%- endfor -%}
    {%- if (col_err | length) > 0 -%}
      {{ exceptions.column_type_missing(column_names=col_err) }}
    {%- elif (col_naked_numeric | length) > 0 -%}
      {{ exceptions.warn("Detected columns with numeric type and unspecified precision/scale, this can lead to unintended rounding: " ~ col_naked_numeric ~ "`") }}
    {%- endif -%}
{% endmacro %}

-- funcsign: (string, optional[string]) -> list[base_column]
{% macro get_column_schema_from_query(select_sql, select_sql_header=none) -%}
    {% set columns = [] %}
    {# -- Using an 'empty subquery' here to get the same schema as the given select_sql statement, without necessitating a data scan.#}
    {% set sql = get_empty_subquery_sql(select_sql, select_sql_header) %}
    {% set column_schema = adapter.get_column_schema_from_query(sql) %}
    {{ return(column_schema) }}
{% endmacro %}

-- here for back compat
-- funcsign: (string) -> list[string]
{% macro get_columns_in_query(select_sql) -%}
  {{ return(adapter.dispatch('get_columns_in_query', 'dbt')(select_sql)) }}
{% endmacro %}

-- funcsign: (string) -> list[string]
{% macro default__get_columns_in_query(select_sql) %}
    {% call statement('get_columns_in_query', fetch_result=True, auto_begin=False) -%}
        {{ get_empty_subquery_sql(select_sql) }}
    {% endcall %}
    {# DIVERGENCE: Fusion reads the original schema because Agate flattens nested columns. #}
    {{ return(load_result('get_columns_in_query').table.top_level_column_names) }}
{% endmacro %}

-- funcsign: (relation, string, string) -> string
{% macro alter_column_type(relation, column_name, new_column_type) -%}
  {{ return(adapter.dispatch('alter_column_type', 'dbt')(relation, column_name, new_column_type)) }}
{% endmacro %}

-- funcsign: (relation, string, string) -> string
{% macro default__alter_column_type(relation, column_name, new_column_type) -%}
  {#
    1. Create a new column (w/ temp name and correct type)
    2. Copy data over to it
    3. Drop the existing column (cascade!)
    4. Rename the new column to existing column
  #}
  {%- set tmp_column = column_name + "__dbt_alter" -%}

  {% call statement('alter_column_type') %}
    alter table {{ relation.render() }} add column {{ adapter.quote(tmp_column) }} {{ new_column_type }};
    update {{ relation.render() }} set {{ adapter.quote(tmp_column) }} = {{ adapter.quote(column_name) }};
    alter table {{ relation.render() }} drop column {{ adapter.quote(column_name) }} cascade;
    alter table {{ relation.render() }} rename column {{ adapter.quote(tmp_column) }} to {{ adapter.quote(column_name) }}
  {% endcall %}

{% endmacro %}


-- funcsign: (relation, optional[list[base_column]], optional[list[base_column]]) -> string
{% macro alter_relation_add_remove_columns(relation, add_columns = none, remove_columns = none) -%}
  {{ return(adapter.dispatch('alter_relation_add_remove_columns', 'dbt')(relation, add_columns, remove_columns)) }}
{% endmacro %}

-- funcsign: (relation, optional[list[base_column]], optional[list[base_column]]) -> string
{% macro default__alter_relation_add_remove_columns(relation, add_columns, remove_columns) %}

  {% if add_columns is none %}
    {% set add_columns = [] %}
  {% endif %}
  {% if remove_columns is none %}
    {% set remove_columns = [] %}
  {% endif %}

  {% set sql -%}

     alter {{ relation.type }} {{ relation.render() }}

            {% for column in add_columns %}
               add column {{ column.quoted }} {{ column.expanded_data_type }}{{ ',' if not loop.last }}
            {% endfor %}{{ ',' if add_columns and remove_columns }}

            {% for column in remove_columns %}
                drop column {{ column.quoted }}{{ ',' if not loop.last }}
            {% endfor %}

  {%- endset -%}

  {% do run_query(sql) %}

{% endmacro %}
