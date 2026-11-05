{% macro trino__persist_docs(relation, model, for_relation, for_columns) -%}
  {% set do_relation = for_relation and config.persist_relation_docs() %}
  {% set do_columns = for_columns and config.persist_column_docs() %}

  {% if do_relation and model.description %}
    {% do run_query(alter_relation_comment(relation, model.description)) %}
  {% endif %}

  {% set documented_columns = {} %}
  {% if do_columns and model.columns %}
    {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
    {% set documented_columns = validate_doc_columns(relation, model.columns, existing_columns) %}
    {% set alter_comment_sql = alter_column_comment(relation, documented_columns) %}
    {% if alter_comment_sql and alter_comment_sql | trim | length > 0 %}
      {% do run_query(alter_comment_sql) %}
    {% endif %}
  {% endif %}

  {% if target.starburst_url %}
    {% set starburst_model = {"description": model.description, "columns": documented_columns} %}
    {% do adapter.persist_starburst_docs(relation, starburst_model, do_relation, do_columns) %}
  {% endif %}
{%- endmacro %}


{#--
  Override of the upstream validate_doc_columns helper, which is not dispatched
  per adapter. Trino lowercases every identifier, quoted ones included, so names
  are always compared case-insensitively; the upstream helper's case-sensitive
  match for quoted columns would never find a quoted mixed-case column.
--#}
{% macro validate_doc_columns(relation, column_dict, existing_column_names) %}
  {% set existing_lower = existing_column_names | map("lower") | list %}
  {% set missing = [] %}
  {% set filtered = {} %}
  {% for col_name in column_dict %}
    {% if col_name | lower in existing_lower %}
      {% do filtered.update({col_name | lower: column_dict[col_name]}) %}
    {% else %}
      {% do missing.append(col_name) %}
    {% endif %}
  {% endfor %}
  {% if missing | length > 0 %}
    {{ exceptions.warn("In relation " ~ relation.render() ~ ": The following columns are specified in the schema but are not present in the database: " ~ missing | join(", ")) }}
  {% endif %}
  {{ return(filtered) }}
{% endmacro %}
