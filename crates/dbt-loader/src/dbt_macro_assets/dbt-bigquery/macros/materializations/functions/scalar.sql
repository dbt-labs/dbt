{% macro bigquery__scalar_function_create_replace_signature_sql(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
    RETURNS {{ model.returns.data_type }}
    {{ scalar_function_volatility_sql() }}
    AS
{% endmacro %}

{% macro bigquery__scalar_function_body_sql() %}
    (
       {{ model.compiled_code }}
    )
{% endmacro %}

{% macro bigquery__scalar_function_volatility_sql() %}
    {% set volatility = model.config.get('volatility') %}
    {% if volatility != None %}
        {% do unsupported_volatility_warning(volatility) %}
    {% endif %}
{% endmacro %}

{% macro bigquery__scalar_function_create_replace_signature_python(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
    RETURNS {{ model.returns.data_type }}
    LANGUAGE python
    {{ get_function_python_options() }}
    {{ scalar_function_volatility_sql() }}
    AS
{% endmacro %}

{% macro bigquery__get_function_python_options() %}
    OPTIONS(
        runtime_version = "{{ 'python-' ~ model.config.get('runtime_version') }}",
        entry_point = "{{ model.config.get('entry_point') }}"
        {%- if model.config.get('packages') %},
        packages = [{% for pkg in model.config.get('packages') %}'{{ pkg }}'{% if not loop.last %}, {% endif %}{% endfor %}]
        {%- endif %}
    )
{% endmacro %}

{% macro bigquery__get_function_body() %}
    r'''
{{ model.compiled_code }}
    '''
{% endmacro %}

{% macro bigquery__scalar_function_python(target_relation) %}
    {{ bigquery__scalar_function_create_replace_signature_python(target_relation) }}
    {{ bigquery__get_function_body() }}
{% endmacro %}
