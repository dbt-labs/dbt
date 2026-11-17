{#
   SingleStore persist_docs implementation.
   - ALTER TABLE <table_name> COMMENT = '<comment>' sets relation-level comments on base tables.
   - SingleStore views do not support comments (Error 1347: not a BASE TABLE); relation and column
     comments on views are safely skipped.
   - Column comments on columnstore and rowstore tables require ALTER TABLE <table_name> MODIFY COLUMN
     with the exact column type and nullability, otherwise SingleStore rejects the alter (Error 1890).
     Multiple column comments are grouped into a single comma-separated ALTER TABLE statement.
   - Upstream reference: https://github.com/memsql/dbt-singlestore/blob/master/dbt/include/singlestore/macros/materializations/view.sql#L74
#}

-- funcsign: (relation, optional[string]) -> string
{% macro singlestore__alter_relation_comment(relation, relation_comment) -%}
  {%- if relation.is_table -%}
    alter table {{ relation.render() }} comment = '{{ escape_single_quotes(relation_comment) }}'
  {%- endif -%}
{%- endmacro %}

-- funcsign: (relation, dict[string, api.column]) -> string
{% macro singlestore__alter_column_comment(relation, column_dict) -%}
  {%- if relation.is_table -%}
    {%- set target_schema = relation.database | default(relation.schema, true) | trim -%}
    {%- if not target_schema -%}
      {%- set target_schema = relation.schema | trim -%}
    {%- endif -%}
    {%- set modify_clauses = [] -%}
    {%- if execute -%}
      {%- call statement('get_column_defs_for_comment', fetch_result=True) -%}
        select
          column_name,
          column_type,
          case when is_nullable = 'NO' then 'not null' else 'null' end as nullability,
          column_default,
          data_type
        from information_schema.columns
        where table_name = '{{ escape_single_quotes(relation.identifier) }}'
          {% if target_schema %}
          and table_schema = '{{ escape_single_quotes(target_schema) }}'
          {% endif %}
        order by ordinal_position
      {%- endcall -%}
      {%- set cols_res = load_result('get_column_defs_for_comment') -%}
      {%- if cols_res and cols_res.table -%}
        {%- set numeric_types = ['tinyint', 'smallint', 'mediumint', 'int', 'integer', 'bigint', 'decimal', 'dec', 'numeric', 'float', 'double', 'double precision', 'real', 'bit', 'boolean', 'bool'] -%}
        {%- for row in cols_res.table -%}
          {%- set col_name = row[0] -%}
          {%- set col_type = row[1] -%}
          {%- set col_null = row[2] -%}
          {%- set col_default = row[3] -%}
          {%- set col_data_type = row[4] | default('', true) | lower | trim -%}
          {%- if col_name in column_dict and column_dict[col_name]['description'] -%}
            {%- set col_comment = escape_single_quotes(column_dict[col_name]['description']) -%}
            {%- set default_clause = '' -%}
            {%- if col_default is not none -%}
              {%- set default_str = col_default | string -%}
              {%- set default_upper = default_str | upper | trim -%}
              {%- if default_upper.startswith('CURRENT_TIMESTAMP') or default_upper.startswith('NOW()') or default_upper in ['LOCALTIMESTAMP', 'LOCALTIMESTAMP()'] -%}
                {%- set default_clause = ' default ' ~ default_str -%}
              {%- elif col_data_type in numeric_types -%}
                {%- set default_clause = ' default ' ~ default_str -%}
              {%- else -%}
                {%- set default_clause = " default '" ~ escape_single_quotes(default_str) ~ "'" -%}
              {%- endif -%}
            {%- endif -%}
            {%- do modify_clauses.append('modify column ' ~ adapter.quote(col_name) ~ ' ' ~ col_type ~ ' ' ~ col_null ~ default_clause ~ " comment '" ~ col_comment ~ "'") -%}
          {%- endif -%}
        {%- endfor -%}
      {%- endif -%}
    {%- endif -%}
    {%- if modify_clauses | length > 0 -%}
      alter table {{ relation.render() }} {{ modify_clauses | join(', ') }}
    {%- endif -%}
  {%- endif -%}
{%- endmacro %}

-- funcsign: (relation, model, optional[bool], optional[bool]) -> string
{% macro singlestore__persist_docs(relation, model, for_relation=true, for_columns=true) -%}
  {# SingleStore only supports comments on base tables; views return Error 1347: not a BASE TABLE #}
  {% if relation.is_table %}
    {% if for_relation and config.persist_relation_docs() and model.description %}
      {% set alter_rel_sql = alter_relation_comment(relation, model.description) %}
      {% if alter_rel_sql and alter_rel_sql | trim | length > 0 %}
        {% do run_query(alter_rel_sql) %}
      {% endif %}
    {% endif %}

    {% if for_columns and config.persist_column_docs() and model.columns %}
      {% set existing_columns = adapter.get_columns_in_relation(relation) | map(attribute="name") | list %}
      {% set filtered_columns = validate_doc_columns(relation, model.columns, existing_columns) %}
      {% set alter_col_sql = alter_column_comment(relation, filtered_columns) %}
      {% if alter_col_sql and alter_col_sql | trim | length > 0 %}
        {% do run_query(alter_col_sql) %}
      {% endif %}
    {% endif %}
  {% endif %}
{%- endmacro %}
