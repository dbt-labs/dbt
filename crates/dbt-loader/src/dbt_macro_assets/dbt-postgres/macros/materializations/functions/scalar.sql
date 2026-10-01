{% macro postgres__formatted_scalar_function_args_sql() %}
    {% set args = [] %}
    {% for arg in model.arguments -%}
        {% set default_value = arg.get('default_value', none) %}
        {% if default_value != none %}
            {%- do args.append(arg.name ~ ' ' ~ arg.data_type ~ ' DEFAULT ' ~ default_value) -%}
        {% else %}
            {%- do args.append(arg.name ~ ' ' ~ arg.data_type) -%}
        {% endif %}
    {%- endfor %}
    {{ args | join(', ') }}
{% endmacro %}
