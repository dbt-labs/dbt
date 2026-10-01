{%- materialization unit, adapter='clickhouse' -%}

  {% set relations = [] %}

  {% set expected_rows = config.get('expected_rows') %}
  {% set expected_sql = config.get('expected_sql') %}
  {% set tested_expected_column_names = expected_rows[0].keys() if (expected_rows | length ) > 0 else get_columns_in_query(sql) %}

  {%- set target_relation = this.incorporate(type='table') -%}
  {%- set temp_relation = make_temp_relation(target_relation, '__dbt_tmp')-%}
  {%- set temp_relation = temp_relation.incorporate(is_temporary=True)-%} {# file overwrite to ensure this is marked as temporary #}
  {% do get_create_table_as_sql(True, temp_relation, get_empty_subquery_sql(sql)) %}
  {%- set columns_in_relation = adapter.get_columns_in_relation(temp_relation) -%}
  {%- set column_name_to_data_types = {} -%}
  {%- set column_name_to_quoted = {} -%}
  {%- for column in columns_in_relation -%}
  {%-   do column_name_to_data_types.update({column.name|lower: column.data_type}) -%}
  {%-   do column_name_to_quoted.update({column.name|lower: column.quoted}) -%}
  {%- endfor -%}

  {%- set expected_column_names_quoted = [] -%}
  {%- for column_name in tested_expected_column_names -%}
  {%-   do expected_column_names_quoted.append(column_name_to_quoted[column_name|lower]) -%}
  {%- endfor -%}

  {% if not expected_sql %}
  {%   set expected_sql = get_expected_sql(expected_rows, column_name_to_data_types, column_name_to_quoted) %}
  {% endif %}
  {% set unit_test_sql = get_unit_test_sql(sql, expected_sql, expected_column_names_quoted) %}

  {% call statement('main', fetch_result=True) -%}

    {{ unit_test_sql }}

  {%- endcall %}

  {% do adapter.drop_relation(temp_relation) %}

  {{ return({'relations': relations}) }}

{%- endmaterialization -%}
