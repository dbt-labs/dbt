{% macro clickhouse__load_csv_rows(model, agate_table) %}
  {# Fusion divergence: literal-value INSERT with explicit escaping instead of v1's
     `adapter.get_csv_data` + `format CSV` + bindings, which v2 does not implement.
     Track: https://github.com/dbt-labs/dbt-fusion/pull/1710 #}
  {% set batch_size = get_batch_size() %}
  {% set cols_sql = get_seed_column_quoted_csv(model, agate_table.column_names) %}
  {% set statements = [] %}

  {% for chunk in agate_table.rows | batch(batch_size) %}
    {% set ns = namespace(row_strs=[]) %}
    {% for row in chunk %}
      {% set ns2 = namespace(vals=[]) %}
      {% for val in row %}
        {%- if val is none -%}
          {% do ns2.vals.append('NULL') %}
        {%- elif val is number -%}
          {% do ns2.vals.append(val | string) %}
        {%- else -%}
          {%- set escaped = val | string
              | replace("\\", "\\\\")
              | replace("'", "\\'") -%}
          {% do ns2.vals.append("'" ~ escaped ~ "'") %}
        {%- endif -%}
      {% endfor %}
      {% do ns.row_strs.append('(' ~ ns2.vals | join(', ') ~ ')') %}
    {% endfor %}

    {% set sql -%}
      INSERT INTO {{ this.render() }} ({{ cols_sql }}) VALUES
      {{ ns.row_strs | join(',\n      ') }}
    {%- endset %}

    {% do adapter.add_query(sql, abridge_sql_log=True) %}

    {% if loop.index0 == 0 %}
      {% do statements.append(sql) %}
    {% endif %}
  {% endfor %}

  {{ return(statements[0] if statements else '') }}
{% endmacro %}

{% macro clickhouse__create_csv_table(model, agate_table) %}
  {%- set column_override = model['config'].get('column_types', {}) -%}
  {%- set quote_seed_column = model['config'].get('quote_columns', None) -%}

  {% set sql %}
    create table {{ this.render() }} {{ on_cluster_clause(this) }} (
      {%- for col_name in agate_table.column_names -%}
        {%- set inferred_type = adapter.convert_type(agate_table, loop.index0) -%}
        {%- set type = column_override.get(col_name, inferred_type) -%}
        {%- set column_name = (col_name | string) -%}
          {{ adapter.quote_seed_column(column_name, quote_seed_column) }} {{ type }} {%- if not loop.last -%}, {%- endif -%}
      {%- endfor -%}
    )
    {{ engine_clause() }}
    {{ order_cols(label='order by') }}
    {{ partition_cols(label='partition by') }}
  {% endset %}

  {% call statement('_') -%}
    {{ sql }}
  {%- endcall %}

  {{ return(sql) }}
{% endmacro %}
