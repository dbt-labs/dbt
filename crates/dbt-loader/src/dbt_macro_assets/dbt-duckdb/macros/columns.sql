{% macro duckdb__alter_relation_add_remove_columns(relation, add_columns, remove_columns) %}

  {% if add_columns %}
    {% for column in add_columns %}
      {% set sql -%}
         alter {{ relation.type }} {{ relation }} add column
           {{ api.Relation.create(identifier=column.name) }} {{ column.data_type }}
      {%- endset -%}
      {% do run_query(sql) %}
    {% endfor %}
  {% endif %}

  {% if remove_columns %}
    {% for column in remove_columns %}
      {% set sql -%}
        alter {{ relation.type }} {{ relation }} drop column
          {{ api.Relation.create(identifier=column.name) }}
      {%- endset -%}
      {% do run_query(sql) %}
    {% endfor %}
  {% endif %}

{% endmacro %}
