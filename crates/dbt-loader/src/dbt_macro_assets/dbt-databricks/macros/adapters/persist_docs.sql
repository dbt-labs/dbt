{% macro databricks__alter_column_comment(relation, column_dict) %}
  {% set file_format = adapter.resolve_file_format(config) %}
  {% if file_format in ['delta', 'hudi'] %}
    {% for column in column_dict.values() %}
      {% set comment = column['description'] %}
      {% set escaped_comment = comment | replace('\'', '\\\'') %}
      {% set column_path = relation.render() ~ '.' ~ adapter.quote(column['name']) %}
      {{ run_query_as(comment_on_column_sql(column_path, escaped_comment), 'alter_column_comment', fetch_result=False) }}
    {% endfor %}
  {% else %}
    {{ log('WARNING - requested to update column comments, but file format ' ~ file_format ~ ' does not support that.') }}
  {% endif %}
{% endmacro %}

{% macro comment_on_column_sql(column_path, escaped_comment) %}
  {# DIVERGENCE: upstream uses adapter.has_dbr_capability('comment_on_column'); we use the
     platform-agnostic method has_feature. Gate on dbt_version so Fusion uses has_feature
     and dbt-core (1.x) uses the v1 method has_dbr_capability. #}
  {% if dbt_version.startswith('2.') %}
    {%- if adapter.has_feature('comment_on_column') -%}
      COMMENT ON COLUMN {{ column_path }} IS '{{ escaped_comment }}'
    {%- else -%}
      {{ alter_table_change_column_comment_sql(column_path, escaped_comment) }}
    {%- endif -%}
  {% else %}
    {%- if adapter.has_dbr_capability('comment_on_column') -%}
      COMMENT ON COLUMN {{ column_path }} IS '{{ escaped_comment }}'
    {%- else -%}
      {{ alter_table_change_column_comment_sql(column_path, escaped_comment) }}
    {%- endif -%}
  {% endif %}
{% endmacro %}

{% macro alter_table_change_column_comment_sql(column_path, escaped_comment) %}
  {%- set parts = column_path.split('.') -%}
  {%- if parts|length >= 4 -%}
    {%- set table_path = parts[:-1] | join('.') -%}
    {%- set column_name = parts[-1] -%}
    ALTER TABLE {{ table_path }} ALTER COLUMN {{ column_name }} COMMENT '{{ escaped_comment }}'
  {%- else -%}
    {{ exceptions.raise_compiler_error("Invalid column path: " ~ column_path ~ ". Expected format: database.schema.table.column") }}
  {%- endif -%}
{% endmacro %}

{% macro databricks__persist_docs(relation, model, for_relation, for_columns) -%}
  {%- if for_relation and config.persist_relation_docs() and model.description %}
    {{ run_query_as(alter_relation_comment_sql(relation, model.description), 'alter_relation_comment', fetch_result=False) }}
  {% endif %}
  {% if for_columns and config.persist_column_docs() and model.columns %}
    {#- DIVERGENCE BEGIN: This section diverges with our pin to dbt-databricks v1.12.4.
      It was added in Fusion in https://github.com/dbt-labs/fs/pull/12094, and to
      dbt-databricks in https://github.com/databricks/dbt-databricks/pull/1563.
      dbt-databricks will catch up with us in v1.13. -#}
    {%- set existing_columns = adapter.get_columns_in_relation(relation) -%}
    {%- set existing_column_names = existing_columns | map(attribute='name') | list -%}
    {%- set valid_columns = validate_doc_columns(relation, model.columns, existing_column_names, case_insensitive=true) -%}
    {#- DIVERGENCE END -#}
    {%- set columns_to_persist_docs = adapter.get_persist_doc_columns(existing_columns, valid_columns) -%}
    {{ alter_column_comment(relation, columns_to_persist_docs) }}
  {% endif %}
{% endmacro %}

{% macro alter_relation_comment_sql(relation, description) %}
{#- DIVERGENCE BEGIN: upstream uses relation.type.render(); we use render_type() Jinja macro instead -#}
COMMENT ON {{ render_type(relation.type) }} {{ relation.render() }} IS '{{ description | replace("'", "\\'") }}'
{#- DIVERGENCE END -#}
{% endmacro %}

{% macro alter_column_comments(relation, column_dict) %}
  {% for column, comment in column_dict.items() %}
    {{ log('Updating comment for column ' ~ column ~ ' with comment ' ~ comment) }}
    {% set escaped_comment = comment | replace('\'', '\\\'') %}
    {% set column_path = relation.render() ~ '.' ~ adapter.quote(column) %}
    {{ run_query_as(comment_on_column_sql(column_path, escaped_comment), 'main', fetch_result=False) }}
  {% endfor %}
{% endmacro %}