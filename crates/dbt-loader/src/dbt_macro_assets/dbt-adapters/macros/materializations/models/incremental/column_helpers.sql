/* {#
    Helper macros for internal use with incremental materializations.
    Use with care if calling elsewhere.
#} */


-- funcsign: (list[string]) -> string
{% macro get_quoted_csv(column_names) %}

    {% set quoted = [] %}
    {% for col in column_names -%}
        {%- do quoted.append(adapter.quote(col)) -%}
    {%- endfor %}

    {%- set dest_cols_csv = quoted | join(', ') -%}
    {{ return(dest_cols_csv) }}

{% endmacro %}


-- funcsign: (list[base_column], list[base_column]) -> list[base_column]
{% macro diff_columns(source_columns, target_columns) %}

  {% set result = [] %}
  {% set source_names = source_columns | map(attribute = 'column') | list %}
  {% set target_names = target_columns | map(attribute = 'column') | list %}

   {# --check whether the name attribute exists in the target - this does not perform a data type check #}
   {% for sc in source_columns %}
     {% if sc.name not in target_names %}
        {{ result.append(sc) }}
     {% endif %}
   {% endfor %}

  {{ return(result) }}

{% endmacro %}


-- funcsign: (list[base_column], list[base_column]) -> list[dict[string, string]]
{% macro diff_column_data_types(source_columns, target_columns) %}
  {{ return(adapter.dispatch('diff_column_data_types', 'dbt')(source_columns, target_columns)) }}
{% endmacro %}

{% macro default__diff_column_data_types(source_columns, target_columns) %}
  {% set result = [] %}
  {% for sc in source_columns %}
    {% set tc = target_columns | selectattr("name", "equalto", sc.name) | list | first %}
    {% if tc %}
      {% if sc.expanded_data_type != tc.expanded_data_type and not sc.can_expand_to(other_column=tc) %}
        {{ result.append( { 'column_name': tc.name, 'new_type': sc.expanded_data_type } ) }}
      {% endif %}
    {% endif %}
  {% endfor %}

  {{ return(result) }}

{% endmacro %}

-- funcsign: (optional[list[string]], optional[list[string]], list[base_column]) -> list[string]
{% macro get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) %}
  {{ return(adapter.dispatch('get_merge_update_columns', 'dbt')(merge_update_columns, merge_exclude_columns, dest_columns)) }}
{% endmacro %}

-- funcsign: (optional[list[string]], optional[list[string]], list[base_column]) -> list[string]
{% macro default__get_merge_update_columns(merge_update_columns, merge_exclude_columns, dest_columns) %}
  {%- set default_cols = dest_columns | map(attribute="quoted") | list -%}

  {%- if merge_update_columns and merge_exclude_columns -%}
    {{ exceptions.raise_compiler_error(
        'Model cannot specify merge_update_columns and merge_exclude_columns. Please update model to use only one config'
    )}}
  {%- elif merge_update_columns -%}
    {%- set update_columns = merge_update_columns -%}
  {%- elif merge_exclude_columns -%}
    {%- set update_columns = [] -%}
    {%- for column in dest_columns -%}
      {% if column.column | lower not in merge_exclude_columns | map("lower") | list %}
        {%- do update_columns.append(column.quoted) -%}
      {% endif %}
    {%- endfor -%}
  {%- else -%}
    {%- set update_columns = default_cols -%}
  {%- endif -%}

  {{ return(update_columns) }}

{% endmacro %}
