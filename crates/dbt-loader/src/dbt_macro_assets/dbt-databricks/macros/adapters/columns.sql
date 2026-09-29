{% macro get_columns_comments(relation) -%}
  {{ return(run_query_as(get_columns_comments_sql(relation), 'get_columns_comments')) }}
{% endmacro %}

{% macro get_columns_comments_sql(relation) %}
DESCRIBE TABLE {{ relation.render() }}
{% endmacro %}

{% macro get_columns_comments_as_json(relation) -%}
  {{ return(run_query_as(get_columns_comments_as_json_sql(relation), 'get_columns_comments_as_json')) }}
{% endmacro %}

{% macro get_columns_comments_as_json_sql(relation) %}
  DESCRIBE TABLE EXTENDED {{ relation.render() }} AS JSON
{% endmacro %}

{% macro databricks__alter_relation_add_remove_columns(relation, add_columns, remove_columns) %}
  {% if remove_columns %}
    {{ unset_column_tags(relation, remove_columns) }}
    {{ run_query_as(drop_columns_sql(relation, remove_columns), 'alter_relation_remove_columns', fetch_result=False) }}
  {% endif %}

  {% if add_columns %}
    {{ run_query_as(add_columns_sql(relation, add_columns), 'alter_relation_add_columns', fetch_result=False) }}
  {% endif %}
{% endmacro %}

{% macro drop_columns_sql(relation, remove_columns) %}
ALTER TABLE {{ relation.render() }} DROP COLUMNS ({{ api.Column.format_remove_column_list(remove_columns) }})
{% endmacro %}

{% macro add_columns_sql(relation, add_columns) %}
ALTER TABLE {{ relation.render() }} ADD COLUMNS ({{ api.Column.format_add_column_list(add_columns) }})
{% endmacro %}


{% macro databricks__alter_column_type(relation, column_name, new_column_type) -%}
  {% call statement('alter_column_type') %}
    alter table {{ relation }} alter column {{ adapter.quote(column_name) }} type {{ new_column_type }};
  {% endcall %}
{% endmacro %}
