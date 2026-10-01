{% macro snowflake__scalar_function_create_replace_signature_sql(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
    RETURNS {{ model.returns.data_type }}
    LANGUAGE SQL
    {{ scalar_function_volatility_sql() }}
    AS
{% endmacro %}

{% macro snowflake__scalar_function_body_sql() %}
    $$
{{ model.compiled_code }}
    $$
{% endmacro %}

{% macro snowflake__formatted_scalar_function_args_sql() %}
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

{% macro snowflake__scalar_function_volatility_sql() %}
    {% set volatility = model.config.get('volatility') %}
    {% if volatility == 'deterministic' %}
        IMMUTABLE
    {% elif volatility == 'non-deterministic'%}
        VOLATILE
    {% elif volatility == 'stable' or volatility != none %}
        {% do unsupported_volatility_warning(volatility) %}
    {% endif %}
    {# If no volatility is set, don't add anything and let the data warehouse default it #}
{% endmacro %}

{% macro snowflake__scalar_function_create_replace_signature_python(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
    RETURNS {{ model.returns.data_type }}
    LANGUAGE PYTHON
    {{ scalar_function_volatility_sql() }}
    {{ get_function_python_options() }}
    AS
{% endmacro %}

{% macro snowflake__scalar_function_python(target_relation) %}
    {{ snowflake__scalar_function_create_replace_signature_python(target_relation) }}
    {{ scalar_function_body_sql() }}
{% endmacro %}

{% macro snowflake__formatted_scalar_function_args_javascript() %}
    {% set args = [] %}
    {% set should_quote = model.config.get('snowflake', {}).get('quote_args', true) %}
    {% for arg in model.arguments -%}
        {% set default_value = arg.get('default_value', none) %}
        {% set arg_name = adapter.quote(arg.name) if should_quote else arg.name %}
        {% if default_value != none %}
            {%- do args.append(arg_name ~ ' ' ~ arg.data_type ~ ' DEFAULT ' ~ default_value) -%}
        {% else %}
            {%- do args.append(arg_name ~ ' ' ~ arg.data_type) -%}
        {% endif %}
    {%- endfor %}
    {{ args | join(', ') }}
{% endmacro %}

{% macro snowflake__scalar_function_create_replace_signature_javascript(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_javascript()}})
    RETURNS {{ model.returns.data_type }}
    LANGUAGE JAVASCRIPT
    {{ scalar_function_volatility_sql() }}
    AS
{% endmacro %}


{% macro snowflake__scalar_function_javascript(target_relation) %}
    {{ snowflake__scalar_function_create_replace_signature_javascript(target_relation) }}
    {{ scalar_function_body_sql() }}
{% endmacro %}
