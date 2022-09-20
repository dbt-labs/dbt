{% macro postgres__current_timestamp() -%}
    now()
{%- endmacro %}

{% macro postgres__snapshot_string_as_time(timestamp) -%}
    {%- set result = "'" ~ timestamp ~ "'::timestamp without time zone" -%}
    {{ return(result) }}
{%- endmacro %}

{% macro postgres__snapshot_get_time() -%}
    {{ current_timestamp() }} :: TIMESTAMP without TIME ZONE
{%- endmacro %}
