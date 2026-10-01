{% macro function_execute_build_sql(build_sql, existing_relation, target_relation) %}
    {{ return(adapter.dispatch('function_execute_build_sql', 'dbt')(build_sql, existing_relation, target_relation)) }}
{% endmacro %}

{% macro default__function_execute_build_sql(build_sql, existing_relation, target_relation) %}

    {% set grant_config = config.get('grants') %}

    {% call statement(name="main") %}
        {{ build_sql }}
    {% endcall %}

    {% set should_revoke = should_revoke(existing_relation, full_refresh_mode=True) %}
    {% do apply_grants(target_relation, grant_config, should_revoke=should_revoke) %}

    {% do persist_docs(target_relation, model) %}

    {{ adapter.commit() }}

{% endmacro %}


{# DIVERGENCE #}
{% macro get_function_macro(function_type, function_language) %}
    {{ return(adapter.dispatch('get_function_macro', 'dbt')(function_type, function_language)) }}
{% endmacro %}


{# DIVERGENCE #}
{% macro default__get_function_macro(function_type, function_language) %}
    {% set macro_name = function_type ~ "_function_" ~ function_language %}
    {% if not macro_name in context %}
        {{ exceptions.raise_not_implemented(function_language ~ ' ' ~ function_type ~ ' function not implemented for adapter ' ~adapter.type()) }}
    {% endif %}
    {% set macro = context[macro_name] %}
    {{ return(macro) }}
{% endmacro %}
