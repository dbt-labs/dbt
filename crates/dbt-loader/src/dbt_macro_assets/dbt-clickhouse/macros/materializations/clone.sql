{% macro clickhouse__can_clone_table() %}
    {#
        Clone behavior with ClickHouse (dbt clone only operates on 'table'-type materializations, for the rest it falls back to view):
        - *MergeTree tables  -> cloned via `CLONE AS`.
        - Tables with other engines -> not supported, fall back to a view.
        - Distributed tables -> not supported, fall back to a view.
        - Materialized views -> only the target table is cloned; now MV is crated/attached to the clon.
    #}
    {%- if config.get('materialized', '').startswith('distributed_') -%}
        {{ return(False) }}
    {%- endif -%}
    {%- set engine = config.get('engine', default='MergeTree()') -%}
    {{ return('MergeTree' in engine) }}
{% endmacro %}

{% macro clickhouse__create_or_replace_clone(this_relation, defer_relation) %}
    CREATE OR REPLACE TABLE {{ this_relation }} {{ on_cluster_clause(this_relation) }} CLONE AS {{ defer_relation }}
{% endmacro %}
