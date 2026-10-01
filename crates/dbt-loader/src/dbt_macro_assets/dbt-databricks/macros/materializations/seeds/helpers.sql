{% macro databricks__get_binding_char() %}
  {{ return('%s') }}
{% endmacro %}

{% macro databricks__load_csv_rows(model, agate_table) %}

  {% set batch_size = get_batch_size() %}
  {% set column_override = model['config'].get('column_types', {}) %}
  {% set must_cast = model['config'].get('file_format', 'delta') == 'parquet' %}

  {% set statements = [] %}

  {% for chunk in agate_table.rows | batch(batch_size) %}
      {% set bindings = [] %}

      {% for row in chunk %}
          {% do bindings.extend(row) %}
      {% endfor %}

      {% set sql %}
          insert {% if loop.index0 == 0 -%} overwrite {% else -%} into {% endif -%} {{ this.render() }} values
          {% for row in chunk -%}
              ({%- for col_name in agate_table.column_names -%}
                  {%- if must_cast -%}
                    {%- set inferred_type = adapter.convert_type(agate_table, loop.index0) -%}
                    {%- set type = column_override.get(col_name, inferred_type) -%}
                    cast({{ get_binding_char() }} as {{type}})
                  {%- else -%}
                    {{ get_binding_char() }}
                  {%- endif -%}
                  {%- if not loop.last%},{%- endif %}
              {%- endfor -%})
              {%- if not loop.last%},{%- endif %}
          {%- endfor %}
      {% endset %}

      {% do adapter.add_query(sql, bindings=bindings, abridge_sql_log=True, close_cursor=True) %}

      {% if loop.index0 == 0 %}
          {% do statements.append(sql) %}
      {% endif %}
  {% endfor %}

  {# Return SQL so we can render it out into the compiled files #}
  {{ return(statements[0]) }}
{% endmacro %}

{% macro databricks__reset_csv_table(model, full_refresh, old_relation, agate_table) %}
    {% if old_relation %}
      {% if old_relation.can_be_replaced and adapter.resolve_file_format(config) in ('delta', 'iceberg') %}
        {% set sql = create_or_replace_csv_table(model, agate_table, True) %}
      {% else %}
        {{ adapter.drop_relation(old_relation) }}
        {% set sql = create_csv_table(model, agate_table) %}
      {% endif %}
    {% else %}
      {% set sql = create_csv_table(model, agate_table) %}
    {% endif %}
    {{ return(sql) }}
{% endmacro %}

{% macro create_or_replace_csv_table(model, agate_table, replace=False) %}

  {%- set catalog_relation = adapter.build_catalog_relation(config.model) -%}

  {%- set column_override = model['config'].get('column_types', {}) -%}
  {%- set quote_seed_column = model['config'].get('quote_columns', None) -%}
  {%- set column_comment = config.persist_column_docs() and model.columns %}
  {%- set identifier = model['alias'] -%}
  {%- set relation = api.Relation.create(database=database, schema=schema, identifier=identifier, type='table') -%}
  {%- set replace_clause = "" -%}
  {%- if replace -%}
    {%- set replace_clause = "or replace" -%}
  {%- endif -%}

  {% set sql %}
    create {{replace_clause}} table {{ this.render() }} (
        {%- for col_name in agate_table.column_names -%}
            {%- set inferred_type = adapter.convert_type(agate_table, loop.index0) -%}
            {%- set type = column_override.get(col_name, inferred_type) -%}
            {%- set column_name = (col_name | string) -%}
            {%- set column_comment_clause = "" -%}
            {%- if column_comment and col_name in model.columns.keys() -%}   
              {%- set comment = model.columns[col_name]['description'] | replace("'", "\\'") -%}
              {%- if comment and comment != "" -%}
                {%- set column_comment_clause = "comment '" ~ comment ~ "'" -%}
              {%- endif -%}
            {%- endif -%}
            {{ adapter.quote_seed_column(column_name, quote_seed_column) }} {{ type }} {{ column_comment_clause }}{%- if not loop.last -%}, {%- endif -%}
        {%- endfor -%}
    )
    {{ file_format_clause(catalog_relation) }}
    {{ partition_cols(label="partitioned by") }}
    {{ clustered_cols(label="clustered by") }}
    {{ location_clause(catalog_relation) }}
    {{ comment_clause() }}
    {{ tblproperties_clause() }}
  {% endset %}

  {% call statement('_') -%}
    {{ sql }}
  {%- endcall %}

  {{ return(sql) }}
{% endmacro %}

{% macro databricks__create_csv_table(model, agate_table) %}
  {{ return(create_or_replace_csv_table(model, agate_table)) }}
{% endmacro %}

{% macro log_seed_operation(agate_table, full_refresh_mode, create_table_sql, sql) %}
  {% set code = 'CREATE' if full_refresh_mode else 'INSERT' %}
  {% set rows_affected = (agate_table.rows | length) %}

  {% call noop_statement('main', code ~ ' ' ~ rows_affected, code, rows_affected) %}
    {{ get_csv_sql(create_table_sql, sql) }};
  {% endcall %}
{% endmacro %}