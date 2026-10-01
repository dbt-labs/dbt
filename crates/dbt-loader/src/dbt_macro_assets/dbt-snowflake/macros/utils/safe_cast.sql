-- funcsign: (string, string) -> string
{% macro snowflake__safe_cast(field, type) %}
    {% if type|upper == "GEOMETRY" -%}
        try_to_geometry({{field}})
    {% elif type|upper == "GEOGRAPHY" -%}
        try_to_geography({{field}})
    {% elif type|upper != "VARIANT" -%}
        {#-- Snowflake try_cast does not support casting to variant, and expects the field as a string --#}
        {% set field_as_string =  dbt.string_literal(field) if field is number else field %}
        try_cast({{field_as_string}} as {{type}})
    {% else -%}
        {{ adapter.dispatch('cast', 'dbt')(field, type) }}
    {% endif -%}
{% endmacro %}
