{#
  Apply a Snowflake tag to a single column on a materialized table or view.

  Args:
    relation_type — "TABLE" or "VIEW" (case-insensitive)
    database, schema, identifier — fully-qualified relation
    column_name — column to tag
    tag_fqn     — fully-qualified Snowflake tag name (db.schema.tag, or bare
                   name if it lives in the same schema as the relation)
    tag_value   — string value to assign

  See propagation_of_snowflake_tags.md §6.2.
#}
{% macro apply_snowflake_column_tag(
    relation_type, database, schema, identifier, column_name, tag_fqn, tag_value
) %}
  {% call statement('apply_snowflake_column_tag', fetch_result=false, auto_begin=false) %}
    ALTER {{ relation_type | upper }} {{ database }}.{{ schema }}.{{ identifier }}
      MODIFY COLUMN {{ column_name }}
      SET TAG {{ tag_fqn }} = '{{ tag_value | replace("'", "''") }}'
  {% endcall %}
{% endmacro %}
