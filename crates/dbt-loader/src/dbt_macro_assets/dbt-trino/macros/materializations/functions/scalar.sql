{#--
  Trino SQL UDFs are stored in a catalog whose connector supports function storage
  (Hive, Memory, or Starburst Galaxy's `galaxy.functions`). The body must be a single
  expression: Trino rejects queries (SELECT) inside functions when the function is created.
--#}

{% macro trino__scalar_function_sql(target_relation) %}
    {{ scalar_function_create_replace_signature_sql(target_relation) }}
    {{ scalar_function_body_sql() }}
{% endmacro %}

{% macro trino__scalar_function_create_replace_signature_sql(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql() }})
    RETURNS {{ model.returns.data_type }}
    {{ scalar_function_volatility_sql() }}
    {% if model.description %}
    COMMENT '{{ model.description | replace("'", "''") }}'
    {% endif %}
{% endmacro %}

{% macro trino__scalar_function_body_sql() %}
    RETURN {{ model.compiled_code | trim }}
{% endmacro %}

{% macro trino__scalar_function_volatility_sql() %}
    {% set volatility = model.config.get('volatility') %}
    {% if volatility == 'deterministic' %}
        DETERMINISTIC
    {% elif volatility == 'non-deterministic' %}
        NOT DETERMINISTIC
    {% elif volatility != none %}
        {% do unsupported_volatility_warning(volatility) %}
    {% endif %}
{% endmacro %}

{#-- Trino has no GRANT or COMMENT ON for functions, so grants and persist_docs are skipped. --#}
{% macro trino__function_execute_build_sql(build_sql, existing_relation, target_relation) %}
    {% if config.get('grants') %}
        {% do exceptions.warn("Grants are not supported on functions by dbt-trino and will be ignored for `" ~ model.name ~ "`") %}
    {% endif %}

    {% call statement(name="main") %}
        {{ build_sql }}
    {% endcall %}

    {{ adapter.commit() }}
{% endmacro %}
