-- funcsign: (relation, string) -> string
{% macro snowflake__get_replace_interactive_table_sql(relation, sql) -%}

    {%- set interactive_table = relation.from_config(config.model) -%}
    {{ snowflake__replace_interactive_table_sql(interactive_table, relation, sql) }}

{%- endmacro %}


{% macro snowflake__replace_interactive_table_sql(interactive_table, relation, sql) -%}
{#-
    Produce DDL that replaces an interactive table with a new interactive table

    The option clauses come from `snowflake__interactive_table_options_sql`, which the create
    form uses too, so the two forms cannot diverge.

    Args:
    - interactive_table: the resolved interactive table configuration
    - relation: Union[SnowflakeRelation, str]
        - SnowflakeRelation - required for relation.render()
        - str - is already the rendered relation name
    - sql: str - the code defining the model
    Returns:
        A valid DDL statement which will result in a new interactive table.
-#}
    create or replace interactive table {{ relation }}
        {{ snowflake__interactive_table_options_sql(interactive_table) }}
        as (
            {{ sql }}
        )

{%- endmacro %}
