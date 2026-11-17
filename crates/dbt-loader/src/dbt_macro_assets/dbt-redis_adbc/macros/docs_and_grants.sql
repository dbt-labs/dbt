{#- persist_docs: dbt's defaults are deliberately unimplemented. The driver
    has COMMENT ON (tables, views and columns); the comments show up in the
    docs catalog. -#}
{% macro redis_adbc__alter_relation_comment(relation, relation_comment) -%}
  comment on {{ "view" if relation.type == "view" else "table" }} {{ relation }} is {{ redis_adbc__comment_literal(relation_comment) }};
{%- endmacro %}

{% macro redis_adbc__alter_column_comment(relation, column_dict) -%}
  {%- for column_name, column in column_dict.items() %}
  comment on column {{ relation }}.{{ adapter.quote(column_name) if column.get("quote") else column_name }} is {{ redis_adbc__comment_literal(column.get("description")) }};
  {%- endfor %}
{%- endmacro %}

{#- A standard SQL string literal ('' for '; backslashes are literal). An empty
    description removes the comment. -#}
{% macro redis_adbc__comment_literal(comment) -%}
  '{{ (comment or "") | replace("'", "''") }}'
{%- endmacro %}

{#- grants: Redis controls access per user with ACLs (key patterns and
    commands), not SQL privileges, and the driver has no SHOW GRANTS, GRANT
    or REVOKE. Skip with a warning rather than fail the model. -#}
{% macro redis_adbc__apply_grants(relation, grant_config, should_revoke=True) -%}
  {%- if grant_config -%}
    {%- do exceptions.warn("grants are skipped for " ~ relation ~ ": Redis controls access with ACLs, not GRANT") -%}
  {%- endif -%}
{%- endmacro %}
