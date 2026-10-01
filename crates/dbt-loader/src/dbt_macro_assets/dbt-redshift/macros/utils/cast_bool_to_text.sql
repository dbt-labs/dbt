{% macro redshift__cast_bool_to_text(field) %}
    case
        when {{ field }} is true then 'true'
        when {{ field }} is false then 'false'
    end::text
{% endmacro %}
