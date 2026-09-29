{% macro exasol__current_timestamp_backcompat() %}
    {# Exasol has no `::` cast operator; the default's current_timestamp::timestamp fails. #}
    cast(current_timestamp as timestamp)
{% endmacro %}

{% macro exasol__current_timestamp_in_utc_backcompat() %}
    {# Exasol has no AT TIME ZONE conversion of CURRENT_TIMESTAMP; like the
       Python dbt-exasol adapter, return the session-local timestamp. #}
    cast(current_timestamp as timestamp)
{% endmacro %}

{% macro exasol__current_timestamp() -%}
    current_timestamp
{%- endmacro %}

{% macro exasol__snapshot_get_time() -%}
    {# cast so 0-row schema detection resolves a dtype matching dbt_valid_from/to #}
    cast(current_timestamp as timestamp)
{%- endmacro %}
