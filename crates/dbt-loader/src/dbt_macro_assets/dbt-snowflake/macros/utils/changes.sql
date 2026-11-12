-- funcsign: (relation, string) -> string
{% macro changes(relation, mode) -%}
{#-
    The change source of a custom incremental dynamic table:
    `select ... from {{ changes(ref('orders'), 'default') }} as o`.

    Renders `<relation> changes(information => DEFAULT|APPEND_ONLY)` with no time bounds, which a
    CI DT refresh binds to its interval. Under `dbt run --empty` it renders
    `(select * from <relation> changes(...) where false limit 0)` instead: the relation renders as
    a zero-row subquery there, and CHANGES must directly follow a table name.

    Args:
    - relation: the change source, usually `ref(...)` or `source(...)`
    - mode: 'default' (inserts, updates and deletes) or 'append_only' (inserts only), any case
-#}
    {%- if mode is undefined or mode is none -%}
        {% do exceptions.raise_compiler_error(
            "changes(): `mode` is required: changes(<relation>, 'default') or changes(<relation>, 'append_only')."
        ) %}
    {%- endif -%}
    {%- set information = {'default': 'DEFAULT', 'append_only': 'APPEND_ONLY'}.get(mode | lower) -%}
    {%- if information is none -%}
        {% do exceptions.raise_compiler_error(
            "changes(): unsupported mode '" ~ mode ~ "'. Supported modes: 'default', 'append_only'."
        ) %}
    {%- endif -%}
    {#- `relation.render()` is the plain name even under --empty, where `{{ relation }}` is a subquery. -#}
    {%- set table = relation if relation is string else relation.render() -%}
    {%- set clause = table ~ ' changes(information => ' ~ information ~ ')' -%}
    {%- if flags is defined and flags.EMPTY -%}
        (select * from {{ clause }} where false limit 0)
    {%- else -%}
        {{ clause }}
        {%- if model is defined and model.get('_dbt_changes_discovery') %} at(offset => 0){% endif -%}
    {%- endif -%}
{%- endmacro %}
