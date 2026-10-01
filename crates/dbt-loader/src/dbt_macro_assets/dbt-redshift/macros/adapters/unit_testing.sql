{%- macro redshift__validate_fixture_rows(rows, row_number) -%}
  {%- if rows is not none and rows|length > 0 -%}
    {%- set row = rows[0] -%}
    {%- for key, value in row.items() -%}
      {%- if value is none -%}
          {%- set fixture_name = "expected output" if model.resource_type == 'unit_test' else ("'" ~ model.name ~ "'") -%}
          {{ exceptions.raise_compiler_error("Unit test fixture " ~ fixture_name ~ " in " ~ model.name ~ " does not have any row free of null values, which may cause type mismatch errors during unit test execution.") }}
      {%- endif -%}
    {%- endfor -%}
  {%- endif -%}
{%- endmacro -%}
