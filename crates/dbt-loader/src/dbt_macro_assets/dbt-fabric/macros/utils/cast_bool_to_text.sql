{% macro fabric__cast_bool_to_text(field) %}
    case {{ field }}
        when 1 then 'true'
        when 0 then 'false'
        else null
    end
{% endmacro %}
