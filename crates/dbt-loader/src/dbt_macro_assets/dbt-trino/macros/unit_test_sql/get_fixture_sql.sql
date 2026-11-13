{#--
  Overrides the global (non-dispatched) format_row. Upstream only strips the length
  from 'character varying(n)' types to stop string fixture values from being
  truncated (dbt-core#11974); Trino reports these columns as 'varchar(n)'.
--#}
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

        {%- if column_value is string and (column_type | lower).startswith('varchar(') -%}
            {%- set column_type = column_type.split('(')[0] -%}
        {%- endif -%}

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
