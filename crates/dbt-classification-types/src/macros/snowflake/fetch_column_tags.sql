{#
  Fetch all column-level Snowflake tags for a single table.
  Returns a result table with columns: column_name (TEXT), tag_database (TEXT),
  tag_schema (TEXT), tag_name (TEXT), tag_value (TEXT). The tag_database/tag_schema
  columns let Phase 3 reconstruct the tag's fully-qualified name so the write-back
  targets the tag where it actually lives (which may differ from the model's schema).

  Args:
    database — Snowflake database name (e.g. "raw")
    schema   — Snowflake schema name   (e.g. "public")
    table    — Snowflake table name    (e.g. "users")

  See propagation_of_snowflake_tags.md §4.1.
#}
{% macro fetch_snowflake_column_tags(database, schema, table) %}
  {% call statement('fetch_snowflake_column_tags', fetch_result=true, auto_begin=false) %}
    SELECT column_name, tag_database, tag_schema, tag_name, tag_value
    FROM TABLE(
      {{ database }}.INFORMATION_SCHEMA.TAG_REFERENCES_ALL_COLUMNS(
        '{{ database }}.{{ schema }}.{{ table }}', 'TABLE'
      )
    )
  {% endcall %}
  {% do return(load_result('fetch_snowflake_column_tags').table) %}
{% endmacro %}
