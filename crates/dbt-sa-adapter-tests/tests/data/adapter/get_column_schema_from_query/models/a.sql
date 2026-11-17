{% set sql = get_sql() %}

-- this invokes adapter.get_column_schema_from_query under the hood
-- this returns result type as the adapter.get_columns_in_relation
{% set columns = get_column_schema_from_query(sql) %}
{% for col in columns %}
    {% do log("Column: " ~ col.name, info=True) %}
    {% do log("Column data_type: " ~ col.data_type, info=True) %}
    {% do log("Column dtype: " ~ col.dtype, info=True) %}
    {% do log("Column numeric_precision: " ~ col.numeric_precision, info=True) %}
    {% do log("Column numeric_scale: " ~ col.numeric_scale, info=True) %}
    {% do log("Column char_size: " ~ col.char_size, info=True) %}
    {% do log("Column quoted: " ~ col.quoted, info=True) %}
{% endfor %}

select {{ dbt.string_literal(data_type) }} as data_type
