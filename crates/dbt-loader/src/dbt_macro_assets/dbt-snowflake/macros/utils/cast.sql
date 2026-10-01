-- funcsign: (string, string) -> string
{% macro snowflake__cast(field, type) %}
    {% if (type|upper == "GEOGRAPHY") -%}
        to_geography({{field}})
    {% elif (type|upper == "GEOMETRY") -%}
        to_geometry({{field}})
    {% else -%}
        cast({{field}} as {{type}})
    {% endif -%}
{% endmacro %}
