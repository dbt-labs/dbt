
{% macro duckdb__get_binding_char() %}
  {{ return(adapter.get_binding_char()) }}
{% endmacro %}

{% macro duckdb__get_batch_size() %}
  {{ return(10000) }}
{% endmacro %}

{# DIVERGENCE BEGIN - pipes in the all_varchar config for duckdb

   `all_varchar` and `column_types` are allowed together rather than erroring.
   `column_types` wins for the columns it names; `all_varchar` types every
   other column VARCHAR. This lets users start from all-VARCHAR and pick out
   the few columns that need a real type. #}
{% macro duckdb__create_csv_table(model, agate_table) %}
  {%- set column_override = model['config'].get('column_types', {}) -%}
  {%- set quote_seed_column = model['config'].get('quote_columns', None) -%}
  {%- set all_varchar = model['config'].get('all_varchar', false) -%}

  {% set sql %}
    create table {{ this.render() }} (
        {%- for col_name in agate_table.column_names -%}
            {%- set inferred_type = 'VARCHAR' if all_varchar else adapter.convert_type(agate_table, loop.index0) -%}
            {%- set type = column_override.get(col_name, inferred_type) -%}
            {%- set column_name = (col_name | string) -%}
            {{ adapter.quote_seed_column(column_name, quote_seed_column) }} {{ type }} {%- if not loop.last -%}, {%- endif -%}
        {%- endfor -%}
    )
  {% endset %}

  {% call statement('_') -%}
    {{ sql }}
  {%- endcall %}

  {{ return(sql) }}
{% endmacro %}
{# DIVERGENCE END #}

{% macro duckdb__load_csv_rows(model, agate_table) %}
    {% if config.get('fast', true) %}
        {% set seed_file_path = adapter.get_seed_file_path(model) %}
        {% set delimiter = config.get('delimiter', ',') %}
        {# DIVERGENCE BEGIN: Remote or sandboxed (e.g. MotherDuck with `saas_mode`)
	    DuckDb instances cannot read local files with `COPY`. So, Fusion
	    appends the seed's already-parsed rows over the connection
	    (Arrow bulk ingest) #}
        {% if dbt_version.startswith('2.') %}
            {% do adapter.load_dataframe(
                model['database'],
                model['schema'],
                model['alias'],
                seed_file_path,
                agate_table,
                config.get('column_types', {}),
                delimiter,
            ) %}
            {{ return('-- ' ~ (agate_table.rows | length) ~ ' rows appended to ' ~ this.render() ~ ' via Arrow bulk ingest') }}
        {% endif %}
        {# DIVERGENCE END #}
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
