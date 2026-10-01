{% macro alter_relation_add_columns(relation, add_columns = none, table_type = 'hive') -%}
  {% if add_columns is none %}
    {% set add_columns = [] %}
  {% endif %}

  {% set sql -%}
      alter {{ relation.type }} {{ relation.render_hive() }}
          add columns (
            {%- for column in add_columns -%}
                {{ column.quoted_hive }} {{ ddl_data_type(column.data_type, table_type) }}{{ ', ' if not loop.last }}
            {%- endfor -%}
          )
  {%- endset -%}

  {% if (add_columns | length) > 0 %}
    {{ return(run_query(sql)) }}
  {% endif %}
{% endmacro %}

{% macro alter_relation_drop_columns(relation, remove_columns = none) -%}
  {% if remove_columns is none %}
    {% set remove_columns = [] %}
  {% endif %}

  {%- for column in remove_columns -%}
    {% set sql -%}
      alter {{ relation.type }} {{ relation.render_hive() }} drop column {{ column.quoted_hive }}
    {% endset %}
    {% do run_query(sql) %}
  {%- endfor -%}
{% endmacro %}


{% macro alter_relation_replace_columns(relation, replace_columns = none, table_type = 'hive') -%}
  {% if replace_columns is none %}
    {% set replace_columns = [] %}
  {% endif %}

  {% set sql -%}
      alter {{ relation.type }} {{ relation.render_hive() }}
          replace columns (
            {%- for column in replace_columns -%}
                {{ column.quoted_hive }} {{ ddl_data_type(column.data_type, table_type) }}{{ ', ' if not loop.last }}
            {%- endfor -%}
          )
  {%- endset -%}

  {% if (replace_columns | length) > 0 %}
    {{ return(run_query(sql)) }}
  {% endif %}
{% endmacro %}

{% macro alter_relation_rename_column(relation, source_column, target_column, target_column_type, table_type = 'hive') -%}
  {% set sql -%}
      alter {{ relation.type }} {{ relation.render_pure() }}
          change column {{ source_column }} {{ target_column }} {{ ddl_data_type(target_column_type, table_type) }}
  {%- endset -%}
  {{ return(run_query(sql)) }}
{% endmacro %}
