
-- This is a workaround since the actual usage of adapter.convert_type is in the seed mat macro
-- and the agate_table is supposed to be loaded from the seed csv
-- but since run is not properly supported yet, we constructs a sql and use its result to test this method
{% set query %}
    {% if target.type == 'bigquery' %}
        select
            'hello' as text_col,
            1 as integer_col,
            1.0 as decimal_col,
            true as bool_col,
            datetime(2008, 12, 25, 05, 30, 00) as datetime_col
    {% elif target.type == 'databricks' %}
        select
            'hello' as text_col,
            1 as integer_col,
            true as bool_col
    {% elif target.type == 'postgres' %}
        select
            'hello' as text_col,
            1 as integer_col,
            1.0 as decimal_col,
            true as bool_col,
            timestamp '2021-07-01 08:43:28.123456' as datetime_col
    {% elif target.type == 'duckdb' %}
        select
            'hello' as text_col,
            1 as integer_col,
            1.0 as decimal_col,
            true as bool_col,
            timestamp '2021-07-01 08:43:28.123456' as datetime_col
    {% else %}
        {{ print('Add sql for ' ~ target.type) }}
    {% endif %}
{% endset %}

{% set table = run_query(query) %}
{% for col_name in table.column_names %}
    {% set inferred_type = adapter.convert_type(table, loop.index0) %}
    {{ print("column: " ~ col_name)}}
    {{ print(inferred_type) }}
{% endfor %}

select 1 as val