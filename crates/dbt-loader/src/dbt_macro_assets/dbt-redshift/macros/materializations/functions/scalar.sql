{% macro redshift__scalar_function_create_replace_signature_sql(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
        RETURNS {{ model.returns.data_type }}
    {{ scalar_function_volatility_sql() }}
    AS
{% endmacro %}

{% macro redshift__formatted_scalar_function_args_sql() %}
    {% set args = [] %}
    {% for arg in model.arguments -%}
        {%- do args.append(arg.data_type) -%}
    {%- endfor %}
    {{ args | join(', ') }}
{% endmacro %}

{% macro redshift__scalar_function_volatility_sql() %}
    {% set volatility = model.config.get('volatility') %}
    {% if volatility == 'deterministic' %}
        IMMUTABLE
    {% elif volatility == 'stable' %}
        STABLE
    {% elif volatility == 'non-deterministic' or volatility == none %}
        VOLATILE
    {% else %}
        {% do unsupported_volatility_warning(volatility) %}
        {# We're ignoring the unknown volatility. But redshift requires a volatility to be set, so we default to VOLATILE #}
        VOLATILE
    {% endif %}
{% endmacro %}
