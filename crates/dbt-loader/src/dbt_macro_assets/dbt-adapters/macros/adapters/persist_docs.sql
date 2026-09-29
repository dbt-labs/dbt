-- funcsign: (relation, dict[string, api.column]) -> string
{% macro alter_column_comment(relation, column_dict) -%}
  {{ return(adapter.dispatch('alter_column_comment', 'dbt')(relation, column_dict)) }}
{% endmacro %}

-- funcsign: (relation, dict[string, api.column]) -> string
{% macro default__alter_column_comment(relation, column_dict) -%}
  {{ exceptions.raise_not_implemented(
    'alter_column_comment macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}


-- funcsign: (relation, string) -> string
{% macro alter_relation_comment(relation, relation_comment) -%}
  {{ return(adapter.dispatch('alter_relation_comment', 'dbt')(relation, relation_comment)) }}
{% endmacro %}

-- funcsign: (relation, string) -> string
{% macro default__alter_relation_comment(relation, relation_comment) -%}
  {{ exceptions.raise_not_implemented(
    'alter_relation_comment macro not implemented for adapter '+adapter.type()) }}
{% endmacro %}


-- funcsign: (relation, model, optional[bool], optional[bool]) -> string
{% macro persist_docs(relation, model, for_relation=true, for_columns=true) -%}
  {{ return(adapter.dispatch('persist_docs', 'dbt')(relation, model, for_relation, for_columns)) }}
{% endmacro %}

{#-- Validates documented columns against the actual database columns. Warns about any columns in column_dict that don't exist in existing_column_names. 
      Returns a filtered column_dict containing only columns that exist.--#}
-- funcsign: (relation, dict[string, api.column], list[string], optional[bool]) -> dict[string, api.column]
{% macro validate_doc_columns(relation, column_dict, existing_column_names, case_insensitive=false) %}
  {% set existing_lower = existing_column_names | map("lower") | list %}
  {% set missing = [] %}
  {% set filtered = {} %}
  {% for col_name in column_dict %}
    {% set is_quoted = column_dict[col_name]['quote'] %}
    {% if case_insensitive or not is_quoted %}
      {% set present = col_name | lower in existing_lower %}
    {% else %}
      {% set present = col_name in existing_column_names %}
    {% endif %}
    {% if present %}
      {% do filtered.update({col_name: column_dict[col_name]}) %}
    {% else %}
      {% do missing.append(col_name) %}
    {% endif %}
  {% endfor %}
  {% if missing | length > 0 %}
    {{ exceptions.warn("In relation " ~ relation.render() ~ ": The following columns are specified in the schema but are not present in the database: " ~ missing | join(", ")) }}
  {% endif %}
  {{ return(filtered) }}
{% endmacro %}

-- funcsign: (relation, model, optional[bool], optional[bool]) -> string
{% macro default__persist_docs(relation, model, for_relation, for_columns) -%}
  {% if for_relation and config.persist_relation_docs() and model.description %}
    {% do run_query(alter_relation_comment(relation, model.description)) %}
  {% endif %}

  {% if for_columns and config.persist_column_docs() and model.columns %}
    {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
    {% set filtered_columns = validate_doc_columns(relation, model.columns, existing_columns) %}
    {% set alter_comment_sql = alter_column_comment(relation, filtered_columns) %}
    {% if alter_comment_sql and alter_comment_sql | trim | length > 0 %}
      {% do run_query(alter_comment_sql) %}
    {% endif %}
  {% endif %}
{% endmacro %}
