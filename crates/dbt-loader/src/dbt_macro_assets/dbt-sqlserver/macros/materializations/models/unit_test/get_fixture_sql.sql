{#
    dbt-core does not dispatch "get_fixture_sql" or "get_expected_sql", so this file
    shadows the implementations from the dbt global project
    (macros/unit_test_sql/get_fixture_sql.sql) - adapter package macros take
    precedence over the global project. Keep in sync with dbt-core when upgrading.

    To ease a future transition if dbt-core adds adapter.dispatch for these macros,
    the public wrappers below are intentionally thin. They delegate to the
    SQL Server-specific implementations (sqlserver__get_fixture_sql and
    sqlserver__get_expected_sql), which contain the copied/adapted upstream logic.

    When upstream dispatches these macros, the public wrappers can be deleted and
    the sqlserver__ implementations kept as the dispatched handlers.

    Changes from upstream (see dbt-msft/dbt-sqlserver#698):
      - sqlserver__get_fixture_sql: the empty-rows branch emits "select top 0"
        instead of "limit 0", which is not valid T-SQL.
      - sqlserver__get_expected_sql: the empty-rows branch emits a "select top 0"
        of typed nulls instead of "select * from dbt_internal_unit_test_actual limit 0".
        Besides the invalid "limit", sqlserver__get_unit_test_sql wraps the expected
        SQL in its own view, where that CTE name is out of scope.
#}

{% macro get_fixture_sql(rows, column_name_to_data_types) %}
    {{ return(sqlserver__get_fixture_sql(rows, column_name_to_data_types)) }}
{% endmacro %}


{% macro sqlserver__get_fixture_sql(rows, column_name_to_data_types) %}
-- Fixture for {{ model.name }}
{% set default_row = {} %}

{%- if not column_name_to_data_types -%}
{#-- Use defer_relation IFF it is available in the manifest and 'this' is missing from the database --#}
{%-   set this_or_defer_relation = defer_relation if (defer_relation and not load_relation(this)) else this -%}
{%-   set columns_in_relation = adapter.get_columns_in_relation(this_or_defer_relation) -%}

{%-   set column_name_to_data_types = {} -%}
{%-   set column_name_to_quoted = {} -%}
{%-   for column in columns_in_relation -%}

{#-- This needs to be a case-insensitive comparison --#}
{%-     do column_name_to_data_types.update({column.name|lower: column.data_type}) -%}
{%-     do column_name_to_quoted.update({column.name|lower: column.quoted}) -%}
{%-   endfor -%}
{%- endif -%}

{%- if not column_name_to_data_types -%}
    {{ exceptions.raise_compiler_error("Not able to get columns for unit test '" ~ model.name ~ "' from relation " ~ this ~ " because the relation doesn't exist") }}
{%- endif -%}

{%- for column_name, column_type in column_name_to_data_types.items() -%}
    {%- do default_row.update({column_name: (safe_cast("null", column_type) | trim )}) -%}
{%- endfor -%}

{{ validate_fixture_rows(rows, row_number) }}

{%- for row in rows -%}
{%-   set formatted_row = format_row(row, column_name_to_data_types) -%}
{%-   set default_row_copy = default_row.copy() -%}
{%-   do default_row_copy.update(formatted_row) -%}
select
{%-   for column_name, column_value in default_row_copy.items() %} {{ column_value }} as {{ column_name_to_quoted[column_name] }}{% if not loop.last -%}, {%- endif %}
{%-   endfor %}
{%-   if not loop.last %}
union all
{%    endif %}
{%- endfor -%}

{%- if (rows | length) == 0 -%}
    select top 0
    {%- for column_name, column_value in default_row.items() %} {{ column_value }} as {{ column_name_to_quoted[column_name] }}{% if not loop.last -%},{%- endif %}
    {%- endfor %}
{%- endif -%}
{% endmacro %}


{% macro get_expected_sql(rows, column_name_to_data_types, column_name_to_quoted) %}
    {{ return(sqlserver__get_expected_sql(rows, column_name_to_data_types, column_name_to_quoted)) }}
{% endmacro %}


{% macro sqlserver__get_expected_sql(rows, column_name_to_data_types, column_name_to_quoted) %}

{%- if (rows | length) == 0 -%}
    select top 0
    {%- for column_name, column_type in column_name_to_data_types.items() %} {{ safe_cast("null", column_type) | trim }} as {{ column_name_to_quoted[column_name] }}{% if not loop.last -%},{%- endif %}
    {%- endfor %}
{%- else -%}
{%- for row in rows -%}
{%- set formatted_row = format_row(row, column_name_to_data_types) -%}
select
{%- for column_name, column_value in formatted_row.items() %} {{ column_value }} as {{ column_name_to_quoted[column_name] }}{% if not loop.last -%}, {%- endif %}
{%- endfor %}
{%- if not loop.last %}
union all
{% endif %}
{%- endfor -%}
{%- endif -%}

{% endmacro %}
