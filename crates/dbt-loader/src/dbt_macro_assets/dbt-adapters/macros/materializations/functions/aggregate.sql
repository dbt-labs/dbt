--- get signature dispatch + default macro ---

{% macro get_aggregate_function_create_replace_signature(target_relation) %}
    {{ return(adapter.dispatch('get_aggregate_function_create_replace_signature', 'dbt')(target_relation)) }}
{% endmacro %}

{% macro default__get_aggregate_function_create_replace_signature(target_relation) %}
    CREATE OR REPLACE AGGREGATE FUNCTION {{ target_relation.render() }} ({{ get_formatted_aggregate_function_args()}})
    RETURNS {{ model.returns.data_type }}
    {{ get_function_language_specifier() }}
    {{ get_aggregate_function_volatility_specifier() }}
    {% if model.get('language') == 'python' %}
        {{ get_function_python_options() }}
    {% endif %}
    AS
{% endmacro %}

--- formatted args dispatch + default macro ---

{% macro get_formatted_aggregate_function_args() %}
    {{ return(adapter.dispatch('get_formatted_aggregate_function_args', 'dbt')()) }}
{% endmacro %}

{% macro default__get_formatted_aggregate_function_args() %}
    {# conveniently we can reuse the sql scalar function args #}
    {{ formatted_scalar_function_args_sql() }}
{% endmacro %}

--- function language specifier dispatch + default macro ---

{% macro get_function_language_specifier() %}
    {{ return(adapter.dispatch('get_function_language_specifier', 'dbt')()) }}
{% endmacro %}

{% macro default__get_function_language_specifier() %}
    {% set language = model.get('language') %}
    {% if language == 'sql' %}
        {# generally you dont need to specify the language for sql functions #}
    {% elif language == 'python' %}
        LANGUAGE PYTHON
    {% elif language == 'javascript' %}
        LANGUAGE js
    {% else %}
        {{ 'LANGUAGE ' ~ language.upper() }}
    {% endif %}
{% endmacro %}

--- function volatility specifier dispatch + default macro ---

{% macro get_aggregate_function_volatility_specifier() %}
    {{ return(adapter.dispatch('get_aggregate_function_volatility_specifier', 'dbt')()) }}
{% endmacro %}

{% macro default__get_aggregate_function_volatility_specifier() %}
    {{ scalar_function_volatility_sql() }}
{% endmacro %}

--- function python options + default macro ---

{% macro get_function_python_options() %}
    {{ return(adapter.dispatch('get_function_python_options', 'dbt')()) }}
{% endmacro %}

{% macro default__get_function_python_options() %}
    RUNTIME_VERSION = '{{ model.config.get('runtime_version') }}'
    HANDLER = '{{ model.config.get('entry_point') }}'
    {% set packages = model.config.get('packages', []) %}
    {% if packages %}
    PACKAGES = ('{{ packages | join("','") }}')
    {% endif %}
{% endmacro %}
