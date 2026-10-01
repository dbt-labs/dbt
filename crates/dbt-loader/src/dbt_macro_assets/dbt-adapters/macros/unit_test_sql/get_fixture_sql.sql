-- funcsign: (list[ANY], optional[dict[string, string]]) -> string
{% macro get_fixture_sql(rows, column_name_to_data_types) %}
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
    select
    {%- for column_name, column_value in default_row.items() %} {{ column_value }} as {{ column_name_to_quoted[column_name] }}{% if not loop.last -%},{%- endif %}
    {%- endfor %}
    limit 0
{%- endif -%}
{% endmacro %}


-- funcsign: (list[ANY], dict[string, string], dict[string, string]) -> string
{% macro get_expected_sql(rows, column_name_to_data_types, column_name_to_quoted) %}

{%- if (rows | length) == 0 -%}
    select * from dbt_internal_unit_test_actual
    limit 0
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

-- funcsign: (dict[string, string], dict[string, string]) -> dict[string, string]
{%- macro format_row(row, column_name_to_data_types) -%}
    {#-- generate case-insensitive formatted row --#}
    {% set formatted_row = {} %}
    {%- for column_name, column_value in row.items() -%}
        {% set column_name = column_name|lower %}

        {%- if column_name not in column_name_to_data_types %}
            {#-- if user-provided row contains column name that relation does not contain, raise an error --#}
            {% set fixture_name = "expected output" if model.resource_type == 'unit_test' else ("'" ~ model.name ~ "'") %}
            {{ exceptions.raise_compiler_error(
                "Invalid column name: '" ~ column_name ~ "' in unit test fixture for " ~ fixture_name ~ "."
                "\nAccepted columns for " ~ fixture_name ~ " are: " ~ (column_name_to_data_types.keys()|list)
            ) }}
        {%- endif -%}

        {%- set column_type = column_name_to_data_types[column_name] %}

        {#-- sanitize column_value: wrap yaml strings in quotes, apply cast --#}
        {%- set column_value_clean = column_value -%}
        {%- if column_value is string -%}
            {%- set column_value_clean = dbt.string_literal(dbt.escape_single_quotes(column_value)) -%}
        {%- elif column_value is none -%}
            {%- set column_value_clean = 'null' -%}
        {%- endif -%}

        {%- set row_update = {column_name: safe_cast(column_value_clean, column_type) } -%}
        {%- do formatted_row.update(row_update) -%}
    {%- endfor -%}
    {{ return(formatted_row) }}
{%- endmacro -%}

-- funcsign: (optional[list[ANY]], integer) -> string
{%- macro validate_fixture_rows(rows, row_number) -%}
  {{ return(adapter.dispatch('validate_fixture_rows', 'dbt')(rows, row_number)) }}
{%- endmacro -%}

-- funcsign: (optional[list[ANY]], integer) -> string
{%- macro default__validate_fixture_rows(rows, row_number) -%}
  {# This is an abstract method for adapter overrides as needed #}
{%- endmacro -%}
