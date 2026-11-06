-- funcsign: (relation, string) -> string
{% macro changes(relation, mode='default') -%}
{#-
    The change source of a custom incremental dynamic table:
    `select ... from {{ changes(ref('orders')) }} as o`.

    Renders `<relation> changes(information => DEFAULT|APPEND_ONLY)` with no bounds, which a CI DT
    refresh binds to its interval. While the materialization discovers the model's columns it
    renders the same clause starting now, `at(offset => 0)`, which is runnable and returns no rows
    but the same columns. (`at(timestamp => current_timestamp())` is rejected: change tracking
    queries cannot time travel to a non-deterministic function, error 091928.)

    Args:
    - relation: the change source, usually `ref(...)` or `source(...)`
    - mode: 'default' (inserts, updates and deletes) or 'append_only' (inserts only)
-#}
    {%- set information = {'default': 'DEFAULT', 'append_only': 'APPEND_ONLY'}.get(mode | lower) -%}
    {%- if information is none -%}
        {% do exceptions.raise_compiler_error(
            "changes(): unsupported mode '" ~ mode ~ "'. Supported modes: 'default', 'append_only'."
        ) %}
    {%- endif -%}
    {{ relation }} changes(information => {{ information }})
    {%- if model is defined and model.get('_dbt_changes_discovery') %} at(offset => 0){% endif -%}
{%- endmacro %}
