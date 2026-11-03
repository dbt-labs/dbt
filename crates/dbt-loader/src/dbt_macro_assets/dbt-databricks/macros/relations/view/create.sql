{% macro databricks__create_view_as(relation, sql) %}
  {% if column_mask_exists() %}
    {% do exceptions.raise_compiler_error("Column masks are not supported for views.") %}
  {% endif %}
  {% if row_filter_exists() %}
    {% do exceptions.raise_compiler_error("Row filters are not supported for views.") %}
  {% endif %}
  {{ log("Creating view " ~ relation) }}
  create or replace view {{ relation.render() }}
  {%- if config.persist_column_docs() -%}
    {%- set model_columns = model.columns -%}
    {%- set query_columns = get_columns_in_query(sql) -%}
    {%- if query_columns %}
  (
    {{ get_persist_docs_column_list(relation, model_columns, query_columns) }}
  )
    {%- endif -%}
  {%- endif %}
  {{ comment_clause() }}
  {%- set contract_config = config.get('contract') -%}
  {%- if contract_config and contract_config.enforced %}
  {{ get_assert_columns_equivalent(sql) }}
  {%- endif -%}
  {{ tblproperties_clause() }}
  as (
    {{ sql }}
  )
{% endmacro %}


{% macro get_column_comment_sql(column_name, column_dict) -%}
  {%- if column_name in column_dict and column_dict[column_name]["description"] -%}
    {%- set escaped_description = column_dict[column_name]["description"] | replace("'", "\\'") -%}
    {%- set column_comment_clause = "comment '" ~ escaped_description ~ "'" -%}
    {{ adapter.quote(column_name) }} {{ column_comment_clause }}
  {%- else -%}
    {{ adapter.quote(column_name) }}
  {%- endif -%}
{%- endmacro %}

{% macro get_persist_docs_column_list(relation, model_columns, query_columns) -%}
  {#- DIVERGENCE BEGIN: fs retains the `relation` arg and calls validate_doc_columns to warn about model columns absent from the view query. -#}
  {#- Warns about any model_columns absent from the view's own query output.  -#}
  {%- do validate_doc_columns(relation, model_columns, query_columns, case_insensitive=true) -%}
  {#- DIVERGENCE END #}
  {%- for column_name in query_columns -%}
    {{ get_column_comment_sql(column_name, model_columns) }}{{",\n\t" if not loop.last else "" }}
  {%- endfor -%}
{% endmacro %}