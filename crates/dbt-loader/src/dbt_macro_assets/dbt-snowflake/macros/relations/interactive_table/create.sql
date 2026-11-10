-- funcsign: (relation, string) -> string
{% macro snowflake__get_create_interactive_table_as_sql(relation, sql) -%}

    {%- set interactive_table = relation.from_config(config.model) -%}
    {{ snowflake__create_interactive_table_sql(interactive_table, relation, sql) }}

{%- endmacro %}


{% macro snowflake__create_interactive_table_sql(interactive_table, relation, sql) -%}
{#-
    Produce DDL that creates an interactive table

    Args:
    - interactive_table: the resolved interactive table configuration
    - relation: Union[SnowflakeRelation, str]
        - SnowflakeRelation - required for relation.render()
        - str - is already the rendered relation name
    - sql: str - the code defining the model
    Returns:
        A valid DDL statement which will result in a new interactive table.
-#}
    create interactive table {{ relation }}
        {{ snowflake__interactive_table_options_sql(interactive_table) }}
        as (
            {{ sql }}
        )

{%- endmacro %}


{# DIVERGENCE: core shares the whole DDL body between the create and replace forms via snowflake__interactive_table_ddl_body_sql(..., ddl_prefix=...); Fusion shares only the option clauses and writes each prefix inline. #}
{% macro snowflake__interactive_table_options_sql(interactive_table) -%}
{#-
    Produce the option clauses shared by the create and the create-or-replace forms.

    `target_lag` is what makes an interactive table refresh on its own. Without it the
    relation is a plain table with no warehouse attached, and Snowflake rejects the
    combination of no target lag with a warehouse, so `target_lag`, `warehouse` and
    `initialization_warehouse` are emitted together or not at all.

    `transient` is never emitted in any form: there is no valid transient interactive
    table DDL.

    Args:
    - interactive_table: the resolved interactive table configuration
    Returns:
        The option clauses, or an empty string when the configuration sets none of them.
-#}
        {{ optional('cluster by', interactive_table.cluster_by, quote_char='(', equals_char='') }}
        {%- if interactive_table.target_lag is not none %}
        target_lag = '{{ interactive_table.target_lag }}'
        warehouse = {{ interactive_table.snowflake_warehouse }}
        {{ optional('initialization_warehouse', interactive_table.snowflake_initialization_warehouse) }}
        {%- endif %}
{%- endmacro %}
