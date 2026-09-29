{% macro one_alter_relation(relation, alter_comments) %}
  alter table {{ relation }} {{ on_cluster_clause(relation) }} {{ alter_comments }}
{% endmacro %}

{% macro one_alter_column_comment(relation, column_name, comment) %}
  alter table {{ relation }} {{ on_cluster_clause(relation) }} comment column `{{ column_name }}` '{{ comment }}'
{% endmacro %}

{% macro clickhouse__alter_relation_comment(relation, comment) %}
  alter table {{ relation }} {{ on_cluster_clause(relation) }} modify comment '{{ comment }}'
{% endmacro %}

{% macro clickhouse__persist_docs(relation, model, for_relation, for_columns) %}
  {%- set alter_comments = [] %}

  {%- if for_relation and config.persist_relation_docs() and model.description -%}
    {% set escaped_comment = clickhouse_escape_comment(model.description) %}
    {% do alter_comments.append("modify comment {comment}".format(comment=escaped_comment)) %}
  {%- endif -%}

  {%- if for_columns and config.persist_column_docs() and model.columns -%}
    {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
    {% do validate_doc_columns(relation, model.columns, existing_columns) %}
    {% for column_name in model.columns if (column_name in existing_columns) %}
      {%- set comment = model.columns[column_name]['description'] -%}
      {%- if comment %}
        {% set escaped_comment = clickhouse_escape_comment(comment) %}
        {% do alter_comments.append("comment column `{column_name}` {comment}".format(column_name=column_name, comment=escaped_comment)) %}
      {%- endif %}
    {%- endfor -%}
  {%- endif -%}

  {%- if alter_comments | length > 0 -%}
    {% do run_query(one_alter_relation(relation, alter_comments|join(', '))) %}
  {%- endif -%}
{% endmacro %}

{#
  By using dollar-quoting like this, users can embed anything they want into their comments
  (including nested dollar-quoting), as long as they do not use this exact dollar-quoting
  label. It would be nice to just pick a new one but eventually you do have to give up.
#}
{% macro clickhouse_escape_comment(comment) -%}
  {% if comment is not string %}
    {% do exceptions.raise_compiler_error('cannot escape a non-string: ' ~ comment) %}
  {% endif %}
  {%- set magic = '$dbt_comment_literal_block$' -%}
  {%- if magic in comment -%}
    {%- do exceptions.raise_compiler_error('The string ' ~ magic ~ ' is not allowed in comments.') -%}
  {%- endif -%}
  {{ magic }}{{ comment }}{{ magic }}
{%- endmacro %}
