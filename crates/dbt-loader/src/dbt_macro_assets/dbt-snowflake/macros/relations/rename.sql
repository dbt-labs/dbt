{% macro snowflake__rename_relation(from_relation, to_relation) -%}
  {% call statement('rename_relation') -%}
    alter table {{ from_relation }} rename to {{ to_relation }}
  {%- endcall %}
{% endmacro %}

-- funcsign: (relation, string) -> string
{% macro snowflake__get_rename_sql(relation, new_name) %}

    {% if relation.is_interactive_table %}
        {{ snowflake__get_rename_interactive_table_sql(relation, new_name) }}

    {% else %}
        {{ default__get_rename_sql(relation, new_name) }}

    {% endif %}

{% endmacro %}
