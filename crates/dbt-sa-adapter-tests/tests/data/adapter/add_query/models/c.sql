{% set table_name = 'a' %}
{% set db_schema = t_schema_name() %}
{% set db_name = target.database %}

{% do set_up(db_schema, table_name) %}

{{ print(db_name) }}
{{ print(db_schema) }}

{% if supports_explicit_transactions() %}
    {% set _ = adapter.add_query(sql='begin', auto_begin=False) %}
    {% set _ = adapter.add_query(sql='select %s as id', auto_begin=False, bindings=[42]) %}
    {% set _ = adapter.add_query(sql='select %s as message', auto_begin=False, bindings=['42']) %}
    {% set _ = adapter.add_query(sql='commit', auto_begin=False) %}
{% else %}
    {% set _ = adapter.add_query(sql='select %s as id', auto_begin=False, bindings=[42]) %}
{% endif %}

SELECT 1 as one
