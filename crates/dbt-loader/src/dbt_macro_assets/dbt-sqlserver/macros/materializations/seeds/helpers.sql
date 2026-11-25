{% macro sqlserver__get_binding_char() %}
  {{ return('?') }}
{% endmacro %}

{% macro sqlserver__get_batch_size() %}
  {{ return(400) }}
{% endmacro %}

{% macro calc_batch_size(num_columns) %}
    {#
        SQL Server allows for a max of 2098 parameters in a single statement.
        Check if the max_batch_size fits with the number of columns, otherwise
        reduce the batch size so it fits.
    #}
    {% set max_batch_size = get_batch_size() %}
    {% set calculated_batch = (2098 / num_columns)|int %}
    {% set batch_size = [max_batch_size, calculated_batch] | min %}

    {{ return(batch_size) }}
{%  endmacro %}

{% macro sqlserver__load_csv_rows(model, agate_table) %}
  {% set cols_sql = get_seed_column_quoted_csv(model, agate_table.column_names) %}
  {% set batch_size = calc_batch_size(agate_table.column_names|length) %}
  {% set statements = [] %}

  {{ log("Inserting batches of " ~ batch_size ~ " records") }}

  {% for chunk in agate_table.rows | batch(batch_size) %}
      {% set bindings = [] %}
      {% set values_clause = [] %}

      {% for row in chunk %}
          {% set row_values = [] %}
          {% for column in agate_table.column_names %}
              {%- set val = row[loop.index0] -%}
              {%- if val is none -%}
                  {%- do row_values.append("null") -%}
              {%- else -%}
                  {%- do row_values.append(get_binding_char()) -%}
                  {%- do bindings.append(val) -%}
              {%- endif -%}
          {% endfor %}
          {% do values_clause.append("(" ~ row_values | join(", ") ~ ")") %}
      {% endfor %}

      {% set sql %}
          insert into {{ this.render() }} ({{ cols_sql }}) values {{ values_clause | join(", ") }}
      {% endset %}

      {% do adapter.add_query(sql, bindings=bindings, abridge_sql_log=True) %}

      {% if loop.index0 == 0 %}
          {% do statements.append(sql) %}
      {% endif %}
  {% endfor %}

  {# Return SQL so we can render it out into the compiled files #}
  {{ return(statements[0]) }}
{% endmacro %}
