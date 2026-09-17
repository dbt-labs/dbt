{% macro sync_column_schemas(on_schema_change, target_relation, schema_changes_dict) %}
  {%- set partitioned_by = config.get('partitioned_by', default=none) -%}
  {% set table_type = resolve_table_type() %}
  {%- if partitioned_by is none -%}
      {%- set partitioned_by = [] -%}
  {%- endif %}
  {%- set add_to_target_arr = schema_changes_dict['source_not_in_target'] -%}
  {%- if on_schema_change == 'append_new_columns'-%}
     {%- if add_to_target_arr | length > 0 -%}
       {%- do alter_relation_add_columns(target_relation, add_to_target_arr, table_type) -%}
     {%- endif -%}
  {% elif on_schema_change == 'sync_all_columns' %}
     {%- set remove_from_target_arr = schema_changes_dict['target_not_in_source'] -%}
     {%- set new_target_types = schema_changes_dict['new_target_types'] -%}
     {% if table_type == 'iceberg' %}
       {#
          If last run of alter_column_type was failed on rename tmp column to origin.
          Do rename to protect origin column from deletion and losing data.
       #}
       {% for remove_col in remove_from_target_arr if remove_col.column.endswith('__dbt_alter') %}
         {%- set origin_col_name = remove_col.column | replace('__dbt_alter', '') -%}
         {% for add_col in add_to_target_arr if add_col.column == origin_col_name %}
             {%- do alter_relation_rename_column(target_relation, remove_col.name, add_col.name, add_col.data_type, table_type) -%}
             {%- do remove_from_target_arr.remove(remove_col) -%}
             {%- do add_to_target_arr.remove(add_col) -%}
         {% endfor %}
       {% endfor %}

       {% if add_to_target_arr | length > 0 %}
         {%- do alter_relation_add_columns(target_relation, add_to_target_arr, table_type) -%}
       {% endif %}
       {% if remove_from_target_arr | length > 0 %}
         {%- do alter_relation_drop_columns(target_relation, remove_from_target_arr) -%}
       {% endif %}
       {% if new_target_types != [] %}
         {% for ntt in new_target_types %}
           {% set column_name = ntt['column_name'] %}
           {% set new_type = ntt['new_type'] %}
           {% do alter_column_type(target_relation, column_name, new_type) %}
         {% endfor %}
       {% endif %}
     {% else %}
       {%- set replace_with_target_arr = remove_partitions_from_columns(schema_changes_dict['source_columns'], partitioned_by) -%}
       {% if add_to_target_arr | length > 0 or remove_from_target_arr | length > 0 or new_target_types | length > 0 %}
         {%- do alter_relation_replace_columns(target_relation, replace_with_target_arr, table_type) -%}
       {% endif %}
     {% endif %}
  {% endif %}
  {% set schema_change_message %}
    In {{ target_relation }}:
        Schema change approach: {{ on_schema_change }}
        Columns added: {{ add_to_target_arr }}
        Columns removed: {{ remove_from_target_arr }}
        Data types changed: {{ new_target_types }}
  {% endset %}
  {% do log(schema_change_message) %}
{% endmacro %}

{% macro athena__alter_column_type(relation, column_name, new_column_type) -%}
  {#
    1. Create a new column (w/ temp name and correct type)
    2. Copy data over to it
    3. Drop the existing column
    4. Rename the new column to existing column
  #}
  {%- set table_type = resolve_table_type() -%}
  {%- set tmp_column = column_name + '__dbt_alter' -%}
  {%- set new_ddl_data_type = ddl_data_type(new_column_type, table_type) -%}

  {#- do alter_relation_add_columns(relation, [ tmp_column ], table_type) -#}
  {%- set add_column_query -%}
    alter {{ relation.type }} {{ relation.render_pure() }} add columns({{ tmp_column }} {{ new_ddl_data_type }});
  {%- endset -%}
  {%- do run_query(add_column_query) -%}

  {%- set update_query -%}
    update {{ relation.render_pure() }} set {{ tmp_column }} = cast({{ column_name }} as {{ new_column_type }});
  {%- endset -%}
  {%- do run_query(update_query) -%}

  {#- do alter_relation_drop_columns(relation, [ column_name ]) -#}
  {%- set drop_column_query -%}
    alter {{ relation.type }} {{ relation.render_pure() }} drop column {{ column_name }};
  {%- endset -%}
  {%- do run_query(drop_column_query) -%}

  {%- do alter_relation_rename_column(relation, tmp_column, column_name, new_column_type, table_type) -%}

{% endmacro %}
