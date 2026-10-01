/* {#
    Snowflake overrides for incremental schema change helpers.
#} */

{% macro snowflake__diff_column_data_types(source_columns, target_columns) %}

  {#
    Snowflake COLLATE is not something dbt should attempt to "remove" via
    `ALTER COLUMN ... SET DATA TYPE ...` during incremental schema change sync.
    If the target column already has a collation, preserve it in any emitted
    `new_type` so we don't drop it unintentionally.
  #}

  {%- set result = [] -%}
  {%- for sc in source_columns -%}
    {%- set tc = target_columns | selectattr("name", "equalto", sc.name) | list | first -%}
    {%- if tc -%}
      {# Compare base type only (ignore collation differences). #}
      {%- if sc.data_type != tc.data_type and not sc.can_expand_to(other_column=tc) -%}
        {%- set new_type = sc.data_type -%}

        {# If the existing target column has a collation, keep it. #}
        {%- if tc.collation is defined and tc.collation -%}
          {%- set new_type = new_type ~ " collate '" ~ tc.collation ~ "'" -%}
        {%- endif -%}

        {%- do result.append({'column_name': tc.name, 'new_type': new_type}) -%}
      {%- endif -%}
    {%- endif -%}
  {%- endfor -%}

  {{- return(result) -}}

{% endmacro %}
