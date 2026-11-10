{# DIVERGENCE: core takes the individual changeset entries (target_lag, warehouse, init_warehouse); Fusion takes the whole `configuration_changes` object and reads the components off it. #}
-- funcsign: (snowflake_node_config, optional[changeset_entry]) -> bool
{% macro snowflake__target_lag_warehouse_alter_active(configuration_changes, scheduler=none) %}
{#- Broader than `has_set_changes` in the sibling macro below: an initialization_warehouse
    being cleared emits only the UNSET statement, and still counts here. Callers use this
    to decide whether their own following ALTER clause needs a leading semicolon. -#}
    {% do return(
        configuration_changes.target_lag
        or configuration_changes.snowflake_warehouse
        or configuration_changes.snowflake_initialization_warehouse
        or scheduler
    ) %}
{% endmacro %}


{# DIVERGENCE: core takes the individual changeset entries (target_lag, warehouse, init_warehouse); Fusion takes the whole `configuration_changes` object and reads the components off it. #}
-- funcsign: (string, relation, snowflake_node_config, optional[changeset_entry]) -> string
{% macro snowflake__get_target_lag_warehouse_alter_sql(table_kind, existing_relation, configuration_changes, scheduler=none) -%}
{#-
    Produce the `alter <table_kind> table ... set ...` statement plus the separate
    `unset initialization_warehouse` statement when it's being cleared.

    `scheduler` is dynamic-table-only and is taken as its own argument rather than read off
    `configuration_changes` here, because an interactive table's changeset never carries that
    component at all -- reaching for it unconditionally would fail for every interactive-table
    caller instead of just doing nothing.

    Args:
    - table_kind: str - 'dynamic' or 'interactive'
    - existing_relation: SnowflakeRelation - the relation being altered
    - configuration_changes: snowflake_node_config - the changeset carrying target_lag /
      snowflake_warehouse / snowflake_initialization_warehouse
    - scheduler: optional changeset entry for the dynamic-table-only `scheduler` component
    Returns:
        The SET statement, the UNSET statement, or both (`;`-separated), or an empty string when
        none of the components changed.
-#}
    {%- set target_lag = configuration_changes.target_lag -%}
    {%- if target_lag -%}{{- log('Applying UPDATE TARGET_LAG to: ' ~ existing_relation) -}}{%- endif -%}
    {%- set snowflake_warehouse = configuration_changes.snowflake_warehouse -%}
    {%- if snowflake_warehouse -%}{{- log('Applying UPDATE WAREHOUSE to: ' ~ existing_relation) -}}{%- endif -%}
    {%- set snowflake_initialization_warehouse = configuration_changes.snowflake_initialization_warehouse -%}
    {%- if snowflake_initialization_warehouse and snowflake_initialization_warehouse.context -%}{{- log('Applying UPDATE INITIALIZATION_WAREHOUSE to: ' ~ existing_relation) -}}{%- endif -%}
    {%- if scheduler -%}{{- log('Applying UPDATE SCHEDULER to: ' ~ existing_relation) -}}{%- endif -%}

    {%- set has_set_changes = target_lag or snowflake_warehouse or (snowflake_initialization_warehouse and snowflake_initialization_warehouse.context) or scheduler -%}

    {% if has_set_changes %}
    alter {{ table_kind }} table {{ existing_relation }} set
        {% if target_lag and target_lag.context %}target_lag = '{{ target_lag.context }}'{% endif %}
        {% if snowflake_warehouse %}warehouse = {{ snowflake_warehouse.context }}{% endif %}
        {% if snowflake_initialization_warehouse and snowflake_initialization_warehouse.context %}initialization_warehouse = {{ snowflake_initialization_warehouse.context }}{% endif %}
        {% if scheduler %}scheduler = '{{ scheduler.context }}'{% endif %}
    {% endif %}

    {% if snowflake_initialization_warehouse and not snowflake_initialization_warehouse.context %}
    {% if has_set_changes %};{% endif %}
    alter {{ table_kind }} table {{ existing_relation }} unset initialization_warehouse
    {% endif %}
{%- endmacro %}
