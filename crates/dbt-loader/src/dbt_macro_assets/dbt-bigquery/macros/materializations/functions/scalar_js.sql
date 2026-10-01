{% macro bigquery__scalar_function_volatility_javascript() %}
    {% set volatility = model.config.get('volatility') %}
     {% if volatility == 'deterministic' %}
        DETERMINISTIC
    {% elif volatility == 'non-deterministic' %}
        NOT DETERMINISTIC
    {% elif volatility != none %}
        {# volatility='stable' is not supported in big query #}
        {% do unsupported_volatility_warning(volatility) %}
    {% endif %}
{% endmacro %}

{% macro bigquery__scalar_function_create_replace_signature_javascript(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
    RETURNS {{ model.returns.data_type }}
    {{ scalar_function_volatility_javascript() }}
    LANGUAGE js
    AS
{% endmacro %}


{% macro bigquery__scalar_function_javascript(target_relation) %}
    {{ bigquery__scalar_function_create_replace_signature_javascript(target_relation) }}
    {{ bigquery__get_function_body() }}
{% endmacro %}
