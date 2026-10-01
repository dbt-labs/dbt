{% macro databricks__scalar_function_create_replace_signature_sql(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql()}})
    RETURNS {{ model.returns.data_type }}
    LANGUAGE SQL
{% endmacro %}

{% macro databricks__scalar_function_body_sql() %}
    RETURN
    {{ model.compiled_code }}
{% endmacro %}

{# Python UDF signature macro #}
{% macro databricks__scalar_function_create_replace_signature_python(target_relation) %}
    CREATE OR REPLACE FUNCTION {{ target_relation.render() }} ({{ formatted_scalar_function_args_sql() }})
    RETURNS {{ model.returns.data_type }}
    LANGUAGE PYTHON
    AS
{% endmacro %}

{# Python UDF body macro - uses dollar-quoting #}
{% macro databricks__scalar_function_body_python() %}
$$
{{ model.compiled_code }}
$$
{% endmacro %}

{# Main Python UDF macro - combines signature and body #}
{% macro databricks__scalar_function_python(target_relation) %}
    {#- Warn if user explicitly provided no-op config fields -#}
    {%- if model.config.get('runtime_version') -%}
        {{ exceptions.warn("'runtime_version' is accepted for compatibility but has no effect on Databricks Python UDFs. Databricks manages the Python runtime internally.") }}
    {%- endif -%}
    {%- if model.config.get('entry_point') -%}
        {{ exceptions.warn("'entry_point' is accepted for compatibility but has no effect on Databricks Python UDFs. The function body is used directly.") }}
    {%- endif -%}
    {{ databricks__scalar_function_create_replace_signature_python(target_relation) }}
    {{ databricks__scalar_function_body_python() }}
{% endmacro %}
