
{% macro duckdb__get_binding_char() %}
  {{ return(adapter.get_binding_char()) }}
{% endmacro %}

{% macro duckdb__get_batch_size() %}
  {{ return(10000) }}
{% endmacro %}

{% macro duckdb__load_csv_rows(model, agate_table) %}
    {% if config.get('fast', true) %}
        {% set seed_file_path = adapter.get_seed_file_path(model) %}
        {% set delimiter = config.get('delimiter', ',') %}
        {% set sql %}
          COPY {{ this.render() }} FROM '{{ seed_file_path }}' (FORMAT CSV, HEADER TRUE, DELIMITER '{{ delimiter }}')
        {% endset %}
        {% do adapter.add_query(sql, abridge_sql_log=True) %}
        {{ return(sql) }}
    {% endif %}

    {% set batch_size = get_batch_size() %}
    {% set agate_table = adapter.convert_datetimes_to_strs(agate_table) %}
    {% set cols_sql = get_seed_column_quoted_csv(model, agate_table.column_names) %}
    {% set bindings = [] %}

    {% set statements = [] %}

    {% for chunk in agate_table.rows | batch(batch_size) %}
        {% set bindings = [] %}

        {% for row in chunk %}
            {% do bindings.extend(row) %}
        {% endfor %}

        {% set sql %}
            insert into {{ this.render() }} ({{ cols_sql }}) values
            {% for row in chunk -%}
                ({%- for column in agate_table.column_names -%}
                    {{ get_binding_char() }}
                    {%- if not loop.last%},{%- endif %}
                {%- endfor -%})
                {%- if not loop.last%},{%- endif %}
            {%- endfor %}
        {% endset %}

        {% do adapter.add_query(sql, bindings=bindings, abridge_sql_log=True) %}

        {% if loop.index0 == 0 %}
            {% do statements.append(sql) %}
        {% endif %}
    {% endfor %}

    {# Return SQL so we can render it out into the compiled files #}
    {{ return(statements[0]) }}
{% endmacro %}
