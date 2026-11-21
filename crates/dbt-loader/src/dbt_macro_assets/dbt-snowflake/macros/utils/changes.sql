-- funcsign: (relation, string) -> string
{% macro changes(relation, mode) -%}
{#-
    The change source of a custom incremental dynamic table:
    `select ... from {{ changes(ref('orders'), 'default') }} as o`.

    Renders `<relation> changes(information => DEFAULT|APPEND_ONLY)` with no time bounds, which a
    CI DT refresh binds to its interval. Under `dbt run --empty` it renders
    `(select * from <relation> changes(...) limit 0)` instead: the relation renders as a zero-row
    subquery there, and CHANGES must directly follow a table name. (Not `where false`: a constant
    false filter on a CI DT's change source fails with 002707 "depends on one or more dropped base
    table".)

    The table name is the relation's own (`relation.render()`), so a `--sample` or event_time
    filter on the relation does not apply to its CHANGES.

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
    {#- CHANGES must directly follow a table name. `{{ relation }}` renders as a filtered subquery
        under --empty / --sample, so use the plain name from `relation.render()`. -#}
    {%- set table = relation if relation is string else relation.render() -%}
    {%- set clause = table ~ ' ' ~ snowflake__changes_clause(information) -%}
    {%- if flags is defined and flags.EMPTY -%}
        (select * from {{ clause }} limit 0)
    {%- else -%}
        {{ clause }}
    {%- endif -%}
{%- endmacro %}


-- funcsign: (string) -> string
{% macro snowflake__changes_clause(information) -%}
{#- The exact CHANGES clause `changes()` emits. Column discovery finds the macro's output by this
    text, so both sides must build it here. `information` is DEFAULT or APPEND_ONLY. -#}
    {{ return('changes(information => ' ~ information ~ ')') }}
{%- endmacro %}
